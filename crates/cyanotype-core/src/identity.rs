//! The account manifest: `manifests/accounts/accounts-YYYYMMDD.jsonl`, the part of the account state a bucket-only rebuild
//! (`cyanotype reindex` on an empty database, `docs/recovery.md` path B) needs and the photos cannot carry.
//!
//! What it holds, per account: the id (the photos' keys name their owner by it), the email, the role, when it was created,
//! whether it is disabled and its three limits; plus the server's default limits. What it deliberately does **not** hold:
//!
//! * **password hashes.** A rebuilt account gets a *locked* password (a hash of a random value nobody knows), and its owner
//!   chooses a new one with a reset link (`cyanotype reset-link --email ...`, the same links the admin portal hands out).
//!   Why: the bucket holds every photo already, so reading it is the worst case anyway, but a hash in it would let
//!   whoever gets a copy of the bucket (a leaked key, a backup of it, a bucket-listing tool) crack the owner's password
//!   offline, and that password is probably used elsewhere. A reset link costs the owner one command during a disaster;
//!   a cracked reused password costs far more. The nightly Postgres dump (path A) does carry the hashes, and it is the
//!   dump, not the bucket, that is expected to be the better-protected copy.
//! * API keys and sessions: credentials. After a rebuild everyone signs in again and creates new keys (`cyanotype login`).
//! * invites, password reset links, admin tasks, the audit log, usage history (`upload_log`), the admin's other server
//!   state: ephemeral or an operational record, not needed to get the library back. The audit log in particular is a
//!   log of *who did what*, can grow without bound and belongs in the dump; it is lost on path B and says so in the docs.
//!
//! Dated files, like the per-user manifests; `reindex` reads only the **newest** one (a deleted account must not come
//! back from an older file). `MANIFEST_KEEP` files are kept. Written by the same daily job as the per-user manifests and by
//! `cyanotype manifest`.
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{PgPool, Row};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{keys, manifest::today_utc, storage::Storage};

pub const FORMAT_VERSION: u32 = 1;
const MAX_BYTES: u64 = 64 << 20;
/// `server_state` entries that are settings an admin made (not heartbeats or job results) and so come back with a rebuild.
pub const SETTINGS: &[&str] = &["default_limits"];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Line {
    Meta(MetaLine),
    User(UserLine),
    Setting(SettingLine),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetaLine {
    pub v: u32,
    #[serde(with = "time::serde::rfc3339")]
    pub generated_at: OffsetDateTime,
    pub users: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserLine {
    pub id: Uuid,
    pub email: String,
    /// `admin` or `member`; anything else reads as `member`.
    pub role: String,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub created_at: Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub disabled_at: Option<OffsetDateTime>,
    #[serde(default)]
    pub quota_bytes: Option<i64>,
    #[serde(default)]
    pub daily_upload_bytes: Option<i64>,
    #[serde(default)]
    pub max_file_bytes: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SettingLine {
    pub key: String,
    pub value: Value,
}

/// The newest account manifest, parsed.
#[derive(Debug, Default)]
pub struct Roster {
    pub key: String,
    pub users: Vec<UserLine>,
    pub settings: Vec<SettingLine>,
    pub bad_lines: usize,
}

/// Build the account manifest from the database. Returns the JSONL bytes and the number of accounts.
pub async fn build(db: &PgPool) -> Result<(Vec<u8>, u64)> {
    let rows = sqlx::query(
        "SELECT id, email, role, created_at, disabled_at, quota_bytes, daily_upload_bytes, max_file_bytes
         FROM users ORDER BY created_at, id",
    )
    .fetch_all(db)
    .await
    .context("read users")?;
    let mut body = Vec::new();
    for r in &rows {
        body.push(Line::User(UserLine {
            id: r.try_get("id")?,
            email: r.try_get("email")?,
            role: r.try_get("role")?,
            created_at: r.try_get("created_at")?,
            disabled_at: r.try_get("disabled_at")?,
            quota_bytes: r.try_get("quota_bytes")?,
            daily_upload_bytes: r.try_get("daily_upload_bytes")?,
            max_file_bytes: r.try_get("max_file_bytes")?,
        }));
    }
    let settings: Vec<(String, Value)> =
        sqlx::query_as("SELECT key, value FROM server_state WHERE key = ANY($1) ORDER BY key")
            .bind(SETTINGS)
            .fetch_all(db)
            .await
            .context("read settings")?;
    for (key, value) in settings {
        body.push(Line::Setting(SettingLine { key, value }));
    }
    let mut out = Vec::new();
    let header = Line::Meta(MetaLine {
        v: FORMAT_VERSION,
        generated_at: OffsetDateTime::now_utc(),
        users: rows.len() as u64,
    });
    for line in std::iter::once(&header).chain(body.iter()) {
        serde_json::to_writer(&mut out, line)?;
        out.push(b'\n');
    }
    Ok((out, rows.len() as u64))
}

/// Unparseable lines (and line types a future version adds) are counted, not fatal.
pub fn parse(bytes: &[u8]) -> (Vec<Line>, usize) {
    let (mut lines, mut bad) = (Vec::new(), 0);
    for raw in bytes
        .split(|b| *b == b'\n')
        .filter(|l| !l.iter().all(u8::is_ascii_whitespace))
    {
        match serde_json::from_slice::<Line>(raw) {
            Ok(l) => lines.push(l),
            Err(_) => bad += 1,
        }
    }
    (lines, bad)
}

/// Every account manifest key, newest first.
pub async fn list(storage: &Storage) -> Result<Vec<String>> {
    let mut found = Vec::new();
    let mut token = None;
    loop {
        let page = storage.list_page(keys::ACCOUNTS_MANIFEST_PREFIX, token).await?;
        found.extend(
            page.objects
                .into_iter()
                .map(|o| o.key)
                .filter(|k| accounts_key_date(k).is_some()),
        );
        match page.next {
            Some(t) => token = Some(t),
            None => break,
        }
    }
    found.sort_by(|a, b| b.cmp(a));
    Ok(found)
}

fn accounts_key_date(key: &str) -> Option<&str> {
    let d = key
        .strip_prefix(keys::ACCOUNTS_MANIFEST_PREFIX)?
        .strip_prefix("accounts-")?
        .strip_suffix(".jsonl")?;
    (d.len() == 8 && d.bytes().all(|b| b.is_ascii_digit())).then_some(d)
}

/// The newest account manifest, if the bucket has one.
pub async fn load_newest(storage: &Storage) -> Result<Option<Roster>> {
    let Some(key) = list(storage).await?.into_iter().next() else {
        return Ok(None);
    };
    let bytes = storage.get_bytes_limited(&key, MAX_BYTES).await?;
    let (lines, bad_lines) = parse(&bytes);
    let mut a = Roster {
        key,
        bad_lines,
        ..Default::default()
    };
    for l in lines {
        match l {
            Line::User(u) => a.users.push(u),
            Line::Setting(s) if SETTINGS.contains(&s.key.as_str()) => a.settings.push(s),
            _ => {}
        }
    }
    Ok(Some(a))
}

/// Write today's account manifest and prune to the newest `keep`. Returns the number of accounts written.
pub async fn write(db: &PgPool, storage: &Storage, keep: usize) -> Result<u64> {
    let (bytes, n) = build(db).await?;
    // An empty database (just restored, or just created, before `reindex` ran) has nothing to protect and must not push
    // the real accounts out of the newest slot.
    if n == 0 {
        return Ok(0);
    }
    storage
        .put_bytes(&keys::accounts_manifest(&today_utc()), bytes, "application/x-ndjson")
        .await?;
    let existing = list(storage).await?;
    if existing.len() > keep {
        storage.delete_all_versions(&existing[keep..]).await?;
    }
    Ok(n)
}

/// True if today's account manifest is already there.
pub async fn written_today(storage: &Storage) -> Result<bool> {
    let today = today_utc();
    Ok(list(storage)
        .await?
        .iter()
        .any(|k| accounts_key_date(k) == Some(today.as_str())))
}

/// A password hash nobody can log in with: argon2id of a random value that is thrown away. Used for accounts rebuilt
/// from the bucket (their owners choose a password with a reset link) and for password-protected share links whose
/// password the bucket does not carry (the link stays closed until its owner sets a new one).
pub fn locked_password_hash() -> Result<String> {
    use argon2::{
        Argon2, PasswordHasher,
        password_hash::{SaltString, rand_core::OsRng},
    };
    let secret = SaltString::generate(&mut OsRng);
    let salt = SaltString::generate(&mut OsRng);
    Ok(Argon2::default()
        .hash_password(secret.as_str().as_bytes(), &salt)
        .map_err(|e| anyhow::anyhow!("hash: {e}"))?
        .to_string())
}

#[derive(Debug, Default)]
pub struct RestoreOutcome {
    /// Users created (or that would be, in a dry run).
    pub restored: Vec<Uuid>,
    /// Users that already exist (same id).
    pub present: u64,
    /// The email is taken by an account with another id (e.g. someone registered again before the reindex): left alone.
    pub conflicts: Vec<Uuid>,
    pub settings_restored: u64,
}

/// Recreate the accounts (and settings) the database does not have. Nothing existing is changed. With `apply` false it only
/// reports. Restored accounts have a locked password.
pub async fn restore(db: &PgPool, roster: &Roster, apply: bool) -> Result<RestoreOutcome> {
    let mut out = RestoreOutcome::default();
    for u in &roster.users {
        let by_id: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE id = $1)")
            .bind(u.id)
            .fetch_one(db)
            .await?;
        if by_id {
            out.present += 1;
            continue;
        }
        let email_taken: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE lower(email) = lower($1))")
                .bind(&u.email)
                .fetch_one(db)
                .await?;
        if email_taken {
            out.conflicts.push(u.id);
            continue;
        }
        if apply {
            let role = if u.role == "admin" { "admin" } else { "member" };
            let hash = locked_password_hash()?;
            let done = sqlx::query(
                "INSERT INTO users (id, email, password_hash, role, created_at, disabled_at, quota_bytes,
                                    daily_upload_bytes, max_file_bytes)
                 VALUES ($1, $2, $3, $4, coalesce($5, now()), $6, $7, $8, $9) ON CONFLICT DO NOTHING",
            )
            .bind(u.id)
            .bind(&u.email)
            .bind(hash)
            .bind(role)
            .bind(u.created_at)
            .bind(u.disabled_at)
            .bind(u.quota_bytes.filter(|n| *n > 0))
            .bind(u.daily_upload_bytes.filter(|n| *n > 0))
            .bind(u.max_file_bytes.filter(|n| *n > 0))
            .execute(db)
            .await?
            .rows_affected();
            if done == 0 {
                out.conflicts.push(u.id);
                continue;
            }
        }
        out.restored.push(u.id);
    }
    for s in &roster.settings {
        if apply {
            let done =
                sqlx::query("INSERT INTO server_state (key, value) VALUES ($1, $2) ON CONFLICT (key) DO NOTHING")
                    .bind(&s.key)
                    .bind(&s.value)
                    .execute(db)
                    .await?
                    .rows_affected();
            out.settings_restored += done;
        } else {
            let exists: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM server_state WHERE key = $1)")
                .bind(&s.key)
                .fetch_one(db)
                .await?;
            out.settings_restored += u64::from(!exists);
        }
    }
    Ok(out)
}

/// A password reset link made without the admin portal (`cyanotype reset-link`): for the owner of a rebuilt database, who
/// cannot sign in to ask an admin for one. Same table, same one-time semantics as the portal's links; earlier unused links
/// of the account stop working. The token is returned once; only its hash is stored.
#[derive(Debug)]
pub struct ResetLink {
    pub user_id: Uuid,
    pub email: String,
    pub token: String,
    pub expires_at: OffsetDateTime,
}

pub async fn create_reset_link(db: &PgPool, email: &str, hours: i32) -> Result<ResetLink> {
    use rand::RngCore;
    use sha2::{Digest, Sha256};
    anyhow::ensure!((1..=24 * 14).contains(&hours), "hours must be between 1 and 336");
    let mut tx = db.begin().await?;
    let user: Option<(Uuid, String)> = sqlx::query_as("SELECT id, email FROM users WHERE lower(email) = lower($1)")
        .bind(email.trim())
        .fetch_optional(&mut *tx)
        .await?;
    let Some((user_id, email)) = user else {
        anyhow::bail!("no account with that email");
    };
    sqlx::query(
        "UPDATE password_resets SET expires_at = now() WHERE user_id = $1 AND used_at IS NULL AND expires_at > now()",
    )
    .bind(user_id)
    .execute(&mut *tx)
    .await?;
    let mut raw = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut raw);
    let token = keys::hex(&raw);
    let hash: Vec<u8> = Sha256::digest(token.as_bytes()).to_vec();
    let (expires_at,): (OffsetDateTime,) = sqlx::query_as(
        "INSERT INTO password_resets (user_id, token_hash, created_by, expires_at)
         VALUES ($1, $2, NULL, now() + make_interval(hours => $3)) RETURNING expires_at",
    )
    .bind(user_id)
    .bind(hash)
    .bind(hours)
    .fetch_one(&mut *tx)
    .await?;
    crate::audit::record(
        &mut *tx,
        None,
        "account.reset_link",
        Some("user"),
        Some(&user_id.to_string()),
        serde_json::json!({"email": email, "by": "cli"}),
    )
    .await?;
    tx.commit().await?;
    Ok(ResetLink {
        user_id,
        email,
        token,
        expires_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::key_date;

    #[test]
    fn key_dates() {
        assert_eq!(
            accounts_key_date(&keys::accounts_manifest("20260131")),
            Some("20260131")
        );
        assert_eq!(accounts_key_date("manifests/accounts/accounts-2026013.jsonl"), None);
        // A per-user manifest directory is never an accounts file, and the other way round.
        assert_eq!(accounts_key_date(&keys::manifest(Uuid::nil(), "20260131")), None);
        assert_eq!(key_date(&keys::accounts_manifest("20260131")), None);
    }

    #[test]
    fn lines_round_trip_and_tolerate_noise() {
        let u = UserLine {
            id: Uuid::new_v4(),
            email: "a@example.com".into(),
            role: "admin".into(),
            created_at: Some(time::macros::datetime!(2026-01-02 03:04:05 UTC)),
            disabled_at: None,
            quota_bytes: Some(5),
            daily_upload_bytes: None,
            max_file_bytes: None,
        };
        let mut buf = serde_json::to_vec(&Line::User(u.clone())).unwrap();
        buf.extend_from_slice(b"\n\nnot json\n{\"t\":\"from_the_future\"}\n");
        buf.extend_from_slice(br#"{"t":"setting","key":"default_limits","value":{"quota_bytes":9}}"#);
        let (lines, bad) = parse(&buf);
        assert_eq!(bad, 2);
        assert!(matches!(&lines[0], Line::User(b) if *b == u));
        assert!(matches!(&lines[1], Line::Setting(s) if s.key == "default_limits"));
    }

    /// The account manifest never carries a credential: the line types have no field for one.
    #[test]
    fn user_lines_carry_no_credentials() {
        let json = serde_json::to_string(&Line::User(UserLine {
            id: Uuid::nil(),
            email: "a@example.com".into(),
            role: "member".into(),
            created_at: None,
            disabled_at: None,
            quota_bytes: None,
            daily_upload_bytes: None,
            max_file_bytes: None,
        }))
        .unwrap();
        for forbidden in ["password", "hash", "token", "key_hash", "argon"] {
            assert!(!json.contains(forbidden), "{forbidden} in {json}");
        }
    }

    #[test]
    fn locked_hashes_are_valid_and_unique() {
        use argon2::{Argon2, PasswordHash, PasswordVerifier};
        let (a, b) = (locked_password_hash().unwrap(), locked_password_hash().unwrap());
        assert_ne!(a, b);
        assert!(a.starts_with("$argon2id$"));
        let parsed = PasswordHash::new(&a).unwrap();
        assert!(Argon2::default().verify_password(b"", &parsed).is_err());
        assert!(Argon2::default().verify_password(b"password", &parsed).is_err());
    }
}
