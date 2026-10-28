//! Named pets (docs/pets.md, "Named pets"): proposing which animals found in the owner's photos are a pet the owner has tagged a few photos
//! of. Nothing here assigns anything: a detection joins a pet only when the owner says so (`routes/named_pets.rs`); this module reads the
//! fingerprints (`pet_detections.embedding`, made by the worker from the crop of each animal) and ranks the animals that are still free.
//!
//! The rule and the thresholds were measured on the evaluation set of `cyanotype-pets/tests/individuals.rs` (ignored; docs/pets.md): an
//! animal is scored by its cosine similarity to the **centre** of the pet's confirmed animals' fingerprints, and shown in a tier by the
//! score. Every answer counts the next time: a confirmed animal moves the centre, a refused one never comes back for that pet.
use std::collections::HashMap;

use anyhow::Result;
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    embeddings::pg_literal,
    people_math as pm,
    pets::{EMBED_DIM, EMBED_MODEL, SCANNED},
};

/// Scores at least this are **likely** the pet: shown first and offered to confirm all at once.
pub const LIKELY: f32 = 0.72;
/// Scores at least this are **maybe** the pet: shown after, to be looked at one by one. Below it nothing is proposed.
pub const MAYBE: f32 = 0.55;
/// The most proposals one list returns and one pending count reaches: the best ones, nearest to the pet's centre first.
pub const CONSIDERED: i64 = 300;

/// How sure a proposal is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Likely,
    Maybe,
}

impl Tier {
    pub fn name(self) -> &'static str {
        match self {
            Tier::Likely => "likely",
            Tier::Maybe => "maybe",
        }
    }

    pub fn parse(s: &str) -> Option<Tier> {
        match s.trim().to_ascii_lowercase().as_str() {
            "likely" => Some(Tier::Likely),
            "maybe" => Some(Tier::Maybe),
            _ => None,
        }
    }
}

/// The tier a score falls in, or `None` below [`MAYBE`].
pub fn tier_of(score: f32) -> Option<Tier> {
    if score >= LIKELY {
        Some(Tier::Likely)
    } else if score >= MAYBE {
        Some(Tier::Maybe)
    } else {
        None
    }
}

/// An animal that may be the pet ("Is this Biscuit?").
#[derive(Debug, Clone, PartialEq)]
pub struct Suggestion {
    pub detection_id: Uuid,
    pub asset_id: Uuid,
    /// What the detector (or the owner) says the animal is; a pet of the other kind takes it over when the owner confirms.
    pub kind: String,
    pub bbox: [f32; 4],
    /// Cosine similarity to the centre of the pet's confirmed animals.
    pub similarity: f32,
    pub tier: Tier,
}

/// One row of the proposals query: the animal, its photo, its kind, its box and its similarity to the centre.
type Proposal = (Uuid, Uuid, String, f32, f32, f32, f32, f64);

/// The centre (unit length) of the fingerprints of the animals in `pet`, or `None` while it has none (a new pet whose photos have not
/// been fingerprinted yet, or one whose only animals were too small).
pub async fn centre(db: &PgPool, owner: Uuid, pet: Uuid) -> Result<Option<Vec<f32>>> {
    let row: Option<(Option<String>,)> = sqlx::query_as(&format!(
        "SELECT avg(d.embedding::vector({EMBED_DIM}))::text FROM pet_detections d
         WHERE d.pet_id = $1 AND d.owner_id = $2 AND d.embed_model = $3 AND d.embedding IS NOT NULL AND {SCANNED}"
    ))
    .bind(pet)
    .bind(owner)
    .bind(EMBED_MODEL)
    .fetch_optional(db)
    .await?;
    Ok(row
        .and_then(|r| r.0)
        .and_then(|t| pm::parse_literal(&t))
        .and_then(|v| pm::centre([v.as_slice()])))
}

/// The free animals closest to the centre of `pet`'s animals, of either kind (the detector may call a dog a cat; a confirmed animal takes
/// the pet's kind, and no animal of the other kind came near a pet's centre on the evaluation set: docs/pets.md), at least [`MAYBE`] similar, not refused for it, in live
/// photos (hidden ones only with `unlocked`), best first, at most `limit`. Empty for a pet with no
/// fingerprinted animal.
pub async fn suggestions(db: &PgPool, owner: Uuid, pet: Uuid, unlocked: bool, limit: i64) -> Result<Vec<Suggestion>> {
    let Some(centre) = centre(db, owner, pet).await? else {
        return Ok(Vec::new());
    };
    // One pass over the owner's free animals (`pet_detections_open_idx`): the distance to the centre, kept when it is close enough.
    let sql = format!(
        "SELECT d.id, d.asset_id, d.kind, d.box_x, d.box_y, d.box_w, d.box_h, 1 - (d.embedding::halfvec({EMBED_DIM}) <=> $1::text::halfvec({EMBED_DIM})) AS sim
         FROM pet_detections d JOIN assets a ON a.id = d.asset_id
         WHERE d.owner_id = $2 AND d.pet_id IS NULL AND d.embed_model = $3 AND d.embedding IS NOT NULL
           AND d.embedding::halfvec({EMBED_DIM}) <=> $1::text::halfvec({EMBED_DIM}) <= $4
           AND a.deleted_at IS NULL AND a.purging_at IS NULL AND a.primary_id IS NULL AND (NOT a.hidden OR $5)
           AND {SCANNED}
           AND NOT EXISTS (SELECT 1 FROM pet_rejections r WHERE r.detection_id = d.id AND r.pet_id = $6)
         ORDER BY sim DESC, d.id LIMIT $7"
    );
    let rows: Vec<Proposal> = sqlx::query_as(&sql)
        .bind(pg_literal(&centre))
        .bind(owner)
        .bind(EMBED_MODEL)
        .bind(f64::from(1.0 - MAYBE))
        .bind(unlocked)
        .bind(pet)
        .bind(limit)
        .persistent(false)
        .fetch_all(db)
        .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(detection_id, asset_id, kind, x, y, w, h, s)| {
            let similarity = s as f32;
            Some(Suggestion {
                detection_id,
                asset_id,
                kind,
                bbox: [x, y, w, h],
                similarity,
                tier: tier_of(similarity)?,
            })
        })
        .collect())
}

/// How many proposals wait for each named, shown pet of `owner` (`only` one of them): the animals [`suggestions`] would list, counted
/// in one statement for all of them (the centre of each pet's animals is an `avg`, and each is compared with the owner's free animals), so the pets list costs one query and not one per pet. Pets without a pending proposal are left out.
pub async fn pending_counts(
    db: &PgPool,
    owner: Uuid,
    only: Option<Uuid>,
    unlocked: bool,
) -> Result<HashMap<Uuid, i64>> {
    let sql = format!(
        "WITH centres AS MATERIALIZED (
             SELECT p.id AS pet_id, avg(d.embedding::vector({EMBED_DIM})) AS c
             FROM pets p JOIN pet_detections d ON d.pet_id = p.id
             WHERE p.owner_id = $1 AND NOT p.hidden AND ($5::uuid IS NULL OR p.id = $5)
               AND d.embed_model = $2 AND d.embedding IS NOT NULL AND {SCANNED}
             GROUP BY p.id),
         open AS MATERIALIZED (
             SELECT d.id, d.asset_id, d.embedding::vector({EMBED_DIM}) AS e FROM pet_detections d
             WHERE d.owner_id = $1 AND d.pet_id IS NULL AND d.embed_model = $2 AND d.embedding IS NOT NULL),
         near AS MATERIALIZED (
             SELECT c.pet_id, o.id, o.asset_id FROM centres c JOIN open o ON true WHERE o.e <=> c.c <= $3)
         SELECT n.pet_id, least(count(*), {CONSIDERED}) FROM near n
         JOIN pet_detections d ON d.id = n.id JOIN assets a ON a.id = n.asset_id
         WHERE a.deleted_at IS NULL AND a.purging_at IS NULL AND a.primary_id IS NULL AND (NOT a.hidden OR $4)
           AND {SCANNED}
           AND NOT EXISTS (SELECT 1 FROM pet_rejections r WHERE r.detection_id = n.id AND r.pet_id = n.pet_id)
         GROUP BY n.pet_id"
    );
    let rows: Vec<(Uuid, i64)> = sqlx::query_as(&sql)
        .bind(owner)
        .bind(EMBED_MODEL)
        .bind(f64::from(1.0 - MAYBE))
        .bind(unlocked)
        .bind(only)
        .persistent(false)
        .fetch_all(db)
        .await?;
    Ok(rows.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_score_falls_in_a_tier_or_in_none() {
        assert_eq!(tier_of(1.0), Some(Tier::Likely));
        assert_eq!(tier_of(LIKELY), Some(Tier::Likely));
        assert_eq!(tier_of(LIKELY - 0.001), Some(Tier::Maybe));
        assert_eq!(tier_of(MAYBE), Some(Tier::Maybe));
        assert_eq!(tier_of(MAYBE - 0.001), None);
        assert_eq!(tier_of(-1.0), None);
        assert_eq!(
            (Tier::parse(" Likely "), Tier::parse("MAYBE"), Tier::parse("sure")),
            (Some(Tier::Likely), Some(Tier::Maybe), None)
        );
        assert_eq!((Tier::Likely.name(), Tier::Maybe.name()), ("likely", "maybe"));
    }
}
