//! Tags: global per-user labels on photos (`docs/library.md`, "Tags").
//!
//! A tag is a name owned by one user; a photo has any number of them (`asset_tags`). Names are flat: `/` is allowed in a name
//! (`trip/2024`) so that a hierarchy can be put on top later, but nothing here treats it as one. Two names are the same tag when
//! they are equal ignoring case (a unique index on `(owner_id, lower(name))`); the spelling kept is the first one used.
//!
//! A tag on a photo is `imported` when it came from the photo's own metadata (IPTC Keywords, XMP `dc:subject`) rather than from
//! the owner. A tag the owner adds to a photo that has it as imported becomes the owner's. Keywords are read once per photo
//! (`assets.tags_imported_at`), so a tag the owner removed does not come back when the metadata job runs again.
use anyhow::Result;
use sqlx::PgConnection;
use uuid::Uuid;

/// The longest tag name, in characters.
pub const MAX_NAME_CHARS: usize = 100;
/// Most keywords imported from one file.
pub const MAX_IMPORTED: usize = 50;

/// Trim a tag name, fold runs of white space to one space and refuse what cannot be a tag: empty, over [`MAX_NAME_CHARS`], a
/// control character, or a comma (the separator of a list of tags in an address).
pub fn normalize(name: &str) -> Result<String, String> {
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() {
        return Err("a tag needs a name".into());
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(format!("a tag name is at most {MAX_NAME_CHARS} characters"));
    }
    if name.contains(',') {
        return Err("a tag name cannot contain a comma".into());
    }
    if name.chars().any(char::is_control) {
        return Err("a tag name cannot contain control characters".into());
    }
    Ok(name)
}

/// A keyword read from a file, made into a tag name: commas become spaces (a file's list may use them), the rest as
/// [`normalize`]; too long names are cut. None when nothing usable is left.
pub fn from_keyword(raw: &str) -> Option<String> {
    let cleaned: String = raw
        .chars()
        .map(|c| if c == ',' || c.is_control() { ' ' } else { c })
        .collect();
    let name = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let name: String = name.chars().take(MAX_NAME_CHARS).collect();
    let name = name.trim().to_string();
    (!name.is_empty()).then_some(name)
}

/// The tag with this name (any case) for `owner`, created when it does not exist. Returns its id.
pub async fn ensure(tx: &mut PgConnection, owner: Uuid, name: &str) -> Result<Uuid> {
    // `DO UPDATE` with the same value, so that RETURNING hands back the id of the row that was already there.
    Ok(sqlx::query_scalar(
        "INSERT INTO tags (owner_id, name) VALUES ($1, $2)
         ON CONFLICT (owner_id, lower(name)) DO UPDATE SET name = tags.name RETURNING id",
    )
    .bind(owner)
    .bind(name)
    .fetch_one(tx)
    .await?)
}

/// Attach tags to a photo, as the owner's or as imported ones. An existing link keeps the owner's mark: an imported tag
/// becomes the owner's when the owner adds it, never the other way round. Returns how many links were new.
pub async fn attach(tx: &mut PgConnection, asset: Uuid, tag: Uuid, imported: bool) -> Result<bool> {
    let done = sqlx::query(
        "INSERT INTO asset_tags (asset_id, tag_id, imported) VALUES ($1, $2, $3)
         ON CONFLICT (asset_id, tag_id) DO UPDATE SET imported = asset_tags.imported AND EXCLUDED.imported
         WHERE asset_tags.imported AND NOT EXCLUDED.imported",
    )
    .bind(asset)
    .bind(tag)
    .bind(imported)
    .execute(tx)
    .await?;
    Ok(done.rows_affected() > 0)
}

/// Import the keywords a file carried as tags marked imported, once per photo: nothing happens when the photo has been through
/// this before (`tags_imported_at`), so a tag the owner removed is not brought back. They go on the visible photo of a RAW + JPEG
/// pair. Returns how many tags were attached.
pub async fn import_keywords(db: &sqlx::PgPool, asset: Uuid, keywords: &[String]) -> Result<usize> {
    let mut tx = db.begin().await?;
    let row: Option<(Uuid, Option<Uuid>, Option<time::OffsetDateTime>)> = sqlx::query_as(
        "SELECT owner_id, primary_id, tags_imported_at FROM assets WHERE id = $1 AND purging_at IS NULL FOR UPDATE",
    )
    .bind(asset)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((owner, primary, done)) = row else {
        return Ok(0);
    };
    if done.is_some() {
        return Ok(0);
    }
    let target = primary.unwrap_or(asset);
    let mut seen = std::collections::HashSet::new();
    let mut attached = 0;
    for name in keywords.iter().filter_map(|k| from_keyword(k)) {
        if !seen.insert(name.to_lowercase()) || seen.len() > MAX_IMPORTED {
            continue;
        }
        let tag = ensure(&mut tx, owner, &name).await?;
        if attach(&mut tx, target, tag, true).await? {
            attached += 1;
        }
    }
    sqlx::query("UPDATE assets SET tags_imported_at = now() WHERE id = $1")
        .bind(asset)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(attached)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_trimmed_folded_and_checked() {
        assert_eq!(normalize("  Summer   2024 ").unwrap(), "Summer 2024");
        assert_eq!(
            normalize("trip/2024/Lisbon").unwrap(),
            "trip/2024/Lisbon",
            "a slash is allowed (no hierarchy yet)"
        );
        assert_eq!(normalize("Café ☕").unwrap(), "Café ☕");
        for bad in ["", "   ", "a,b", "tab\u{0}in", "x\u{7}y"] {
            assert!(normalize(bad).is_err(), "{bad:?}");
        }
        assert!(normalize(&"x".repeat(MAX_NAME_CHARS)).is_ok());
        assert!(normalize(&"x".repeat(MAX_NAME_CHARS + 1)).is_err());
        assert!(
            normalize(&"é".repeat(MAX_NAME_CHARS)).is_ok(),
            "counted in characters, not bytes"
        );
    }

    #[test]
    fn keywords_become_names_without_failing() {
        assert_eq!(from_keyword(" beach, sunset ").as_deref(), Some("beach sunset"));
        assert_eq!(from_keyword("a\u{0}b").as_deref(), Some("a b"));
        assert_eq!(from_keyword("   ,, "), None);
        assert_eq!(from_keyword(&"y".repeat(300)).unwrap().chars().count(), MAX_NAME_CHARS);
    }
}
