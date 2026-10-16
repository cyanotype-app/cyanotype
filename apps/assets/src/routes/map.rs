//! `GET /assets/map`: the owner's geotagged photos clustered for the map, computed in SQL.
//!
//! The browser asks for the part of the world on screen (`bbox=west,south,east,north`) at the current zoom. Photos
//! are grouped into a fixed grid whose cell size halves with every zoom level (a cell is 64 to 128 screen pixels
//! wide), so clusters stay put while the user pans and split as they zoom in. Each cluster carries its count, its
//! centre and the bounds of its photos (a click zooms to them); a cluster of one is the photo itself, with the
//! thumbnail for its marker. Bigger clusters are drawn as count bubbles, so they carry no thumbnail.
//!
//! Why clusters and not every point: a 50,000-photo library is about 50,000 x ~70 bytes = 3.5 MB of JSON before a
//! single thumbnail URL, re-sent on every pan; clusters are bounded by the screen (at most a few hundred markers),
//! and the heap read is bounded by the bbox and `assets_geo_idx`. Measured numbers are in docs/library.md.
//!
//! The same `bbox` also filters `GET /assets` (the strip of photos in the viewport), through [`Bbox::sql`].
use axum::{
    Json,
    extract::{Query, State},
};
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use crate::AppState;
use cyanotype_service::{ApiError, ApiResult, AuthUser};

/// Deepest zoom the grid refines to (cells of about a metre). Photos at the very same spot stay one cluster.
const MAX_ZOOM: i32 = 22;
/// Safety cap on markers per response. A screen holds a few hundred cells; this only bounds absurd viewports.
const MAX_CLUSTERS: i64 = 3000;

/// A rectangle on the map. `west > east` means it crosses the antimeridian (180 degrees).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Bbox {
    pub south: f64,
    pub north: f64,
    pub west: f64,
    pub east: f64,
}

fn wrap_lon(x: f64) -> f64 {
    (x + 180.0).rem_euclid(360.0) - 180.0
}

impl Bbox {
    /// `west,south,east,north` in degrees. Longitudes may lie outside -180..180 (a map scrolled past the date line);
    /// a box 360 degrees or wider is the whole world. Latitudes are clamped to the poles.
    pub fn parse(s: &str) -> ApiResult<Self> {
        let bad = || ApiError::BadRequest("bbox must be west,south,east,north in degrees".into());
        let v: Vec<f64> = s
            .split(',')
            .map(|p| p.trim().parse::<f64>())
            .collect::<Result<_, _>>()
            .map_err(|_| bad())?;
        let [west, south, east, north] = v[..] else {
            return Err(bad());
        };
        if ![west, south, east, north].iter().all(|x| x.is_finite()) || south > north {
            return Err(bad());
        }
        // After wrapping, west > east is how a box across the antimeridian is spelled.
        let (west, east) = if east - west >= 360.0 {
            (-180.0, 180.0)
        } else {
            (wrap_lon(west), wrap_lon(east))
        };
        Ok(Self {
            south: south.clamp(-90.0, 90.0),
            north: north.clamp(-90.0, 90.0),
            west,
            east,
        })
    }

    pub fn crosses_antimeridian(&self) -> bool {
        self.west > self.east
    }

    /// ` AND latitude ... AND longitude ...` for `assets`. The numbers are validated finite `f64`s, whose `Display`
    /// is plain decimal, so they are written into the statement rather than bound: one statement text for the list
    /// whether or not a box is given, and no parameter numbering to thread through.
    pub fn sql(&self) -> String {
        let lon = if self.crosses_antimeridian() {
            format!("(longitude >= {} OR longitude <= {})", self.west, self.east)
        } else {
            format!("longitude BETWEEN {} AND {}", self.west, self.east)
        };
        format!(
            " AND latitude IS NOT NULL AND longitude IS NOT NULL AND latitude BETWEEN {} AND {} AND {lon}",
            self.south, self.north
        )
    }
}

#[derive(Deserialize)]
pub(super) struct MapQuery {
    bbox: Option<String>,
    /// Map zoom level (fractional values are floored). Default 0 (the world).
    zoom: Option<f64>,
    #[serde(default)]
    favorite: bool,
}

#[derive(FromRow)]
struct ClusterRow {
    count: i64,
    lat: f64,
    lon: f64,
    min_lat: f64,
    max_lat: f64,
    min_lon: f64,
    max_lon: f64,
    id: Uuid,
    thumbhash: Option<Vec<u8>>,
}

#[derive(Serialize)]
pub(super) struct Cluster {
    /// The photo, when `count` is 1; otherwise an arbitrary photo of the cluster (not meant to be shown).
    id: Uuid,
    lat: f64,
    lon: f64,
    count: i64,
    /// `[west, south, east, north]` of the cluster's photos; null for a single photo.
    bounds: Option<[f64; 4]>,
    /// Only for a single photo; null until the worker has produced the derived images.
    thumbhash: Option<String>,
    /// Presigned thumbnail of the single photo; null for clusters and until the derived images exist.
    thumb_url: Option<String>,
}

#[derive(Serialize)]
pub(super) struct MapResponse {
    clusters: Vec<Cluster>,
    /// Photos in the box (the sum of the counts).
    total: i64,
    /// More cells than [`MAX_CLUSTERS`] matched; zoom in.
    truncated: bool,
}

/// Grid cell size in degrees for a zoom level: 360 / 2^(zoom + 3), i.e. 64 px at that zoom on a 512 px tile map.
fn cell_degrees(zoom: f64) -> f64 {
    let z = if zoom.is_finite() { zoom.floor() as i32 } else { 0 };
    360.0 / f64::from(2_i32.pow((z.clamp(0, MAX_ZOOM) + 3) as u32))
}

pub(super) async fn clusters(
    State(s): State<AppState>,
    user: AuthUser,
    Query(q): Query<MapQuery>,
) -> ApiResult<Json<MapResponse>> {
    let bbox = match &q.bbox {
        Some(b) => Bbox::parse(b)?,
        None => Bbox::parse("-180,-90,180,90")?,
    };
    let cell = cell_degrees(q.zoom.unwrap_or(0.0));
    let fav = if q.favorite { " AND favorite" } else { "" };
    // `cell` is a power of two fraction of 360, so the floor() divisions are exact. No ORDER BY inside the
    // aggregates: that would sort every matching row (about 300 ms for 50,000 photos at world zoom, against 90 ms
    // hashed); only single photos use their `id` and thumbhash, and for them any order is the same.
    let rows: Vec<ClusterRow> = sqlx::query_as(&format!(
        "SELECT count(*) AS count, avg(latitude) AS lat, avg(longitude) AS lon,
                min(latitude) AS min_lat, max(latitude) AS max_lat, min(longitude) AS min_lon, max(longitude) AS max_lon,
                (array_agg(id))[1] AS id, (array_agg(thumbhash))[1] AS thumbhash
         FROM assets
         WHERE owner_id = $1 AND deleted_at IS NULL AND primary_id IS NULL{fav}{bbox}
         GROUP BY floor((longitude + 180) / $2), floor((latitude + 90) / $2)
         ORDER BY count(*) DESC
         LIMIT $3",
        bbox = bbox.sql()
    ))
    .bind(user.id)
    .bind(cell)
    .bind(MAX_CLUSTERS + 1)
    .fetch_all(&s.db)
    .await?;
    let truncated = rows.len() as i64 > MAX_CLUSTERS;
    let mut clusters = Vec::with_capacity(rows.len().min(MAX_CLUSTERS as usize));
    let mut total = 0;
    for r in rows.into_iter().take(MAX_CLUSTERS as usize) {
        total += r.count;
        let single = r.count == 1;
        let thumb_url = match (&r.thumbhash, single) {
            (Some(_), true) => Some(
                s.storage
                    .presign_get(&cyanotype_core::keys::thumb(user.id, r.id))
                    .await?,
            ),
            _ => None,
        };
        clusters.push(Cluster {
            id: r.id,
            lat: r.lat,
            lon: r.lon,
            count: r.count,
            bounds: (r.count > 1).then_some([r.min_lon, r.min_lat, r.max_lon, r.max_lat]),
            thumbhash: r.thumbhash.as_deref().filter(|_| single).map(|h| B64.encode(h)),
            thumb_url,
        });
    }
    Ok(Json(MapResponse {
        clusters,
        total,
        truncated,
    }))
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn parses_and_normalises_boxes() {
        let b = Bbox::parse("-10,40,10,50").unwrap();
        assert_eq!((b.west, b.south, b.east, b.north), (-10.0, 40.0, 10.0, 50.0));
        assert!(!b.crosses_antimeridian());
        // Scrolled past the date line: 170..190 is 170..-170.
        let b = Bbox::parse("170,-20,190,-10").unwrap();
        assert_eq!((b.west, b.east), (170.0, -170.0));
        assert!(b.crosses_antimeridian());
        // Wider than the world.
        let b = Bbox::parse("-400,-95,400,95").unwrap();
        assert_eq!((b.west, b.east, b.south, b.north), (-180.0, 180.0, -90.0, 90.0));
        for bad in [
            "",
            "1,2,3",
            "1,2,3,4,5",
            "a,b,c,d",
            "1,2,3,NaN",
            "0,50,10,40",
            "0,0,inf,1",
        ] {
            assert!(Bbox::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn cells_halve_with_each_zoom() {
        assert_eq!(cell_degrees(0.0), 45.0);
        assert_eq!(cell_degrees(0.99), 45.0);
        assert_eq!(cell_degrees(1.0), 22.5);
        assert_eq!(cell_degrees(-3.0), 45.0);
        assert_eq!(cell_degrees(99.0), 360.0 / f64::from(1_u32 << 25));
        assert_eq!(cell_degrees(f64::NAN), 45.0);
    }

    #[test]
    fn sql_has_plain_decimal_numbers() {
        let sql = Bbox::parse("-0.000001,-1e-7,1e2,5").unwrap().sql();
        assert!(sql.contains("BETWEEN -0.0000001 AND 5"), "{sql}");
        assert!(!sql.contains("e-") && !sql.contains("e+"), "{sql}");
    }
}
