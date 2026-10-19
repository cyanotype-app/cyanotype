//! Per-account limits (docs/admin.md, "Limits"): a storage quota, a daily upload cap and a largest file, each optional.
//!
//! The check runs when an upload is created (`check_new_upload`), with the account's uploads serialised by a lock so
//! concurrent uploads cannot each pass while together exceeding a limit. What is declared but not yet finished counts
//! against the account (`uploads` rows, a reservation), so the sum of what concurrent uploads may add is bounded too.
//! The size an upload may have is then pinned by the part layout (the proxy checks every part, direct uploads sign the
//! part length and the assets service lists the parts before completing).
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use sqlx::{PgConnection, PgPool};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

/// The cap's window: any 24 hours, rolling.
pub const WINDOW_HOURS: i64 = 24;

/// How long an unfinished upload keeps its reservation without a sign of life (a part arriving or a resume refreshes it).
pub const RESERVATION_HOURS: i64 = 24;

/// One account's limits. `None` is unlimited.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct Limits {
    pub quota_bytes: Option<i64>,
    pub daily_upload_bytes: Option<i64>,
    pub max_file_bytes: Option<i64>,
}

impl Limits {
    pub fn is_unlimited(&self) -> bool {
        *self == Limits::default()
    }

    /// Every set value must be positive (zero would lock the account out; "no limit" is `None`).
    pub fn validate(&self) -> Result<()> {
        for (name, v) in [
            ("quota_bytes", self.quota_bytes),
            ("daily_upload_bytes", self.daily_upload_bytes),
            ("max_file_bytes", self.max_file_bytes),
        ] {
            if v.is_some_and(|n| n <= 0) {
                bail!("{name} must be greater than zero (or empty for no limit)");
            }
        }
        Ok(())
    }

    /// This limit set, with each unset field taken from `fallback` (an invite's presets in front of the server default).
    pub fn or(self, fallback: Limits) -> Limits {
        Limits {
            quota_bytes: self.quota_bytes.or(fallback.quota_bytes),
            daily_upload_bytes: self.daily_upload_bytes.or(fallback.daily_upload_bytes),
            max_file_bytes: self.max_file_bytes.or(fallback.max_file_bytes),
        }
    }
}

pub async fn of(db: &PgPool, user: Uuid) -> Result<Limits> {
    Ok(
        sqlx::query_as("SELECT quota_bytes, daily_upload_bytes, max_file_bytes FROM users WHERE id = $1")
            .bind(user)
            .fetch_optional(db)
            .await?
            .unwrap_or_default(),
    )
}

/// Sets an account's limits; false if there is no such account.
pub async fn set(db: &PgPool, user: Uuid, l: Limits) -> Result<bool> {
    l.validate()?;
    let n =
        sqlx::query("UPDATE users SET quota_bytes = $2, daily_upload_bytes = $3, max_file_bytes = $4 WHERE id = $1")
            .bind(user)
            .bind(l.quota_bytes)
            .bind(l.daily_upload_bytes)
            .bind(l.max_file_bytes)
            .execute(db)
            .await?
            .rows_affected();
    Ok(n == 1)
}

const DEFAULT_KEY: &str = "default_limits";

/// The limits new accounts start with (unlimited until an admin sets some).
pub async fn server_default(db: impl sqlx::PgExecutor<'_>) -> Result<Limits> {
    let v: Option<serde_json::Value> = sqlx::query_scalar("SELECT value FROM server_state WHERE key = $1")
        .bind(DEFAULT_KEY)
        .fetch_optional(db)
        .await?;
    Ok(v.and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default())
}

pub async fn set_server_default(db: &PgPool, l: Limits) -> Result<()> {
    l.validate()?;
    sqlx::query(
        "INSERT INTO server_state (key, value) VALUES ($1, $2)
         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = now()",
    )
    .bind(DEFAULT_KEY)
    .bind(serde_json::to_value(l)?)
    .execute(db)
    .await?;
    Ok(())
}

/// Why an upload may not start. The caller turns it into a response (413 or 429).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Larger than the account's largest file.
    FileTooLarge { size: i64, max_bytes: i64 },
    /// Larger than the whole daily cap: it could never fit, however long the account waits.
    FileExceedsDailyCap { size: i64, cap: i64 },
    /// Storing it would pass the quota.
    QuotaExceeded {
        size: i64,
        quota_bytes: i64,
        stored_bytes: i64,
        /// Declared by this account's other unfinished uploads.
        reserved_bytes: i64,
    },
    /// The last 24 hours of uploads leave too little of the cap; `retry_at` is when enough has rolled out of the window.
    DailyCap {
        size: i64,
        cap: i64,
        used_bytes: i64,
        retry_at: OffsetDateTime,
    },
}

/// The outcome of fitting `size` into the rolling window. `events` are `(when, bytes)` inside the window, oldest first.
pub fn fit_daily(
    events: &[(OffsetDateTime, i64)],
    cap: i64,
    size: i64,
) -> std::result::Result<(), (i64, Option<OffsetDateTime>)> {
    let used: i64 = events.iter().map(|e| e.1).sum();
    if used + size <= cap {
        return Ok(());
    }
    if size > cap {
        return Err((used, None));
    }
    let need = used + size - cap;
    let mut freed = 0;
    for (at, bytes) in events {
        freed += bytes;
        if freed >= need {
            return Err((used, Some(*at + Duration::hours(WINDOW_HOURS))));
        }
    }
    // Unreachable while size <= cap (everything freed leaves `size` alone), but never panic on arithmetic.
    Err((used, events.last().map(|e| e.0 + Duration::hours(WINDOW_HOURS))))
}

/// Serialises upload creation of one account across all service replicas (released at commit or rollback).
pub async fn lock_account_uploads(tx: &mut PgConnection, user: Uuid) -> Result<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('upload-limits:' || $1::text, 0))")
        .bind(user)
        .execute(tx)
        .await?;
    Ok(())
}

/// What counts in the rolling window: completed uploads in the last 24 hours and the unfinished uploads of the account
/// (declared sizes, at their last sign of life), except one for `except_sha` (the upload being resumed or recreated).
pub async fn window_events(
    conn: &mut PgConnection,
    user: Uuid,
    except_sha: Option<&[u8]>,
) -> Result<Vec<(OffsetDateTime, i64)>> {
    Ok(sqlx::query_as(
        "SELECT at, bytes FROM (
             SELECT at, bytes FROM upload_log WHERE owner_id = $1 AND at > now() - make_interval(hours => $3)
             UNION ALL
             SELECT created_at, size_bytes FROM uploads
              WHERE owner_id = $1 AND created_at > now() - make_interval(hours => $4)
                AND ($2::bytea IS NULL OR sha256 <> $2)
         ) e ORDER BY at",
    )
    .bind(user)
    .bind(except_sha)
    .bind(WINDOW_HOURS as i32)
    .bind(RESERVATION_HOURS as i32)
    .fetch_all(conn)
    .await?)
}

/// Decides whether `user` may start uploading a file of `size` bytes with content hash `sha`. Call it inside the
/// transaction that holds [`lock_account_uploads`], and insert the `uploads` row in the same transaction.
/// `already_stored`: the same content is in the account's trash, so completing the upload revives that file and adds
/// nothing to what is stored (it still counts toward the daily cap: the bytes are sent again).
pub async fn check_new_upload(
    conn: &mut PgConnection,
    user: Uuid,
    sha: &[u8],
    size: i64,
    already_stored: bool,
) -> Result<Option<Refusal>> {
    let limits: Limits =
        sqlx::query_as("SELECT quota_bytes, daily_upload_bytes, max_file_bytes FROM users WHERE id = $1")
            .bind(user)
            .fetch_optional(&mut *conn)
            .await?
            .unwrap_or_default();
    if let Some(max) = limits.max_file_bytes.filter(|max| size > *max) {
        return Ok(Some(Refusal::FileTooLarge { size, max_bytes: max }));
    }
    if let Some(cap) = limits.daily_upload_bytes {
        let events = window_events(&mut *conn, user, Some(sha)).await?;
        if let Err((used, retry_at)) = fit_daily(&events, cap, size) {
            return Ok(Some(match retry_at {
                Some(retry_at) => Refusal::DailyCap {
                    size,
                    cap,
                    used_bytes: used,
                    retry_at,
                },
                None => Refusal::FileExceedsDailyCap { size, cap },
            }));
        }
    }
    if let Some(quota) = limits.quota_bytes.filter(|_| !already_stored) {
        {
            let stored: i64 = sqlx::query_scalar(
                "SELECT coalesce(originals_bytes + trash_bytes, 0)::bigint FROM user_usage WHERE user_id = $1",
            )
            .bind(user)
            .fetch_optional(&mut *conn)
            .await?
            .unwrap_or(0);
            let reserved: i64 = sqlx::query_scalar(
                "SELECT coalesce(sum(size_bytes), 0)::bigint FROM uploads
                 WHERE owner_id = $1 AND sha256 <> $2 AND created_at > now() - make_interval(hours => $3)",
            )
            .bind(user)
            .bind(sha)
            .bind(RESERVATION_HOURS as i32)
            .fetch_one(&mut *conn)
            .await?;
            if stored + reserved + size > quota {
                return Ok(Some(Refusal::QuotaExceeded {
                    size,
                    quota_bytes: quota,
                    stored_bytes: stored,
                    reserved_bytes: reserved,
                }));
            }
        }
    }
    Ok(None)
}

/// The account's rolling window as the Storage page shows it: bytes counted now and when the oldest of them rolls out.
pub async fn daily_state(db: &PgPool, user: Uuid) -> Result<(i64, Option<OffsetDateTime>)> {
    let mut conn = db.acquire().await?;
    let events = window_events(&mut conn, user, None).await?;
    let used = events.iter().map(|e| e.1).sum();
    Ok((used, events.first().map(|e| e.0 + Duration::hours(WINDOW_HOURS))))
}

/// "3.4 GB" for messages (1024-based, like the web app).
pub fn human(bytes: i64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes.max(0) as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 || v >= 10.0 {
        format!("{} {}", v.round() as i64, UNITS[u])
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use super::*;

    const T0: OffsetDateTime = datetime!(2026-06-01 12:00 UTC);
    fn ev(h: i64, bytes: i64) -> (OffsetDateTime, i64) {
        (T0 + Duration::hours(h), bytes)
    }

    #[test]
    fn exactly_the_cap_fits_and_one_byte_more_does_not() {
        let events = [ev(0, 60), ev(1, 40)];
        assert!(fit_daily(&events, 150, 50).is_ok());
        assert_eq!(
            fit_daily(&events, 150, 51),
            Err((100, Some(T0 + Duration::hours(WINDOW_HOURS))))
        );
        assert!(fit_daily(&[], 100, 100).is_ok());
        assert_eq!(fit_daily(&[], 100, 101), Err((0, None)));
    }

    #[test]
    fn the_reset_is_when_enough_of_the_oldest_uploads_roll_out() {
        let events = [ev(0, 30), ev(2, 30), ev(5, 40)];
        // Needs 40 more room than the cap leaves: the first (30) is not enough, the second frees enough.
        assert_eq!(
            fit_daily(&events, 100, 40),
            Err((100, Some(T0 + Duration::hours(2 + WINDOW_HOURS))))
        );
        // 10 is enough with the first alone.
        assert_eq!(
            fit_daily(&events, 100, 10),
            Err((100, Some(T0 + Duration::hours(WINDOW_HOURS))))
        );
        // A file as big as the cap waits for everything.
        assert_eq!(
            fit_daily(&events, 100, 100),
            Err((100, Some(T0 + Duration::hours(5 + WINDOW_HOURS))))
        );
    }

    #[test]
    fn limits_validate_and_fall_back_field_by_field() {
        assert!(Limits::default().validate().is_ok());
        assert!(
            Limits {
                quota_bytes: Some(1),
                ..Default::default()
            }
            .validate()
            .is_ok()
        );
        for bad in [
            Limits {
                quota_bytes: Some(0),
                ..Default::default()
            },
            Limits {
                daily_upload_bytes: Some(-1),
                ..Default::default()
            },
            Limits {
                max_file_bytes: Some(0),
                ..Default::default()
            },
        ] {
            assert!(bad.validate().is_err(), "{bad:?}");
        }
        let preset = Limits {
            quota_bytes: Some(5),
            ..Default::default()
        };
        let default = Limits {
            quota_bytes: Some(9),
            daily_upload_bytes: Some(7),
            max_file_bytes: None,
        };
        assert_eq!(
            preset.or(default),
            Limits {
                quota_bytes: Some(5),
                daily_upload_bytes: Some(7),
                max_file_bytes: None
            }
        );
        assert!(Limits::default().is_unlimited());
    }
}
