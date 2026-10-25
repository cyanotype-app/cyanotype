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
use cyanotype_core::recipe::Recipe;
use serde::Serialize;
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use super::albums;
use crate::{
    AppState,
    details::{self, DateInfo, Exposure},
};
use cyanotype_service::{ApiError, ApiResult, AuthUser, Unlock};

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
    caption: Option<String>,
    date_edited: bool,
    taken_at_exif: Option<OffsetDateTime>,
    taken_offset_edit: Option<i32>,
    location_edited: bool,
    latitude_exif: Option<f64>,
    longitude_exif: Option<f64>,
    recipe: Option<sqlx::types::Json<Recipe>>,
    width_orig: Option<i32>,
    height_orig: Option<i32>,
    edited_copy_at: Option<OffsetDateTime>,
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
    /// The photo's caption, if it has one.
    caption: Option<String>,
    /// What was edited, and what the file itself says (for "Revert to original").
    edits: EditState,
}

#[derive(Serialize)]
struct EditState {
    date_edited: bool,
    location_edited: bool,
    /// The date line of what the file says (when the date was edited).
    original_date: Option<DateInfo>,
    /// The coordinates the file says (when the location was edited); null when it had none.
    original_location: Option<Coordinates>,
    /// The rotate / straighten / crop recipe (`cyanotype_core::recipe`), null when the picture is shown as the file is.
    recipe: Option<Recipe>,
    /// The edited JPEG exists ("Download edited" answers at once instead of asking the worker).
    edited_copy_ready: bool,
}

#[derive(Serialize)]
struct Coordinates {
    latitude: f64,
    longitude: f64,
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

pub(super) async fn get(
    State(s): State<AppState>,
    user: AuthUser,
    unlock: Unlock,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Details>> {
    let r: Row = sqlx::query_as(
        "SELECT original_name, content_type, size_bytes, width, height, taken_at, file_modified_at, created_at,
                latitude, longitude, camera_make, camera_model, camera_label, lens_model, exif, file_ext,
                place_city, place_region, place_country, place_country_code,
                coalesce(stack_id, CASE WHEN stack_unlinked THEN NULL ELSE burst_id END) AS burst_id,
                caption, date_edited, taken_at_exif, taken_offset_edit, location_edited, latitude_exif, longitude_exif,
                recipe, width_orig, height_orig, edited_copy_at
         FROM assets WHERE id = $1 AND owner_id = $2 AND deleted_at IS NULL AND (NOT hidden OR $3)",
    )
    .bind(id)
    .bind(user.id)
    .bind(unlock.0)
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
            let (position, size): (i64, i64) = sqlx::query_as(&format!(
                "SELECT count(*) FILTER (WHERE m.taken_at < $4 OR (m.taken_at = $4 AND m.id <= $3)), count(*)
                 FROM assets m WHERE m.owner_id = $2 AND {} AND m.deleted_at IS NULL AND m.primary_id IS NULL
                   AND NOT m.hidden",
                super::stacks::in_stack_sql("m", "$1")
            ))
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
        date: details::date_info_in(
            r.taken_at,
            r.file_modified_at,
            r.created_at,
            dump,
            r.date_edited.then_some(r.taken_offset_edit),
        ),
        caption: r.caption,
        edits: EditState {
            date_edited: r.date_edited,
            location_edited: r.location_edited,
            // What the file says: its own zone (from the dump), and the fallbacks when it had no capture time.
            original_date: r
                .date_edited
                .then(|| details::date_info(r.taken_at_exif, r.file_modified_at, r.created_at, dump)),
            original_location: r
                .location_edited
                .then_some(r.latitude_exif.zip(r.longitude_exif))
                .flatten()
                .map(|(latitude, longitude)| Coordinates { latitude, longitude }),
            recipe: r.recipe.as_ref().map(|j| j.0),
            edited_copy_ready: r.edited_copy_at.is_some(),
        },
        location,
        camera,
        exposure: (!extra.exposure.is_empty()).then_some(extra.exposure),
        file: File {
            name: r.original_name,
            format,
            raw,
            content_type: r.content_type,
            size_bytes: r.size_bytes,
            // The file's own size, not the cropped picture's.
            width: r.width_orig.or(r.width),
            height: r.height_orig.or(r.height),
            megapixels: r
                .width_orig
                .or(r.width)
                .zip(r.height_orig.or(r.height))
                .and_then(|(w, h)| details::megapixels(w, h)),
            color_space: extra.file.color_space,
            software: extra.file.software,
        },
        files,
        albums: albums::containing(&s.db, user.id, id).await?,
        burst,
    }))
}
