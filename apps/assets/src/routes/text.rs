//! `GET /assets/{id}/text`: the text the worker read off a photo, and the QR codes in it (docs/library.md, "Text in photos").
//! The caller's own photo only (unknown or someone else's is a 404, like the other photo routes), and a hidden photo's text only in
//! a session that has unlocked the Hidden view (an API key never has: a 404, as for the photo's details). A photo that has not been
//! read yet answers `status: "pending"` with nothing in it: reading is background work, after the photo's own processing.
//!
//! ```json
//! {"status": "read", "text": "line one\nline two",
//!  "lines": [{"t": "line one", "s": 0.97, "q": [x0,y0,x1,y1,x2,y2,x3,y3], "w": [{"t": "line", "q": [...]}]}],
//!  "qr": [{"t": "https://example.org", "q": [...], "link": "https://example.org"}]}
//! ```
//! Coordinates are 0..1 of the picture. `link` is present only for a QR payload that is a plain `http(s)` address; anything
//! else (a Wi-Fi login, `javascript:`) is text only. Nothing here ever opens a code.
use axum::{
    Json,
    extract::{Path, State},
};
use serde_json::{Value, json};
use sqlx::FromRow;
use uuid::Uuid;

use crate::AppState;
use cyanotype_service::{ApiError, ApiResult, AuthUser, Unlock};

#[derive(FromRow)]
struct Row {
    read: bool,
    text: Option<String>,
    lines: Option<Value>,
    qr: Option<Value>,
}

pub(super) async fn get(
    State(s): State<AppState>,
    user: AuthUser,
    unlock: Unlock,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    let row: Option<Row> = sqlx::query_as(
        "SELECT t.asset_id IS NOT NULL AS read, t.text, t.lines, t.qr
         FROM assets a LEFT JOIN asset_text t ON t.asset_id = a.id
         WHERE a.id = $1 AND a.owner_id = $2 AND a.deleted_at IS NULL AND a.purging_at IS NULL AND (NOT a.hidden OR $3)",
    )
    .bind(id)
    .bind(user.id)
    .bind(unlock.0)
    .fetch_optional(&s.db)
    .await?;
    let row = row.ok_or(ApiError::NotFound)?;
    if !row.read {
        return Ok(Json(json!({"status": "pending", "text": "", "lines": [], "qr": []})));
    }
    let qr: Vec<Value> = row
        .qr
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .map(|mut c| {
            let link = c
                .get("t")
                .and_then(Value::as_str)
                .and_then(cyanotype_ocr::qr::web_link)
                .map(str::to_string);
            if let (Some(link), Some(o)) = (link, c.as_object_mut()) {
                o.insert("link".into(), Value::String(link));
            }
            c
        })
        .collect();
    Ok(Json(json!({
        "status": "read",
        "text": row.text.unwrap_or_default(),
        "lines": row.lines.unwrap_or_else(|| json!([])),
        "qr": qr,
    })))
}
