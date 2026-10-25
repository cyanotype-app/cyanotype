//! What the slideshow and the `/frame` view need that the library lists do not give: a whole view in another order, paged,
//! and a fresh preview address for a photo about to be shown.
//!
//! - `GET /assets/slideshow?scope=<json>&order=listed|oldest|shuffle&seed=&limit=&cursor=`: the photos of a bulk-action
//!   scope (`"library"`, `"favorites"`, `{"album": id}`, `{"person": id}`, `{"search": {filters}}`, ... the JSON of
//!   [`scope`](super::scope), URL-encoded) in one order, `limit` at a time. `listed` is the order the page lists them in
//!   (newest first), `oldest` its reverse, `shuffle` a random order fixed by `seed` (any short word; the same seed gives the same
//!   order, so pages join up and nothing repeats within one pass). The answer is `{items, next_cursor, total}`; `items` are
//!   the library's list entries. Trash and undo batches are refused; the Hidden scope needs the unlocked Hidden view like
//!   everywhere else. The photos of the scope are resolved on every request (the same query "Select all" runs) and ordered
//!   in SQL, so a page costs one pass over the view, not a copy kept per client: 50,000 photos take tens of milliseconds.
//!   The cursor is a position in that order; photos added or removed between two pages may shift it by a place.
//! - `GET /assets/previews?ids=a,b,c`: for up to [`MAX_PREVIEWS`] photos, a presigned preview address (the same short-lived
//!   one `GET /assets/{id}` returns), the picture's size and its caption. A photo that is not the caller's, is in the trash,
//!   is hidden (while the Hidden view is locked) or has no derived images yet is left out. The slideshow asks for the next few
//!   photos just before it needs them, so a slideshow of hours never holds an address that has expired.
use axum::{
    Json,
    extract::{Query, State},
};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use super::{ListItem, ListRow, keys, list_items, scope, visibility};
use crate::AppState;
use cyanotype_service::{ApiError, ApiResult, AuthUser, Unlock};

const DEFAULT_PAGE: usize = 40;
const MAX_PAGE: usize = 100;
/// Most photos one `previews` call answers for.
pub(super) const MAX_PREVIEWS: usize = 24;
const MAX_SEED: usize = 64;

#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "snake_case")]
enum Order {
    #[default]
    Listed,
    Oldest,
    Shuffle,
}

#[derive(Deserialize)]
pub(super) struct SlideshowQuery {
    /// The scope's JSON (what a bulk request carries as `scope`).
    scope: String,
    #[serde(default)]
    order: Order,
    seed: Option<String>,
    limit: Option<usize>,
    cursor: Option<String>,
}

#[derive(Serialize)]
pub(super) struct Page {
    items: Vec<ListItem>,
    next_cursor: Option<String>,
    /// Photos in the whole view (the count in "12 of 340").
    total: usize,
}

pub(super) async fn slideshow(
    State(s): State<AppState>,
    user: AuthUser,
    unlock: Unlock,
    Query(q): Query<SlideshowQuery>,
) -> ApiResult<Json<Page>> {
    let scope_json: serde_json::Value =
        serde_json::from_str(&q.scope).map_err(|_| ApiError::BadRequest("scope must be JSON".into()))?;
    let selector: scope::Selector = serde_json::from_value(serde_json::json!({ "scope": scope_json }))
        .map_err(|_| ApiError::BadRequest("unknown scope".into()))?;
    let pick = selector.validated(unlock, s.faces_enabled)?;
    if let scope::Pick::Scope(scope::Scope::Trash | scope::Scope::TrashBatch(_) | scope::Scope::ArchiveBatch(_), ..) =
        &pick
    {
        return Err(ApiError::BadRequest("that view cannot be shown".into()));
    }
    let seed = q.seed.unwrap_or_default();
    if seed.len() > MAX_SEED || !seed.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(ApiError::BadRequest("seed must be a short word".into()));
    }
    let limit = q.limit.unwrap_or(DEFAULT_PAGE).clamp(1, MAX_PAGE);
    let offset: usize = match &q.cursor {
        Some(c) => c.parse().map_err(|_| ApiError::BadRequest("invalid cursor".into()))?,
        None => 0,
    };

    let mut conn = s.db.acquire().await?;
    let ids = pick.resolve(&mut conn, user.id).await?.unwrap_or_default();
    let total = ids.len();
    if offset >= total {
        return Ok(Json(Page {
            items: Vec::new(),
            next_cursor: None,
            total,
        }));
    }
    let expr = visibility::Sort::Taken.expr();
    let order_by = match q.order {
        Order::Listed => format!("{expr} DESC, id DESC"),
        Order::Oldest => format!("{expr} ASC, id ASC"),
        Order::Shuffle => "md5(id::text || $6::text), id".to_string(),
    };
    // Whatever the scope resolved to, the owner, the trash and the Hidden lock are checked again here.
    let sql = format!(
        "SELECT id, original_name, content_type, size_bytes, width, height, {expr} AS sort_time, favorite, rating, archived,
                hidden, (recipe IS NOT NULL) AS edited, thumbhash
         FROM assets
         WHERE owner_id = $1 AND deleted_at IS NULL AND primary_id IS NULL AND id = ANY($2) AND (NOT hidden OR $3)
         ORDER BY {order_by} OFFSET $4 LIMIT $5"
    );
    let query = sqlx::query_as::<_, ListRow>(&sql)
        .bind(user.id)
        .bind(&ids)
        .bind(unlock.0)
        .bind(offset as i64)
        .bind(limit as i64);
    let rows = if q.order == Order::Shuffle {
        query.bind(&seed).fetch_all(&mut *conn).await?
    } else {
        query.fetch_all(&mut *conn).await?
    };
    drop(conn);
    let next = offset + rows.len();
    let next_cursor = (next < total && !rows.is_empty()).then(|| next.to_string());
    let items = list_items(&s, user.id, rows).await?;
    Ok(Json(Page {
        items,
        next_cursor,
        total,
    }))
}

#[derive(Deserialize)]
pub(super) struct PreviewsQuery {
    ids: String,
}

#[derive(Serialize)]
struct Preview {
    id: Uuid,
    preview_url: String,
    width: Option<i32>,
    height: Option<i32>,
    caption: Option<String>,
}

#[derive(FromRow)]
struct PreviewRow {
    id: Uuid,
    width: Option<i32>,
    height: Option<i32>,
    caption: Option<String>,
}

pub(super) async fn previews(
    State(s): State<AppState>,
    user: AuthUser,
    unlock: Unlock,
    Query(q): Query<PreviewsQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let mut ids: Vec<Uuid> = Vec::new();
    for part in q.ids.split(',').filter(|p| !p.is_empty()).take(MAX_PREVIEWS + 1) {
        ids.push(Uuid::parse_str(part).map_err(|_| ApiError::BadRequest("ids must be photo ids".into()))?);
    }
    if ids.len() > MAX_PREVIEWS {
        return Err(ApiError::BadRequest(format!("at most {MAX_PREVIEWS} ids per request")));
    }
    // `thumbhash` is set only once both derived images exist: it doubles as "has a preview".
    let rows: Vec<PreviewRow> = sqlx::query_as(
        "SELECT id, width, height, caption FROM assets
         WHERE owner_id = $1 AND id = ANY($2) AND deleted_at IS NULL AND (NOT hidden OR $3) AND thumbhash IS NOT NULL",
    )
    .bind(user.id)
    .bind(&ids)
    .bind(unlock.0)
    .fetch_all(&s.db)
    .await?;
    let mut items = Vec::new();
    for r in rows {
        items.push(Preview {
            preview_url: s.storage.presign_get(&keys::preview(user.id, r.id)).await?,
            id: r.id,
            width: r.width,
            height: r.height,
            caption: r.caption,
        });
    }
    Ok(Json(serde_json::json!({ "items": items })))
}
