//! Offline reverse geocoding: latitude/longitude to city, region and country from the GeoNames dataset in the
//! `places` table, with no external service. See `docs/library.md` ("Places").
//!
//! Lookup: a latitude/longitude box around the point (a few hundred rows at most, from `places_lat_lon_idx`), ranked
//! by great-circle distance; the nearest place within [`MAX_DISTANCE_KM`] wins. Farther than that (open sea, polar
//! regions, empty desert) a photo gets no place rather than a misleading one.
use std::{collections::HashMap, io::BufRead};

use anyhow::{Context, Result, bail};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::jobs;

/// A photo farther than this from every place in the dataset has no place. Towns of 1000+ people are rarely more
/// than a few tens of kilometres apart outside deserts, mountains and the sea.
pub const MAX_DISTANCE_KM: f64 = 50.0;

/// Kilometres per degree of latitude (slightly low on purpose, so the search box is never too small).
const KM_PER_DEGREE: f64 = 110.5;

#[derive(Debug, Clone, PartialEq)]
pub struct Place {
    pub city: String,
    pub region: Option<String>,
    pub country: String,
    pub country_code: String,
    pub distance_km: f64,
}

/// The latitude/longitude box that contains every point within `km` of (`lat`, `lon`): `(south, north)` and up to two
/// longitude ranges (two when the box crosses the antimeridian; a range that is empty is `(1.0, 0.0)`).
fn search_box(lat: f64, lon: f64, km: f64) -> (f64, f64, [(f64, f64); 2]) {
    let dlat = km / KM_PER_DEGREE;
    let (south, north) = ((lat - dlat).max(-90.0), (lat + dlat).min(90.0));
    // Meridians converge towards the poles; use the cosine at the box edge farthest from the equator, and search
    // every longitude when the box reaches a pole.
    let worst = south.abs().max(north.abs());
    let none = (1.0, 0.0);
    if worst >= 89.0 {
        return (south, north, [(-180.0, 180.0), none]);
    }
    let dlon = km / (KM_PER_DEGREE * worst.to_radians().cos());
    if dlon >= 180.0 {
        return (south, north, [(-180.0, 180.0), none]);
    }
    let (w, e) = (lon - dlon, lon + dlon);
    let ranges = if w < -180.0 {
        [(-180.0, e), (w + 360.0, 180.0)]
    } else if e > 180.0 {
        [(w, 180.0), (-180.0, e - 360.0)]
    } else {
        [(w, e), none]
    };
    (south, north, ranges)
}

/// The nearest place within [`MAX_DISTANCE_KM`], or None. Errors only on database failure.
pub async fn nearest(db: &PgPool, lat: f64, lon: f64) -> Result<Option<Place>> {
    if !(lat.is_finite() && lon.is_finite() && lat.abs() <= 90.0 && lon.abs() <= 180.0) {
        return Ok(None);
    }
    let (south, north, [(w1, e1), (w2, e2)]) = search_box(lat, lon, MAX_DISTANCE_KM);
    let row = sqlx::query(
        "SELECT name, region, country, country_code, dist FROM (
             SELECT name, region, country, country_code, population,
                    2 * 6371.0088 * asin(least(1.0, sqrt(
                        power(sin(radians(latitude - $1) / 2), 2)
                        + cos(radians($1)) * cos(radians(latitude)) * power(sin(radians(longitude - $2) / 2), 2)
                    ))) AS dist
             FROM places
             WHERE latitude BETWEEN $3 AND $4
               AND ((longitude BETWEEN $5 AND $6) OR (longitude BETWEEN $7 AND $8))
         ) p
         WHERE dist <= $9
         ORDER BY dist, population DESC, name LIMIT 1",
    )
    .bind(lat)
    .bind(lon)
    .bind(south)
    .bind(north)
    .bind(w1)
    .bind(e1)
    .bind(w2)
    .bind(e2)
    .bind(MAX_DISTANCE_KM)
    .fetch_optional(db)
    .await
    .context("nearest place")?;
    Ok(row.map(|r| Place {
        city: r.get("name"),
        region: r.get("region"),
        country: r.get("country"),
        country_code: r.get("country_code"),
        distance_km: r.get("dist"),
    }))
}

/// Whether a dataset has been loaded (migrations alone create the table empty).
pub async fn dataset_loaded(db: &PgPool) -> Result<bool> {
    Ok(sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM place_dataset)")
        .fetch_one(db)
        .await?)
}

#[derive(Debug, PartialEq, Eq)]
pub enum Geocoded {
    /// The photo now has this place.
    Placed,
    /// It has coordinates but nothing in the dataset is close enough; any earlier place was cleared.
    NoPlace,
    /// It has no coordinates (any earlier place was cleared).
    NoCoordinates,
    /// The asset no longer exists.
    Gone,
}

/// The body of the `geocode` job: look the photo's coordinates up and store the place on the asset. Idempotent.
/// Fails (so the job retries) while no dataset is loaded, rather than recording "no place" for everything.
pub async fn geocode_asset(db: &PgPool, asset_id: Uuid) -> Result<Geocoded> {
    let Some((lat, lon)) =
        sqlx::query_as::<_, (Option<f64>, Option<f64>)>("SELECT latitude, longitude FROM assets WHERE id = $1")
            .bind(asset_id)
            .fetch_optional(db)
            .await?
    else {
        return Ok(Geocoded::Gone);
    };
    let place = match (lat, lon) {
        (Some(lat), Some(lon)) => {
            if !dataset_loaded(db).await? {
                bail!("the places dataset is not loaded yet (`cyanotype migrate` loads it)");
            }
            nearest(db, lat, lon).await?
        }
        _ => None,
    };
    sqlx::query(
        "UPDATE assets SET place_city = $2, place_region = $3, place_country = $4, place_country_code = $5 WHERE id = $1
         AND (place_city, place_region, place_country, place_country_code) IS DISTINCT FROM ($2, $3, $4, $5)",
    )
    .bind(asset_id)
    .bind(place.as_ref().map(|p| &p.city))
    .bind(place.as_ref().and_then(|p| p.region.as_ref()))
    .bind(place.as_ref().map(|p| &p.country))
    .bind(place.as_ref().map(|p| &p.country_code))
    .execute(db)
    .await?;
    Ok(match (&place, lat.is_some() && lon.is_some()) {
        (Some(_), _) => Geocoded::Placed,
        (None, true) => Geocoded::NoPlace,
        (None, false) => Geocoded::NoCoordinates,
    })
}

/// Queue (or re-queue, when it already finished) a geocode job for each given geotagged-or-not asset. Used after the
/// metadata job, which may have changed the coordinates, and after a new dataset is loaded.
pub async fn requeue(db: impl sqlx::PgExecutor<'_>, asset_id: Uuid) -> Result<()> {
    sqlx::query(
        "INSERT INTO jobs (kind, asset_id, version) VALUES ($1, $2, 1)
         ON CONFLICT (kind, asset_id, version) DO UPDATE
             SET state = 'queued', attempts = 0, run_after = now(), locked_at = NULL, finished_at = NULL, last_error = NULL
             WHERE jobs.state IN ('done', 'failed')",
    )
    .bind(jobs::KIND_GEOCODE)
    .bind(asset_id)
    .execute(db)
    .await?;
    Ok(())
}

/// Queue a geocode job for every geotagged photo that has none yet, and re-queue the finished ones: the backfill
/// after a new dataset. Idempotent. Returns the number of jobs inserted or re-queued.
pub async fn requeue_all(db: &PgPool) -> Result<u64> {
    let r = sqlx::query(
        "INSERT INTO jobs (kind, asset_id, version)
         SELECT 'geocode', id, 1 FROM assets WHERE latitude IS NOT NULL AND longitude IS NOT NULL
         ON CONFLICT (kind, asset_id, version) DO UPDATE
             SET state = 'queued', attempts = 0, run_after = now(), locked_at = NULL, finished_at = NULL, last_error = NULL
             WHERE jobs.state IN ('done', 'failed')",
    )
    .execute(db)
    .await?;
    Ok(r.rows_affected())
}

/// What a dataset load did.
#[derive(Debug, PartialEq, Eq)]
pub enum Loaded {
    /// The same version was already loaded; nothing changed.
    UpToDate { version: String },
    Loaded {
        version: String,
        places: usize,
        requeued: u64,
    },
}

/// Load the dataset (the text produced by `scripts/build-places-dataset.sh`, already gunzipped) unless its version is
/// the one already loaded. Replaces the table in one transaction; a new version queues geocode jobs again.
pub async fn load_dataset(db: &PgPool, text: impl BufRead) -> Result<Loaded> {
    let ds = parse(text)?;
    let current: Option<String> = sqlx::query_scalar("SELECT version FROM place_dataset")
        .fetch_optional(db)
        .await?;
    if current.as_deref() == Some(ds.version.as_str()) {
        return Ok(Loaded::UpToDate { version: ds.version });
    }
    let mut tx = db.begin().await?;
    sqlx::query("TRUNCATE places").execute(&mut *tx).await?;
    let mut copy = tx
        .copy_in_raw("COPY places (name, region, country, country_code, latitude, longitude, population) FROM STDIN")
        .await?;
    copy.send(ds.copy_text.as_bytes()).await?;
    copy.finish().await?;
    sqlx::query(
        "INSERT INTO place_dataset (version, places) VALUES ($1, $2)
         ON CONFLICT (only_row) DO UPDATE SET version = $1, places = $2, loaded_at = now()",
    )
    .bind(&ds.version)
    .bind(ds.places as i32)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    sqlx::query("ANALYZE places").execute(db).await?;
    // Photos geocoded against an older snapshot (or before there was one) are looked up again.
    let requeued = requeue_all(db).await?;
    Ok(Loaded::Loaded {
        version: ds.version,
        places: ds.places,
        requeued,
    })
}

struct Dataset {
    version: String,
    places: usize,
    /// `COPY ... FROM STDIN` text format, one row per place.
    copy_text: String,
}

/// COPY text format escapes backslash, tab, newline and carriage return.
fn copy_escape(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
}

fn parse(text: impl BufRead) -> Result<Dataset> {
    let mut version = None;
    let mut countries: HashMap<String, String> = HashMap::new();
    let mut regions: HashMap<(String, String), String> = HashMap::new();
    let mut raw: Vec<(String, f64, f64, String, String, i32)> = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line?;
        let f: Vec<&str> = line.split('\t').collect();
        let bad = || format!("places dataset line {}: malformed", n + 1);
        match f.as_slice() {
            ["#version", v] => version = Some(v.to_string()),
            [c, ..] if c.starts_with('#') => {}
            ["C", cc, name] => {
                countries.insert(cc.to_string(), name.to_string());
            }
            ["A", cc, code, name] => {
                regions.insert((cc.to_string(), code.to_string()), name.to_string());
            }
            ["P", name, lat, lon, cc, admin1, pop] => raw.push((
                name.to_string(),
                lat.parse().with_context(bad)?,
                lon.parse().with_context(bad)?,
                cc.to_string(),
                admin1.to_string(),
                pop.parse().with_context(bad)?,
            )),
            [""] => {}
            _ => bail!(bad()),
        }
    }
    let version = version.context("places dataset has no #version line")?;
    if raw.is_empty() {
        bail!("places dataset has no places");
    }
    let mut copy_text = String::with_capacity(raw.len() * 64);
    for (name, lat, lon, cc, admin1, pop) in &raw {
        let country = countries.get(cc).map_or(cc.as_str(), String::as_str);
        let region = regions.get(&(cc.clone(), admin1.clone()));
        copy_escape(name, &mut copy_text);
        copy_text.push('\t');
        match region {
            Some(r) => copy_escape(r, &mut copy_text),
            None => copy_text.push_str("\\N"),
        }
        copy_text.push('\t');
        copy_escape(country, &mut copy_text);
        copy_text.push('\t');
        copy_escape(cc, &mut copy_text);
        copy_text.push_str(&format!("\t{lat}\t{lon}\t{pop}\n"));
    }
    Ok(Dataset {
        version,
        places: raw.len(),
        copy_text,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn box_far_from_the_poles_and_the_antimeridian() {
        let (s, n, [a, b]) = search_box(48.85, 2.35, 50.0);
        assert!((n - s - 2.0 * 50.0 / KM_PER_DEGREE).abs() < 1e-9);
        assert!(a.0 < 2.35 && a.1 > 2.35);
        assert!(b.0 > b.1, "second range is empty");
    }

    #[test]
    fn box_wraps_across_the_antimeridian() {
        let (_, _, [a, b]) = search_box(-16.8, 179.9, 50.0); // Fiji
        assert_eq!(a.1, 180.0);
        assert!(a.0 < 179.9);
        assert_eq!(b.0, -180.0);
        assert!(b.1 < -179.0 && b.1 > -180.0);
        let (_, _, [a, b]) = search_box(-16.8, -179.9, 50.0);
        assert_eq!(a.0, -180.0);
        assert_eq!(b.1, 180.0);
        assert!(b.0 > 179.0);
    }

    #[test]
    fn box_near_a_pole_covers_every_longitude() {
        let (_, n, [a, _]) = search_box(89.9, 10.0, 50.0);
        assert_eq!(n, 90.0);
        assert_eq!(a, (-180.0, 180.0));
    }

    #[test]
    fn parses_and_joins_names() {
        let text = "#version\tv1\nC\tFR\tFrance\nA\tFR\t11\tÎle-de-France\nP\tParis\t48.853\t2.349\tFR\t11\t2138551\nP\tNowhere\t1.000\t2.000\tZZ\t\t5\n";
        let d = parse(text.as_bytes()).unwrap();
        assert_eq!((d.version.as_str(), d.places), ("v1", 2));
        assert!(
            d.copy_text
                .starts_with("Paris\tÎle-de-France\tFrance\tFR\t48.853\t2.349\t2138551\n")
        );
        assert!(d.copy_text.contains("Nowhere\t\\N\tZZ\tZZ\t"));
        assert!(parse("#version\tv\n".as_bytes()).is_err());
        assert!(parse("P\tx\t1\t2\n".as_bytes()).is_err());
    }
}
