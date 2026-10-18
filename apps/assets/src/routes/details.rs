//! `GET /assets/{id}/details`: what the photo details panel shows (docs/library.md, "Photo details"), derived from the
//! asset's columns and its stored EXIF dump (`crate::details`), so photos imported long ago need no reprocessing. Same
//! rules as `GET /assets/{id}`: the caller's own photo, not in the trash; unknown or someone else's is a 404.
//!
//! Share links never see any of it: the public share endpoints do not use this module, and a shared photo's derived
//! images carry no metadata at all (docs/library.md, "Sharing").
use axum::{
    Json,
    extract::{Path, State},
};
use serde::Serialize;
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use super::albums;
use crate::{
    AppState,
    details::{self, DateInfo, Exposure},
};
use cyanotype_service::{ApiError, ApiResult, AuthUser};

#[derive(FromRow)]
struct Row {
    original_name: String,
    content_type: String,
    size_bytes: i64,
    width: Option<i32>,
    height: Option<i32>,
    taken_at: Option<OffsetDateTime>,
    file_modified_at: Option<OffsetDateTime>,
    created_at: OffsetDateTime,
    latitude: Option<f64>,
    longitude: Option<f64>,
    camera_make: Option<String>,
    camera_model: Option<String>,
    camera_label: Option<String>,
    lens_model: Option<String>,
    exif: Option<serde_json::Value>,
    file_ext: Option<String>,
    place_city: Option<String>,
    place_region: Option<String>,
    place_country: Option<String>,
    place_country_code: Option<String>,
    burst_id: Option<Uuid>,
}

#[derive(Serialize)]
pub(super) struct Details {
    id: Uuid,
    date: DateInfo,
    location: Option<Location>,
    camera: Option<Camera>,
    /// Null when the EXIF has none of the exposure settings.
    exposure: Option<Exposure>,
    file: File,
    /// Every file of the photo: the photo itself and, for a RAW + JPEG pair, the RAW behind it.
    files: Vec<FileEntry>,
    albums: serde_json::Value,
    burst: Option<BurstPosition>,
}

#[derive(Serialize)]
struct Location {
    latitude: f64,
    longitude: f64,
    /// The nearest populated place from the offline dataset; null when nothing is near.
    place: Option<Place>,
    #[serde(flatten)]
    gps: details::GpsExtras,
}

#[derive(Serialize)]
struct Place {
    city: String,
    region: Option<String>,
    country: String,
    country_code: Option<String>,
}

#[derive(Serialize)]
struct Camera {
    /// The camera as the search filter and its facets name it (`Canon EOS R5`); null without make or model.
    label: Option<String>,
    make: Option<String>,
    model: Option<String>,
    /// The lens as the search filter names it.
    lens: Option<String>,
}

#[derive(Serialize)]
struct File {
    name: String,
    /// `JPEG`, `HEIC`, `CR3`...
    format: String,
    raw: bool,
    content_type: String,
    size_bytes: i64,
    width: Option<i32>,
    height: Option<i32>,
    /// `12 MP`.
    megapixels: Option<String>,
    color_space: Option<String>,
    software: Option<String>,
}

#[derive(Serialize)]
struct FileEntry {
    id: Uuid,
    name: String,
    format: String,
    raw: bool,
    size_bytes: i64,
}

#[derive(Serialize)]
struct BurstPosition {
    /// 1-based, in capture order.
    position: i64,
    size: i64,
}

pub(super) async fn get(State(s): State<AppState>, user: AuthUser, Path(id): Path<Uuid>) -> ApiResult<Json<Details>> {
    let r: Row = sqlx::query_as(
        "SELECT original_name, content_type, size_bytes, width, height, taken_at, file_modified_at, created_at,
                latitude, longitude, camera_make, camera_model, camera_label, lens_model, exif, file_ext,
                place_city, place_region, place_country, place_country_code, burst_id
         FROM assets WHERE id = $1 AND owner_id = $2 AND deleted_at IS NULL",
    )
    .bind(id)
    .bind(user.id)
    .fetch_optional(&s.db)
    .await?
    .ok_or(ApiError::NotFound)?;

    let dump = r.exif.as_ref();
    let extra = details::exif_details(dump);
    let (format, raw) = details::format_of(r.file_ext.as_deref(), &r.content_type);

    let location = match (r.latitude, r.longitude) {
        (Some(latitude), Some(longitude)) => Some(Location {
            latitude,
            longitude,
            place: match (r.place_city, r.place_country) {
                (Some(city), Some(country)) => Some(Place {
                    city,
                    region: r.place_region,
                    country,
                    country_code: r.place_country_code,
                }),
                _ => None,
            },
            gps: extra.gps,
        }),
        _ => None,
    };
    let camera = (r.camera_label.is_some() || r.lens_model.is_some()).then_some(Camera {
        label: r.camera_label,
        make: r.camera_make,
        model: r.camera_model,
        lens: r.lens_model,
    });

    let mut files = vec![FileEntry {
        id,
        name: r.original_name.clone(),
        format: format.clone(),
        raw,
        size_bytes: r.size_bytes,
    }];
    for p in super::paired_files(&s.db, &[id]).await?.remove(&id).unwrap_or_default() {
        let ext = p.original_name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
        let (format, raw) = details::format_of(ext.as_deref(), &p.content_type);
        files.push(FileEntry {
            id: p.id,
            name: p.original_name,
            format,
            raw,
            size_bytes: p.size_bytes,
        });
    }

    let burst = match r.burst_id {
        Some(b) => {
            // Members are the owner's visible photos of the group, in capture order (the worker's grouping rules:
            // docs/library.md, "Bursts").
            let (position, size): (i64, i64) = sqlx::query_as(
                "SELECT count(*) FILTER (WHERE m.taken_at < $4 OR (m.taken_at = $4 AND m.id <= $3)), count(*)
                 FROM assets m WHERE m.owner_id = $2 AND m.burst_id = $1 AND m.deleted_at IS NULL AND m.primary_id IS NULL",
            )
            .bind(b)
            .bind(user.id)
            .bind(id)
            .bind(r.taken_at)
            .fetch_one(&s.db)
            .await?;
            (position > 0 && size > 1).then_some(BurstPosition { position, size })
        }
        None => None,
    };

    Ok(Json(Details {
        id,
        date: details::date_info(r.taken_at, r.file_modified_at, r.created_at, dump),
        location,
        camera,
        exposure: (!extra.exposure.is_empty()).then_some(extra.exposure),
        file: File {
            name: r.original_name,
            format,
            raw,
            content_type: r.content_type,
            size_bytes: r.size_bytes,
            width: r.width,
            height: r.height,
            megapixels: r.width.zip(r.height).and_then(|(w, h)| details::megapixels(w, h)),
            color_space: extra.file.color_space,
            software: extra.file.software,
        },
        files,
        albums: albums::containing(&s.db, user.id, id).await?,
        burst,
    }))
}
