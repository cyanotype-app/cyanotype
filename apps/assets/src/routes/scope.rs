//! Bulk requests address photos either by explicit `ids` (at most [`MAX_IDS`]) or by a server-side `scope`: "every
//! photo in this view" except an optional `except` list. That is how "Select all" works on a library of 50,000
//! photos without the browser ever listing them.
//!
//! ```json
//! {"scope": "library" | "favorites" | "archive" | "hidden" | "trash" | {"album": "<id>"} | {"person": "<id>"}
//!           | {"pets": "any" | "dog" | "cat"} | {"pet": "<id>"} | {"search": {"from": "2024-01-01", ...}} | {"trash_batch": "<rfc3339>"},
//!   "except": ["<id>"]}
//! ```
//!
//! `person` is the photos of one of the owner's people, as the person's page lists them (live, archived ones too, hidden ones only
//! with the Hidden view unlocked). People is off (`FACES_ENABLED`) or the id is not the owner's: `404`, as for an unknown album. A `search`
//! scope with a `people` filter is refused while People is off, not read as "no filter" (a bulk trash must never widen).
//!
//! `pets` is the photos with a dog, a cat or either found in them, as the Pets view lists them (live, archived ones too, hidden ones only
//! with the Hidden view unlocked). Pets is off (`PETS_ENABLED`): `404`, and so is a `search` scope with a `pets` filter (a bulk trash must
//! never widen); a value that is not `any`, `dog` or `cat` is a `400`.
//!
//! `pet` is the photos of one of the owner's named pets, as the pet's page lists them (live, archived ones too, hidden ones only with the
//! Hidden view unlocked). Pets is off, or the id is not the owner's: `404`. A `search` scope with a `pet` filter is refused while Pets is
//! off, like one with `pets`.
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
use crate::Features;
use cyanotype_core::pets::Which;
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
    /// The photos of one of the owner's people (`FACES_ENABLED`).
    Person(Uuid),
    /// The photos with a dog, a cat or either found in them (`PETS_ENABLED`): `any`, `dog` or `cat`.
    Pets(String),
    /// The photos of one of the owner's named pets (`PETS_ENABLED`).
    Pet(Uuid),
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
    /// The scope, its `except` list, whether this session has the Hidden view unlocked, and which features are on.
    Scope(Scope, Vec<Uuid>, bool, Features),
}

impl Selector {
    pub(super) fn validated(mut self, unlock: Unlock, features: Features) -> ApiResult<Pick> {
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
                // With People off nothing about it exists: a scope that names a person, or filters by people, is a 404.
                let people = match &scope {
                    Scope::Person(_) => true,
                    Scope::Search(q) => q.people.as_deref().is_some_and(|p| !p.trim().is_empty()),
                    _ => false,
                };
                if people && !features.faces {
                    return Err(ApiError::NotFound);
                }
                // Likewise with Pets off; a kind that is not one is the caller's mistake.
                let pets = match &scope {
                    Scope::Pets(w) => {
                        Which::parse(w).ok_or_else(|| ApiError::BadRequest("pets must be any, dog or cat".into()))?;
                        true
                    }
                    Scope::Pet(_) => true,
                    Scope::Search(q) => {
                        q.pets.as_deref().is_some_and(|p| !p.trim().is_empty())
                            || q.pet.as_deref().is_some_and(|p| !p.trim().is_empty())
                    }
                    _ => false,
                };
                if pets && !features.pets {
                    return Err(ApiError::NotFound);
                }
                Ok(Pick::Scope(scope, self.except, unlock.0, features))
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
            Pick::Scope(scope, except, unlocked, features) => {
                Ok(Some(resolve(tx, owner, scope, except, *unlocked, *features).await?))
            }
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
    features: Features,
) -> ApiResult<Vec<Uuid>> {
    if let Scope::Search(q) = scope {
        return resolve_search(tx, owner, q, except, features).await;
    }
    if let Scope::Person(person) = scope {
        return resolve_person(tx, owner, *person, except, unlocked, features.faces).await;
    }
    if let Scope::Pets(which) = scope {
        return resolve_pets(tx, owner, which, except, unlocked, features.pets).await;
    }
    if let Scope::Pet(pet) = scope {
        return resolve_pet(tx, owner, *pet, except, unlocked, features.pets).await;
    }
    // A smart album has no members: its photos are what its rules match now (never a hidden one: `push_where`).
    if let Scope::Album(album) = scope {
        let rules: Option<(Option<serde_json::Value>,)> =
            sqlx::query_as("SELECT rules FROM albums WHERE id = $1 AND owner_id = $2")
                .bind(album)
                .bind(owner)
                .fetch_optional(&mut *tx)
                .await?;
        if let Some(rules) = rules.ok_or(ApiError::NotFound)?.0 {
            let filters = super::smart::Rules::from_json(&rules)?.filters(features);
            let mut qb = sqlx::QueryBuilder::<sqlx::Postgres>::new("SELECT a.id FROM assets a WHERE");
            filters.push_where(&mut qb, owner);
            qb.push(" AND a.id <> ALL(").push_bind(except.to_vec()).push(")");
            return Ok(qb
                .build_query_scalar::<Uuid>()
                .persistent(false)
                .fetch_all(&mut *tx)
                .await?);
        }
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
        Scope::Search(_) | Scope::Person(_) | Scope::Pets(_) | Scope::Pet(_) => unreachable!("handled above"),
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

/// The photos of one of the owner's people, as the person's page lists them.
async fn resolve_person(
    tx: &mut sqlx::PgConnection,
    owner: Uuid,
    person: Uuid,
    except: &[Uuid],
    unlocked: bool,
    faces: bool,
) -> ApiResult<Vec<Uuid>> {
    if !faces {
        return Err(ApiError::NotFound);
    }
    let owned: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM people WHERE id = $1 AND owner_id = $2")
        .bind(person)
        .bind(owner)
        .fetch_optional(&mut *tx)
        .await?;
    owned.ok_or(ApiError::NotFound)?;
    Ok(sqlx::query_scalar(&format!(
        "SELECT x.id FROM assets x
         WHERE x.owner_id = $1 AND {} AND x.id <> ALL($2)
           AND EXISTS (SELECT 1 FROM faces f WHERE f.asset_id = x.id AND f.person_id = $3 AND {})",
        super::people::seen("x", "$4"),
        super::people::SCANNED
    ))
    .bind(owner)
    .bind(except)
    .bind(person)
    .bind(unlocked)
    .fetch_all(&mut *tx)
    .await?)
}

/// The photos of one of the owner's named pets, as the pet's page lists them.
async fn resolve_pet(
    tx: &mut sqlx::PgConnection,
    owner: Uuid,
    pet: Uuid,
    except: &[Uuid],
    unlocked: bool,
    pets: bool,
) -> ApiResult<Vec<Uuid>> {
    if !pets {
        return Err(ApiError::NotFound);
    }
    let owned: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM pets WHERE id = $1 AND owner_id = $2")
        .bind(pet)
        .bind(owner)
        .fetch_optional(&mut *tx)
        .await?;
    owned.ok_or(ApiError::NotFound)?;
    Ok(sqlx::query_scalar(&format!(
        "SELECT x.id FROM assets x
         WHERE x.owner_id = $1 AND {} AND x.id <> ALL($2)
           AND EXISTS (SELECT 1 FROM pet_detections d WHERE d.asset_id = x.id AND d.pet_id = $3 AND {})",
        super::people::seen("x", "$4"),
        cyanotype_core::pets::SCANNED
    ))
    .bind(owner)
    .bind(except)
    .bind(pet)
    .bind(unlocked)
    .fetch_all(&mut *tx)
    .await?)
}

/// The photos with a dog, a cat or either found in them, as the Pets view lists them.
async fn resolve_pets(
    tx: &mut sqlx::PgConnection,
    owner: Uuid,
    which: &str,
    except: &[Uuid],
    unlocked: bool,
    pets: bool,
) -> ApiResult<Vec<Uuid>> {
    let which = Which::parse(which).ok_or_else(|| ApiError::BadRequest("pets must be any, dog or cat".into()))?;
    if !pets {
        return Err(ApiError::NotFound);
    }
    Ok(sqlx::query_scalar(&format!(
        "SELECT x.id FROM assets x
         WHERE x.owner_id = $1 AND {} AND x.id <> ALL($2) AND {}",
        super::people::seen("x", "$3"),
        super::pets::with_pets("x", "$4"),
    ))
    .bind(owner)
    .bind(except)
    .bind(unlocked)
    .bind(which.kind())
    .fetch_all(&mut *tx)
    .await?)
}

/// The photos a filter-only search lists (ownership, not trashed, not a hidden RAW, then the filters), as ids.
async fn resolve_search(
    tx: &mut sqlx::PgConnection,
    owner: Uuid,
    q: &super::search::SearchQuery,
    except: &[Uuid],
    features: Features,
) -> ApiResult<Vec<Uuid>> {
    if q.q.as_deref().is_some_and(|t| !t.trim().is_empty()) {
        return Err(ApiError::BadRequest(
            "a text search selects the photos it shows: send their ids".into(),
        ));
    }
    if !features.faces && q.people.as_deref().is_some_and(|p| !p.trim().is_empty()) {
        return Err(ApiError::NotFound);
    }
    if !features.pets
        && (q.pets.as_deref().is_some_and(|p| !p.trim().is_empty())
            || q.pet.as_deref().is_some_and(|p| !p.trim().is_empty()))
    {
        return Err(ApiError::NotFound);
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
