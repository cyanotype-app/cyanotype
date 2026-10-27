//! API key scopes and expiry, enforced by the extractors every route takes. One table lists every route of the service;
//! a source scan fails the build of the tests when a route is added without a row, so a new route cannot ship without a
//! decision about which scopes reach it.
use std::collections::BTreeSet;

use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::testutil::{TestDb, app, send, send_cookie, session};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Who {
    /// Check, upload parts, complete, the count, the processing status: `import` and `full` keys.
    Import,
    /// Any other `GET`: `read` and `full` keys.
    Read,
    /// Any other method: `full` keys only.
    Write,
    /// Administrators by session only: every key is refused, whatever its scope.
    Admin,
    /// No login at all (public share links): keys are not even looked at.
    Public,
}
use Who::*;

const ID: &str = "00000000-0000-4000-8000-000000000001";

/// (method, path template, who). `{x}` placeholders are filled with a valid-looking value.
const ROUTES: &[(&str, &str, Who)] = &[
    ("POST", "/assets/check", Import),
    ("POST", "/assets/complete", Import),
    ("PUT", "/assets/uploads/{upload_id}/parts/{number}", Import),
    ("GET", "/assets/count", Import),
    ("GET", "/assets/processing", Import),
    ("GET", "/assets", Read),
    ("GET", "/assets/timeline", Read),
    ("GET", "/assets/search", Read),
    ("GET", "/assets/search/facets", Read),
    ("GET", "/assets/slideshow", Read),
    ("GET", "/assets/previews", Read),
    ("GET", "/assets/map", Read),
    ("GET", "/assets/tiles", Read),
    ("GET", "/assets/tiles/{name}", Read),
    ("GET", "/assets/places", Read),
    ("GET", "/assets/usage", Read),
    ("GET", "/assets/prefs", Read),
    ("GET", "/assets/memories", Read),
    ("GET", "/assets/trips", Read),
    ("GET", "/assets/trash", Read),
    ("GET", "/assets/albums", Read),
    ("GET", "/assets/tags", Read),
    ("GET", "/assets/duplicates", Read),
    ("GET", "/assets/people/status", Read),
    ("GET", "/assets/people", Read),
    ("GET", "/assets/people/{id}", Read),
    ("GET", "/assets/people/{id}/photos", Read),
    ("GET", "/assets/people/{id}/timeline", Read),
    ("GET", "/assets/people/{id}/suggestions", Read),
    ("GET", "/assets/people/{id}/faces", Read),
    ("GET", "/assets/{id}/faces", Read),
    ("GET", "/assets/pets/status", Read),
    ("GET", "/assets/pets", Read),
    ("GET", "/assets/pets/photos", Read),
    ("GET", "/assets/pets/timeline", Read),
    ("GET", "/assets/pets/named", Read),
    ("GET", "/assets/pets/named/{id}", Read),
    ("GET", "/assets/pets/named/{id}/photos", Read),
    ("GET", "/assets/pets/named/{id}/timeline", Read),
    ("GET", "/assets/pets/named/{id}/suggestions", Read),
    ("GET", "/assets/pets/named/{id}/detections", Read),
    ("GET", "/assets/{id}/pets", Read),
    ("GET", "/assets/albums/{id}", Read),
    ("GET", "/assets/albums/{id}/assets", Read),
    ("GET", "/assets/albums/{id}/zip", Read),
    ("GET", "/assets/shares", Read),
    ("GET", "/assets/shares/{id}", Read),
    ("GET", "/assets/{id}", Read),
    ("GET", "/assets/{id}/similar", Read),
    ("GET", "/assets/{id}/details", Read),
    ("GET", "/assets/{id}/text", Read),
    ("GET", "/assets/{id}/download", Read),
    ("GET", "/assets/{id}/download/edited", Read),
    ("GET", "/assets/{id}/stack", Read),
    ("POST", "/assets/favorite", Write),
    ("POST", "/assets/rating", Write),
    ("POST", "/assets/duplicates/scan", Write),
    ("POST", "/assets/duplicates/{id}/keep", Write),
    ("POST", "/assets/duplicates/{id}/dismiss", Write),
    ("POST", "/assets/tags", Write),
    ("POST", "/assets/tags/apply", Write),
    ("PATCH", "/assets/tags/{id}", Write),
    ("DELETE", "/assets/tags/{id}", Write),
    ("POST", "/assets/tags/{id}/merge", Write),
    ("PATCH", "/assets/people/{id}", Write),
    ("DELETE", "/assets/people/{id}", Write),
    ("POST", "/assets/people/bulk", Write),
    ("POST", "/assets/people/{id}/merge", Write),
    ("POST", "/assets/people/{id}/faces/remove", Write),
    ("POST", "/assets/faces/{id}/assign", Write),
    ("POST", "/assets/faces/{id}/reject", Write),
    ("POST", "/assets/pets/named", Write),
    ("PATCH", "/assets/pets/named/{id}", Write),
    ("DELETE", "/assets/pets/named/{id}", Write),
    ("POST", "/assets/pets/named/{id}/merge", Write),
    ("POST", "/assets/pets/named/{id}/photos", Write),
    ("POST", "/assets/pets/named/{id}/photos/remove", Write),
    ("POST", "/assets/pets/named/{id}/suggestions/answer", Write),
    ("POST", "/assets/pets/detections/{id}/assign", Write),
    ("POST", "/assets/pets/detections/{id}/reject", Write),
    ("POST", "/assets/faces/{id}/not-a-face", Write),
    ("DELETE", "/assets/faces/{id}/not-a-face", Write),
    ("POST", "/assets/hide", Write),
    ("POST", "/assets/archive", Write),
    ("PUT", "/assets/prefs", Write),
    ("POST", "/assets/memories/dismiss", Write),
    ("POST", "/assets/takeout/plan", Write),
    ("POST", "/assets/edits", Write),
    ("POST", "/assets/processing/retry", Write),
    ("POST", "/assets/trash", Write),
    ("POST", "/assets/trash/empty", Write),
    ("POST", "/assets/restore", Write),
    ("POST", "/assets/purge", Write),
    ("POST", "/assets/albums", Write),
    ("POST", "/assets/albums/preview", Write),
    ("PATCH", "/assets/albums/{id}", Write),
    ("DELETE", "/assets/albums/{id}", Write),
    ("POST", "/assets/albums/{id}/assets", Write),
    ("POST", "/assets/albums/{id}/assets/remove", Write),
    ("POST", "/assets/zip", Write),
    ("POST", "/assets/zip/plan", Write),
    ("POST", "/assets/shares", Write),
    ("PATCH", "/assets/shares/{id}", Write),
    ("DELETE", "/assets/shares/{id}", Write),
    ("POST", "/assets/stacks", Write),
    ("POST", "/assets/stacks/restore", Write),
    ("POST", "/assets/{id}/stack/cover", Write),
    ("POST", "/assets/{id}/stack/remove", Write),
    ("DELETE", "/assets/{id}/stack", Write),
    ("POST", "/assets/{id}/unpair", Write),
    ("DELETE", "/assets/{id}", Write),
    ("GET", "/assets/admin/health", Admin),
    ("POST", "/assets/admin/duplicates/scan", Admin),
    ("POST", "/assets/admin/jobs/retry", Admin),
    ("GET", "/assets/admin/accounts", Admin),
    ("PUT", "/assets/admin/accounts/{id}/limits", Admin),
    ("GET", "/assets/admin/default-limits", Admin),
    ("PUT", "/assets/admin/default-limits", Admin),
    ("GET", "/assets/admin/integrity", Admin),
    ("POST", "/assets/admin/integrity/run", Admin),
    ("GET", "/assets/admin/integrity/findings", Admin),
    ("POST", "/assets/admin/integrity/findings/{id}", Admin),
    ("GET", "/assets/admin/bucket-check", Admin),
    ("POST", "/assets/admin/bucket-check", Admin),
    ("GET", "/assets/public/s/{token}", Public),
    ("POST", "/assets/public/s/{token}/unlock", Public),
    ("GET", "/assets/public/s/{token}/items", Public),
    ("GET", "/assets/public/s/{token}/items/{id}", Public),
    ("POST", "/assets/public/s/{token}/items/{id}/prepare", Public),
    ("GET", "/assets/public/s/{token}/items/{id}/download", Public),
    ("GET", "/assets/public/s/{token}/zip/plan", Public),
    ("GET", "/assets/public/s/{token}/zip", Public),
];

fn fill(path: &str) -> String {
    path.replace("{upload_id}", "u1")
        .replace("{number}", "1")
        .replace("{name}", "world.pmtiles")
        .replace("{token}", "tok")
        .replace("{id}", ID)
}

/// Every path literal given to `.route(` in the service's route files.
fn routed_paths() -> BTreeSet<String> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = vec![root.join("routes.rs")];
    for e in std::fs::read_dir(root.join("routes")).unwrap() {
        let p = e.unwrap().path();
        if p.extension().is_some_and(|x| x == "rs") && !p.to_string_lossy().ends_with("_tests.rs") {
            files.push(p);
        }
    }
    let mut out = BTreeSet::new();
    for f in files {
        let src = std::fs::read_to_string(&f).unwrap();
        // Only the production part: tests build their own routers.
        let src = src.split("#[cfg(test)]\nmod tests").next().unwrap();
        for chunk in src.split(".route(").skip(1) {
            let rest = chunk.trim_start();
            if let Some(lit) = rest.strip_prefix('"')
                && let Some(end) = lit.find('"')
                && lit.starts_with("/assets")
            {
                out.insert(lit[..end].to_string());
            }
        }
    }
    out
}

#[test]
fn every_route_of_the_service_has_a_scope_decision() {
    let listed: BTreeSet<String> = ROUTES.iter().map(|r| r.1.to_string()).collect();
    let routed = routed_paths();
    assert!(routed.len() > 30, "the source scan found {} routes", routed.len());
    let missing: Vec<_> = routed.difference(&listed).collect();
    assert!(
        missing.is_empty(),
        "routes without a row in ROUTES (decide their scope): {missing:?}"
    );
    let stale: Vec<_> = listed.difference(&routed).collect();
    assert!(stale.is_empty(), "rows for routes that no longer exist: {stale:?}");
}

async fn key_with(pool: &sqlx::PgPool, user: Uuid, scope: &str, expired: bool) -> String {
    let key = format!("cyk_{}", Uuid::new_v4().simple());
    sqlx::query(
        "INSERT INTO api_keys (user_id, name, key_hash, scope, expires_at)
         VALUES ($1, $2, $3, $4, CASE WHEN $5 THEN now() - interval '1 minute' ELSE now() + interval '1 day' END)",
    )
    .bind(user)
    .bind(scope)
    .bind(cyanotype_service::auth::sha256(key.as_bytes()))
    .bind(scope)
    .bind(expired)
    .execute(pool)
    .await
    .unwrap();
    key
}

/// Whether the response says "your key's scope is too small" (or "session only" for the admin routes).
fn refused_by_scope(s: StatusCode, v: &Value) -> bool {
    s == StatusCode::FORBIDDEN
        && (v["code"] == "insufficient_scope"
            || v["error"]
                .as_str()
                .is_some_and(|e| e.contains("login session, not an API key")))
}

#[tokio::test]
async fn each_route_admits_exactly_the_scopes_it_names() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, _) = crate::testutil::user(&t.pool, "a@example.com").await;
    let app = app(&t.pool);
    let keys = [
        ("import", key_with(&t.pool, uid, "import", false).await),
        ("read", key_with(&t.pool, uid, "read", false).await),
        ("full", key_with(&t.pool, uid, "full", false).await),
    ];
    for (method, path, who) in ROUTES {
        let url = fill(path);
        for (scope, key) in &keys {
            let (s, v) = send(&app, method, &url, key, None).await;
            let allowed = match who {
                Import => *scope != "read",
                Read => *scope != "import",
                Write => *scope == "full",
                Admin | Public => false,
            };
            if *who == Public {
                assert!(
                    !refused_by_scope(s, &v) && s != StatusCode::UNAUTHORIZED,
                    "{method} {path}: public routes ignore keys ({s} {v})"
                );
            } else if allowed {
                assert!(
                    s != StatusCode::UNAUTHORIZED && !refused_by_scope(s, &v),
                    "{scope} key must reach {method} {path} ({s} {v})"
                );
            } else {
                assert!(
                    refused_by_scope(s, &v),
                    "{scope} key must be refused on {method} {path}, got {s} {v}"
                );
            }
        }
    }
    t.teardown().await;
}

#[tokio::test]
async fn a_session_reaches_every_non_admin_route_and_a_member_session_no_admin_one() {
    let Some(t) = TestDb::new().await else { return };
    let (_, member) = session(&t.pool, "m@example.com", "member").await;
    let app = app(&t.pool);
    for (method, path, who) in ROUTES {
        if *who == Public {
            continue;
        }
        let (s, v) = send_cookie(&app, method, &fill(path), Some(&member), None).await;
        if *who == Admin {
            assert_eq!(s, StatusCode::FORBIDDEN, "{method} {path} {v}");
        } else {
            assert!(
                s != StatusCode::UNAUTHORIZED && !refused_by_scope(s, &v),
                "a session reaches {method} {path} ({s} {v})"
            );
        }
    }
    t.teardown().await;
}

#[tokio::test]
async fn an_expired_key_is_refused_on_every_route() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, _) = crate::testutil::user(&t.pool, "a@example.com").await;
    let app = app(&t.pool);
    for scope in ["import", "read", "full"] {
        let key = key_with(&t.pool, uid, scope, true).await;
        for (method, path, who) in ROUTES {
            if *who == Public {
                continue;
            }
            let (s, _) = send(&app, method, &fill(path), &key, None).await;
            // Admin routes refuse every key (403) before looking at it; the rest see an expired key as no key.
            let want = if *who == Admin {
                StatusCode::FORBIDDEN
            } else {
                StatusCode::UNAUTHORIZED
            };
            assert_eq!(s, want, "expired {scope} key on {method} {path}");
        }
    }
    t.teardown().await;
}

#[tokio::test]
async fn a_scoped_key_can_still_do_its_job_end_to_end() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, _) = crate::testutil::user(&t.pool, "a@example.com").await;
    let app = app(&t.pool);
    let import = key_with(&t.pool, uid, "import", false).await;
    let read = key_with(&t.pool, uid, "read", false).await;

    // The import key does what `cyanotype import` needs: count and status work, a dedupe check answers.
    let (s, v) = send(&app, "GET", "/assets/count", &import, None).await;
    assert_eq!((s, v["count"].as_i64()), (StatusCode::OK, Some(0)), "{v}");
    let (s, _) = send(&app, "GET", "/assets/processing", &import, None).await;
    assert_eq!(s, StatusCode::OK);
    let (s, v) = send(
        &app,
        "POST",
        "/assets/check",
        &import,
        Some(json!({"files": [{"sha256": "a".repeat(64), "size": 10, "name": "x.jpg"}]})),
    )
    .await;
    assert!(s != StatusCode::UNAUTHORIZED && !refused_by_scope(s, &v), "{s} {v}");
    // ... and it cannot read the library, even its own count of what is there beyond the import path.
    let (s, v) = send(&app, "GET", "/assets", &import, None).await;
    assert!(refused_by_scope(s, &v), "{s} {v}");

    // The read key reads, and neither uploads nor changes anything.
    let (s, _) = send(&app, "GET", "/assets", &read, None).await;
    assert_eq!(s, StatusCode::OK);
    let (s, v) = send(&app, "POST", "/assets/check", &read, Some(json!({"files": []}))).await;
    assert!(refused_by_scope(s, &v), "{s} {v}");
    let (s, v) = send(
        &app,
        "POST",
        "/assets/favorite",
        &read,
        Some(json!({"ids": [], "favorite": true})),
    )
    .await;
    assert!(refused_by_scope(s, &v), "{s} {v}");
    t.teardown().await;
}
