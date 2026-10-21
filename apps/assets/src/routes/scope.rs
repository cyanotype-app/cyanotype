//! Bulk requests address photos either by explicit `ids` (at most [`MAX_IDS`]) or by a server-side `scope`: "every
//! photo in this view" except an optional `except` list. That is how "Select all" works on a library of 50,000
//! photos without the browser ever listing them.
//!
//! ```json
//! {"scope": "library" | "favorites" | "archive" | "hidden" | "trash" | {"album": "<id>"} | {"search": {"from": "2024-01-01", ...}}
//!           | {"trash_batch": "<rfc3339>"}, "except": ["<id>"]}
//! ```
//!
//! `search` takes the filters of `GET /assets/search` (never `q`: a ranked search selects what it shows, by id) and names the
//! photos a filter-only search lists. A scope names the same photos the matching list shows: not trashed (or only trashed, for `trash`), hidden RAW
//! files left out, and never a hidden photo (except `hidden`, which needs the unlocked Hidden view, and `trash`, which lists them only then), so a RAW + JPEG pair is one photo and the endpoints' usual pair handling applies. It is resolved
//! to ids inside the transaction of the operation, always for the caller's own photos. `trash_batch` is what a
//! scope-based trash returns as `batch`, so Undo restores exactly what that request trashed.
use serde::Deserialize;
use time::OffsetDateTime;
use uuid::Uuid;

use super::trash::MAX_IDS;
use cyanotype_service::{ApiError, ApiResult, Unlock};

/// Most ids an `except` list may carry (a JSON body of this many ids stays under the 2 MB default limit).
pub const MAX_EXCEPT: usize = 20_000;

#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum Scope {
    Library,
    Favorites,
    Archive,
    /// The Hidden view: refused unless this session has unlocked it.
    Hidden,
    Trash,
    Album(Uuid),
    /// Boxed: the filters are much bigger than the other scopes.
    Search(Box<super::search::SearchQuery>),
    TrashBatch(#[serde(with = "time::serde::rfc3339")] OffsetDateTime),
    /// What one scope-based archive archived (`archive`'s `batch`): the Undo of a bulk archive.
    ArchiveBatch(#[serde(with = "time::serde::rfc3339")] OffsetDateTime),
}

/// The request fields shared by all bulk endpoints.
#[derive(Deserialize, Default)]
pub(super) struct Selector {
    #[serde(default)]
    ids: Vec<Uuid>,
    scope: Option<Scope>,
    #[serde(default)]
    except: Vec<Uuid>,
}

pub(super) enum Pick {
    Ids(Vec<Uuid>),
    /// The scope, its `except` list, and whether this session has the Hidden view unlocked.
    Scope(Scope, Vec<Uuid>, bool),
}

impl Selector {
    pub(super) fn validated(mut self, unlock: Unlock) -> ApiResult<Pick> {
        match self.scope {
            None => {
                if !self.except.is_empty() {
                    return Err(ApiError::BadRequest("except needs a scope".into()));
                }
                if self.ids.len() > MAX_IDS {
                    return Err(ApiError::BadRequest(format!("at most {MAX_IDS} ids per request")));
                }
                self.ids.sort_unstable();
                self.ids.dedup();
                Ok(Pick::Ids(self.ids))
            }
            Some(scope) => {
                if !self.ids.is_empty() {
                    return Err(ApiError::BadRequest("send ids or a scope, not both".into()));
                }
                if self.except.len() > MAX_EXCEPT {
                    return Err(ApiError::BadRequest(format!("at most {MAX_EXCEPT} except ids")));
                }
                self.except.sort_unstable();
                self.except.dedup();
                if scope == Scope::Hidden {
                    unlock.require()?;
                }
                Ok(Pick::Scope(scope, self.except, unlock.0))
            }
        }
    }
}

impl Pick {
    pub(super) fn is_scope(&self) -> bool {
        matches!(self, Pick::Scope(..))
    }

    /// The ids to act on (visible photos only for a scope).
    pub(super) async fn resolve(&self, tx: &mut sqlx::PgConnection, owner: Uuid) -> ApiResult<Option<Vec<Uuid>>> {
        match self {
            Pick::Ids(ids) => Ok(Some(ids.clone())),
            Pick::Scope(scope, except, unlocked) => Ok(Some(resolve(tx, owner, scope, except, *unlocked).await?)),
        }
    }
}

/// The timeline's photos: live, not a paired RAW, not hidden, not archived.
const LIVE: &str =
    "owner_id = $1 AND deleted_at IS NULL AND primary_id IS NULL AND NOT hidden AND NOT archived AND id <> ALL($2)";
const TRASHED: &str =
    "owner_id = $1 AND deleted_at IS NOT NULL AND purging_at IS NULL AND primary_id IS NULL AND id <> ALL($2)";

pub(super) async fn resolve(
    tx: &mut sqlx::PgConnection,
    owner: Uuid,
    scope: &Scope,
    except: &[Uuid],
    unlocked: bool,
) -> ApiResult<Vec<Uuid>> {
    if let Scope::Search(q) = scope {
        return resolve_search(tx, owner, q, except).await;
    }
    // $1 is the owner and $2 the except list; $3 is the batch time or the album, where a scope has one.
    let (sql, album, at) = match scope {
        Scope::Library => (format!("SELECT id FROM assets WHERE {LIVE}"), None, None),
        // Favorites keep archived photos (as the Favorites view does), never hidden ones.
        Scope::Favorites => (
            "SELECT id FROM assets WHERE owner_id = $1 AND deleted_at IS NULL AND primary_id IS NULL AND NOT hidden
                 AND id <> ALL($2) AND favorite"
                .to_string(),
            None,
            None,
        ),
        Scope::Archive => (
            "SELECT id FROM assets WHERE owner_id = $1 AND deleted_at IS NULL AND primary_id IS NULL AND NOT hidden
                 AND id <> ALL($2) AND archived"
                .to_string(),
            None,
            None,
        ),
        Scope::ArchiveBatch(at) => (
            "SELECT id FROM assets WHERE owner_id = $1 AND deleted_at IS NULL AND primary_id IS NULL AND NOT hidden
             AND id <> ALL($2) AND archived AND archived_at = $3"
                .to_string(),
            None,
            Some(*at),
        ),
        Scope::Hidden => {
            // `validated` refused a locked session already; this is the second lock on the same door.
            if !unlocked {
                return Err(ApiError::NotFound);
            }
            (
                "SELECT id FROM assets WHERE owner_id = $1 AND deleted_at IS NULL AND primary_id IS NULL
                 AND id <> ALL($2) AND hidden"
                    .to_string(),
                None,
                None,
            )
        }
        // Trashed hidden photos are in the trash only for an unlocked session: a locked one neither sees nor restores them.
        Scope::Trash => (
            format!("SELECT id FROM assets WHERE {TRASHED}{}", hidden_clause("", unlocked)),
            None,
            None,
        ),
        Scope::TrashBatch(at) => (
            format!(
                "SELECT id FROM assets WHERE {TRASHED}{} AND deleted_at = $3",
                hidden_clause("", unlocked)
            ),
            None,
            Some(*at),
        ),
        Scope::Search(_) => unreachable!("handled above"),
        Scope::Album(album) => {
            let owned: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM albums WHERE id = $1 AND owner_id = $2")
                .bind(album)
                .bind(owner)
                .fetch_optional(&mut *tx)
                .await?;
            owned.ok_or(ApiError::NotFound)?;
            (
                "SELECT x.id FROM album_assets aa JOIN assets x ON x.id = aa.asset_id
                 WHERE aa.album_id = $3 AND x.owner_id = $1 AND x.deleted_at IS NULL AND x.primary_id IS NULL
                   AND NOT x.hidden AND x.id <> ALL($2)"
                    .to_string(),
                Some(*album),
                None,
            )
        }
    };
    let q = sqlx::query_scalar(&sql).bind(owner).bind(except);
    let q = match (album, at) {
        (Some(a), _) => q.bind(a),
        (_, Some(t)) => q.bind(t),
        _ => q,
    };
    Ok(q.fetch_all(&mut *tx).await?)
}

fn hidden_clause(alias: &str, unlocked: bool) -> String {
    if unlocked {
        String::new()
    } else {
        let dot = if alias.is_empty() { "" } else { "." };
        format!(" AND NOT {alias}{dot}hidden")
    }
}

/// The photos a filter-only search lists (ownership, not trashed, not a hidden RAW, then the filters), as ids.
async fn resolve_search(
    tx: &mut sqlx::PgConnection,
    owner: Uuid,
    q: &super::search::SearchQuery,
    except: &[Uuid],
) -> ApiResult<Vec<Uuid>> {
    if q.q.as_deref().is_some_and(|t| !t.trim().is_empty()) {
        return Err(ApiError::BadRequest(
            "a text search selects the photos it shows: send their ids".into(),
        ));
    }
    let filters = super::search::Filters::parse(q)?;
    let mut qb = sqlx::QueryBuilder::<sqlx::Postgres>::new("SELECT a.id FROM assets a WHERE");
    filters.push_where(&mut qb, owner);
    qb.push(" AND a.id <> ALL(").push_bind(except.to_vec()).push(")");
    Ok(qb
        .build_query_scalar::<Uuid>()
        .persistent(false)
        .fetch_all(&mut *tx)
        .await?)
}
