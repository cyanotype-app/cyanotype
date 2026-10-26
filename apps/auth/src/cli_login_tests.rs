// Included at the end of `routes::tests` (it shares that module's helpers). Signing the command line in through the browser:
// the loopback flow with PKCE, the device flow, and what each refuses. The CLI's side of the exchange is tested in apps/cli.
const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
const CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
const CALLBACK: &str = "http://127.0.0.1:53682/callback";

fn open_app(db: PgPool) -> Router {
    app(db, |c| c.registration = RegistrationMode::Open)
}

fn authorize_body(redirect: &str) -> String {
    json!({
        "redirect_uri": redirect, "state": "state-1", "code_challenge": CHALLENGE,
        "device": "test-laptop", "scope": "import",
    })
    .to_string()
}

async fn send(app: &Router, method: &str, path: &str, auth: (&str, &str), body: Option<&str>) -> (StatusCode, Value) {
    let res = app.clone().oneshot(limited_call(method, path, auth, body)).await.unwrap();
    let status = res.status();
    (status, json_of(res).await)
}

/// Presses Allow on the loopback page for the session `cookie`; returns the code the redirect carries.
async fn allow(app: &Router, cookie: &str) -> String {
    let (st, v) = send(
        app,
        "POST",
        "/auth/cli/authorize/approve",
        ("cookie", cookie),
        Some(&authorize_body(CALLBACK)),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let redirect = v["redirect"].as_str().unwrap();
    let rest = redirect.strip_prefix(&format!("{CALLBACK}?code=")).unwrap();
    let (code, state) = rest.split_once("&state=").unwrap();
    assert_eq!(state, "state-1");
    code.to_string()
}

fn exchange_body(code: &str, verifier: &str, redirect: &str) -> String {
    json!({"code": code, "code_verifier": verifier, "redirect_uri": redirect}).to_string()
}

async fn exchange(app: &Router, code: &str, verifier: &str) -> (StatusCode, Value) {
    send(app, "POST", "/auth/cli/token", ("x-none", ""), Some(&exchange_body(code, verifier, CALLBACK))).await
}

#[tokio::test]
async fn loopback_login_trades_the_code_for_a_key_once() {
    let Some(t) = TestDb::new().await else { return };
    let app = open_app(t.pool.clone());
    let cookie = register_cookie(&app, "alpha@example.com").await;

    let (st, v) = send(&app, "POST", "/auth/cli/authorize/check", ("cookie", &cookie), Some(&authorize_body(CALLBACK))).await;
    assert_eq!((st, v["device"].as_str(), v["scope"].as_str()), (StatusCode::OK, Some("test-laptop"), Some("import")));

    let code = allow(&app, &cookie).await;
    // Only the hash of the code is stored.
    let (stored,): (Vec<u8>,) = sqlx::query_as("SELECT code_hash FROM cli_auth_codes").fetch_one(&t.pool).await.unwrap();
    assert_eq!(stored, cyanotype_service::auth::sha256(code.as_bytes()));
    assert_ne!(stored, code.as_bytes());

    let (st, v) = exchange(&app, &code, VERIFIER).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let key = v["key"].as_str().unwrap().to_string();
    assert!(key.starts_with("cyk_"));
    assert_eq!((v["name"].as_str(), v["scope"].as_str(), v["email"].as_str()), (Some("cyanotype CLI on test-laptop"), Some("import"), Some("alpha@example.com")));

    // The key works, with the scope that was asked for.
    let bearer = format!("Bearer {key}");
    let (st, me) = send(&app, "GET", "/auth/me", ("authorization", &bearer), None).await;
    assert_eq!((st, me["email"].as_str(), me["scope"].as_str()), (StatusCode::OK, Some("alpha@example.com"), Some("import")));

    // It is an ordinary key: listed with its name, and revocable from the keys page.
    let (_, list) = send(&app, "GET", "/auth/api-keys", ("cookie", &cookie), None).await;
    assert_eq!(list["items"][0]["name"], "cyanotype CLI on test-laptop");
    assert!(list["items"][0]["last_used_at"].is_string());

    // Single use: the same code again, even with the right verifier, is refused.
    let (st, v) = exchange(&app, &code, VERIFIER).await;
    assert_eq!((st, v["code"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_grant")));

    let id = list["items"][0]["id"].as_str().unwrap();
    let (st, _) = send(&app, "DELETE", &format!("/auth/api-keys/{id}"), ("cookie", &cookie), None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _) = send(&app, "GET", "/auth/me", ("authorization", &bearer), None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);

    // Approval and issue are in the audit log, without any secret.
    let rows: Vec<(String, Value)> = sqlx::query_as("SELECT action, detail FROM audit_log WHERE action LIKE 'cli_login.%' ORDER BY id")
        .fetch_all(&t.pool)
        .await
        .unwrap();
    assert_eq!(rows.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(), ["cli_login.approve", "cli_login.issue"]);
    assert_eq!(rows[0].1["device"], "test-laptop");
    assert!(!rows.iter().any(|r| r.1.to_string().contains(&key) || r.1.to_string().contains(&code)));
    t.teardown().await;
}

#[tokio::test]
async fn a_wrong_verifier_or_redirect_or_late_exchange_spends_the_code() {
    let Some(t) = TestDb::new().await else { return };
    let app = open_app(t.pool.clone());
    let cookie = register_cookie(&app, "alpha@example.com").await;

    // Wrong verifier (a valid-looking one, but not the one behind the challenge): refused, and the code is gone.
    let code = allow(&app, &cookie).await;
    let other = "a".repeat(43);
    let (st, v) = exchange(&app, &code, &other).await;
    assert_eq!((st, v["code"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_grant")));
    let (st, _) = exchange(&app, &code, VERIFIER).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "a guess must not leave the code usable");

    // A verifier that is not even well formed.
    let code = allow(&app, &cookie).await;
    assert_eq!(exchange(&app, &code, "short").await.0, StatusCode::BAD_REQUEST);

    // Another redirect_uri than the one approved.
    let code = allow(&app, &cookie).await;
    let (st, _) = send(
        &app,
        "POST",
        "/auth/cli/token",
        ("x-none", ""),
        Some(&exchange_body(&code, VERIFIER, "http://127.0.0.1:53683/callback")),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    // An unknown code.
    assert_eq!(exchange(&app, &"0".repeat(64), VERIFIER).await.0, StatusCode::BAD_REQUEST);

    // Expired (60 s): the right verifier no longer helps.
    let code = allow(&app, &cookie).await;
    sqlx::query("UPDATE cli_auth_codes SET expires_at = now() - interval '1 second'")
        .execute(&t.pool)
        .await
        .unwrap();
    let (st, v) = exchange(&app, &code, VERIFIER).await;
    assert_eq!((st, v["code"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_grant")));

    // None of that made a key.
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM api_keys").fetch_one(&t.pool).await.unwrap();
    assert_eq!(n, 0);
    t.teardown().await;
}

#[tokio::test]
async fn only_exact_loopback_redirects_are_accepted() {
    let Some(t) = TestDb::new().await else { return };
    let app = open_app(t.pool.clone());
    let cookie = register_cookie(&app, "alpha@example.com").await;
    for bad in [
        "http://localhost:53682/callback",
        "http://127.0.0.2:53682/callback",
        "https://127.0.0.1:53682/callback",
        "https://evil.example/callback",
        "http://evil.example:53682/callback",
        "http://127.0.0.1@evil.example/callback",
        "http://127.0.0.1:53682@evil.example/callback",
        "http://127.0.0.1:53682/callback?next=https://evil.example",
        "javascript:alert(1)",
        "",
    ] {
        for path in ["check", "approve", "deny"] {
            let (st, _) = send(&app, "POST", &format!("/auth/cli/authorize/{path}"), ("cookie", &cookie), Some(&authorize_body(bad))).await;
            assert_eq!(st, StatusCode::BAD_REQUEST, "{path} {bad:?}");
        }
    }
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM cli_auth_codes").fetch_one(&t.pool).await.unwrap();
    assert_eq!(n, 0);
    // IPv6 loopback is fine; a bad challenge or state is not.
    let (st, _) = send(&app, "POST", "/auth/cli/authorize/check", ("cookie", &cookie), Some(&authorize_body("http://[::1]:53682/callback"))).await;
    assert_eq!(st, StatusCode::OK);
    for body in [
        json!({"redirect_uri": CALLBACK, "state": "s", "code_challenge": "short", "device": "x"}),
        json!({"redirect_uri": CALLBACK, "state": "a&b=c", "code_challenge": CHALLENGE, "device": "x"}),
        json!({"redirect_uri": CALLBACK, "state": "s", "code_challenge": CHALLENGE, "scope": "root"}),
        // A full key (it can delete photos) is never handed out by an approval page.
        json!({"redirect_uri": CALLBACK, "state": "s", "code_challenge": CHALLENGE, "scope": "full"}),
    ] {
        let (st, _) = send(&app, "POST", "/auth/cli/authorize/approve", ("cookie", &cookie), Some(&body.to_string())).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    }
    t.teardown().await;
}

#[tokio::test]
async fn denying_redirects_with_an_error_and_is_audited() {
    let Some(t) = TestDb::new().await else { return };
    let app = open_app(t.pool.clone());
    let cookie = register_cookie(&app, "alpha@example.com").await;
    let (st, v) = send(&app, "POST", "/auth/cli/authorize/deny", ("cookie", &cookie), Some(&authorize_body(CALLBACK))).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["redirect"], format!("{CALLBACK}?error=access_denied&state=state-1"));
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM audit_log WHERE action = 'cli_login.deny'").fetch_one(&t.pool).await.unwrap();
    assert_eq!(n, 1);
    t.teardown().await;
}

#[tokio::test]
async fn approving_needs_a_session_that_is_not_disabled_and_not_a_key() {
    let Some(t) = TestDb::new().await else { return };
    let app = open_app(t.pool.clone());
    let admin = register_cookie(&app, "alpha@example.com").await;
    let member = register_cookie(&app, "bravo@example.com").await;
    let body = authorize_body(CALLBACK);

    // No session at all.
    let (st, _) = send(&app, "POST", "/auth/cli/authorize/approve", ("x-none", ""), Some(&body)).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);

    // A key, even a full one, cannot approve (a leaked key must not mint more keys).
    let code = allow(&app, &member).await;
    let (_, v) = exchange(&app, &code, VERIFIER).await;
    let bearer = format!("Bearer {}", v["key"].as_str().unwrap());
    let (st, _) = send(&app, "POST", "/auth/cli/authorize/approve", ("authorization", &bearer), Some(&body)).await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // Pressed Allow, then the admin disabled the account: the code buys nothing, and the session no longer approves.
    let code = allow(&app, &member).await;
    let (_, accounts) = send(&app, "GET", "/auth/admin/audit", ("cookie", &admin), None).await;
    assert!(accounts["items"].is_array());
    let (id,): (Uuid,) = sqlx::query_as("SELECT id FROM users WHERE email = 'bravo@example.com'").fetch_one(&t.pool).await.unwrap();
    let (st, _) = send(&app, "POST", &format!("/auth/admin/users/{id}/disable"), ("cookie", &admin), None).await;
    assert_eq!(st, StatusCode::OK);
    let (st, v) = exchange(&app, &code, VERIFIER).await;
    assert_eq!((st, v["code"].as_str()), (StatusCode::FORBIDDEN, Some("account_disabled")));
    let (st, _) = send(&app, "POST", "/auth/cli/authorize/approve", ("cookie", &member), Some(&body)).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    let (st, _) = send(&app, "POST", "/auth/cli/device/approve", ("cookie", &member), Some(r#"{"user_code":"BCDF-GHJK"}"#)).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    t.teardown().await;
}

async fn start_device(app: &Router) -> (String, String) {
    let (st, v) = send(app, "POST", "/auth/cli/device", ("x-none", ""), Some(r#"{"device":"test-laptop","scope":"import"}"#)).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!((v["expires_in"].clone(), v["interval"].clone(), v["verification_path"].as_str()), (json!(600), json!(5), Some("/activate")));
    let user_code = v["user_code"].as_str().unwrap().to_string();
    assert_eq!((user_code.len(), user_code.as_bytes()[4]), (9, b'-'));
    (v["device_code"].as_str().unwrap().to_string(), user_code)
}

async fn poll(app: &Router, device_code: &str) -> (StatusCode, Value) {
    send(app, "POST", "/auth/cli/device/token", ("x-none", ""), Some(&json!({"device_code": device_code}).to_string())).await
}

/// Lets the poll interval pass without waiting for it.
async fn age_polls(pool: &PgPool) {
    sqlx::query("UPDATE cli_device_codes SET last_polled_at = now() - interval '1 minute'")
        .execute(pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn device_login_waits_slows_down_then_hands_over_the_key_once() {
    let Some(t) = TestDb::new().await else { return };
    let app = open_app(t.pool.clone());
    let cookie = register_cookie(&app, "alpha@example.com").await;
    let (st, _) = send(&app, "POST", "/auth/cli/device", ("x-none", ""), Some(r#"{"device":"test-laptop","scope":"full"}"#)).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "a code login never makes a full key");
    let (device_code, user_code) = start_device(&app).await;

    // Both codes are stored only as hashes.
    let (dh, uh): (Vec<u8>, Vec<u8>) = sqlx::query_as("SELECT device_code_hash, user_code_hash FROM cli_device_codes").fetch_one(&t.pool).await.unwrap();
    assert_eq!(dh, cyanotype_service::auth::sha256(device_code.as_bytes()));
    assert_eq!(uh, cyanotype_service::auth::sha256(user_code.replace('-', "").as_bytes()));

    // Nothing decided yet.
    let (st, v) = poll(&app, &device_code).await;
    assert_eq!((st, v["code"].as_str()), (StatusCode::BAD_REQUEST, Some("authorization_pending")));
    // An immediate second poll is too fast: slow_down, and the interval grows by five seconds.
    let (st, v) = poll(&app, &device_code).await;
    assert_eq!((st, v["code"].as_str(), v["interval"].clone()), (StatusCode::BAD_REQUEST, Some("slow_down"), json!(10)));
    age_polls(&t.pool).await;
    let (_, v) = poll(&app, &device_code).await;
    assert_eq!(v["code"], "authorization_pending");

    // The person types the code (lowercase, no hyphen is fine) on /activate and sees the device before allowing.
    let typed = user_code.replace('-', "").to_lowercase();
    let (st, v) = send(&app, "POST", "/auth/cli/device/lookup", ("cookie", &cookie), Some(&json!({"user_code": typed}).to_string())).await;
    assert_eq!((st, v["device"].as_str(), v["scope"].as_str()), (StatusCode::OK, Some("test-laptop"), Some("import")));
    let body = json!({"user_code": user_code}).to_string();
    let (st, _) = send(&app, "POST", "/auth/cli/device/approve", ("cookie", &cookie), Some(&body)).await;
    assert_eq!(st, StatusCode::OK);
    // Decided once: a second decision finds nothing pending.
    let (st, _) = send(&app, "POST", "/auth/cli/device/deny", ("cookie", &cookie), Some(&body)).await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    age_polls(&t.pool).await;
    let (st, v) = poll(&app, &device_code).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["name"], "cyanotype CLI on test-laptop");
    let bearer = format!("Bearer {}", v["key"].as_str().unwrap());
    let (st, _) = send(&app, "GET", "/auth/me", ("authorization", &bearer), None).await;
    assert_eq!(st, StatusCode::OK);

    // The key is handed over once.
    age_polls(&t.pool).await;
    let (st, v) = poll(&app, &device_code).await;
    assert_eq!((st, v["code"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_grant")));
    let rows: Vec<(String, Value)> = sqlx::query_as("SELECT action, detail FROM audit_log WHERE action LIKE 'cli_login.%' ORDER BY id").fetch_all(&t.pool).await.unwrap();
    assert_eq!(rows.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(), ["cli_login.approve", "cli_login.issue"]);
    assert_eq!(rows[0].1["flow"], "device");
    t.teardown().await;
}

#[tokio::test]
async fn device_login_can_be_denied_expire_or_be_guessed_wrong() {
    let Some(t) = TestDb::new().await else { return };
    let app = open_app(t.pool.clone());
    let cookie = register_cookie(&app, "alpha@example.com").await;

    // Denied.
    let (device_code, user_code) = start_device(&app).await;
    let body = json!({"user_code": user_code}).to_string();
    let (st, _) = send(&app, "POST", "/auth/cli/device/deny", ("cookie", &cookie), Some(&body)).await;
    assert_eq!(st, StatusCode::OK);
    let (st, v) = poll(&app, &device_code).await;
    assert_eq!((st, v["code"].as_str()), (StatusCode::BAD_REQUEST, Some("access_denied")));
    let (_, v) = poll(&app, &device_code).await;
    assert_eq!(v["code"], "invalid_grant", "a denial is reported once");

    // Expired before anyone decided: the poll says so, and the code can no longer be approved.
    let (device_code, user_code) = start_device(&app).await;
    sqlx::query("UPDATE cli_device_codes SET expires_at = now() - interval '1 second'").execute(&t.pool).await.unwrap();
    let body = json!({"user_code": user_code}).to_string();
    let (st, _) = send(&app, "POST", "/auth/cli/device/approve", ("cookie", &cookie), Some(&body)).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, v) = poll(&app, &device_code).await;
    assert_eq!((st, v["code"].as_str()), (StatusCode::BAD_REQUEST, Some("expired_token")));

    // Approved but never collected before it expired: no key.
    let (device_code, user_code) = start_device(&app).await;
    let body = json!({"user_code": user_code}).to_string();
    send(&app, "POST", "/auth/cli/device/approve", ("cookie", &cookie), Some(&body)).await;
    sqlx::query("UPDATE cli_device_codes SET expires_at = now() - interval '1 second'").execute(&t.pool).await.unwrap();
    let (_, v) = poll(&app, &device_code).await;
    assert_eq!(v["code"], "expired_token");

    // Wrong or malformed codes find nothing.
    for bad in ["BCDF-GHJK", "nonsense", "", "AEIO-UAEI"] {
        let (st, v) = send(&app, "POST", "/auth/cli/device/lookup", ("cookie", &cookie), Some(&json!({"user_code": bad}).to_string())).await;
        assert_eq!((st, v["code"].as_str()), (StatusCode::NOT_FOUND, Some("unknown_code")), "{bad:?}");
    }
    assert_eq!(poll(&app, "not-a-device-code").await.1["code"], "invalid_grant");
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM api_keys").fetch_one(&t.pool).await.unwrap();
    assert_eq!(n, 0);
    t.teardown().await;
}

#[tokio::test]
async fn a_device_approved_by_an_account_that_was_then_disabled_gets_no_key() {
    let Some(t) = TestDb::new().await else { return };
    let app = open_app(t.pool.clone());
    let admin = register_cookie(&app, "alpha@example.com").await;
    let member = register_cookie(&app, "bravo@example.com").await;
    let (device_code, user_code) = start_device(&app).await;
    let body = json!({"user_code": user_code}).to_string();
    send(&app, "POST", "/auth/cli/device/approve", ("cookie", &member), Some(&body)).await;
    let (id,): (Uuid,) = sqlx::query_as("SELECT id FROM users WHERE email = 'bravo@example.com'").fetch_one(&t.pool).await.unwrap();
    send(&app, "POST", &format!("/auth/admin/users/{id}/disable"), ("cookie", &admin), None).await;
    let (st, v) = poll(&app, &device_code).await;
    assert_eq!((st, v["code"].as_str()), (StatusCode::FORBIDDEN, Some("account_disabled")));
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM api_keys").fetch_one(&t.pool).await.unwrap();
    assert_eq!(n, 0);
    t.teardown().await;
}

#[tokio::test]
async fn logout_revokes_the_key_it_is_called_with() {
    let Some(t) = TestDb::new().await else { return };
    let app = open_app(t.pool.clone());
    let cookie = register_cookie(&app, "alpha@example.com").await;
    let code = allow(&app, &cookie).await;
    let (_, v) = exchange(&app, &code, VERIFIER).await;
    let bearer = format!("Bearer {}", v["key"].as_str().unwrap());
    let (st, _) = send(&app, "POST", "/auth/cli/logout", ("authorization", &bearer), None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _) = send(&app, "GET", "/auth/me", ("authorization", &bearer), None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    // Already gone, or never a key: 401, and a session cookie is not a key.
    let (st, _) = send(&app, "POST", "/auth/cli/logout", ("authorization", &bearer), None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    let (st, _) = send(&app, "POST", "/auth/cli/logout", ("cookie", &cookie), None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    t.teardown().await;
}

#[tokio::test]
async fn the_code_endpoints_share_the_login_rate_limit() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(t.pool.clone(), |c| {
        c.registration = RegistrationMode::Open;
        c.rate_limit.burst = 3;
        c.rate_limit.per_minute = 1;
    });
    let mut last = StatusCode::OK;
    for _ in 0..5 {
        last = exchange(&app, &"0".repeat(64), VERIFIER).await.0;
    }
    assert_eq!(last, StatusCode::TOO_MANY_REQUESTS);
    let (st, _) = send(&app, "POST", "/auth/cli/device", ("x-none", ""), Some("{}")).await;
    assert_eq!(st, StatusCode::TOO_MANY_REQUESTS, "same bucket as login");
    t.teardown().await;
}

#[tokio::test]
async fn admins_see_an_accounts_keys_with_last_use_and_can_revoke_them() {
    let Some(t) = TestDb::new().await else { return };
    let app = open_app(t.pool.clone());
    let admin = register_cookie(&app, "alpha@example.com").await;
    let member = register_cookie(&app, "bravo@example.com").await;
    let code = allow(&app, &member).await;
    let (_, v) = exchange(&app, &code, VERIFIER).await;
    let bearer = format!("Bearer {}", v["key"].as_str().unwrap());
    send(&app, "GET", "/auth/me", ("authorization", &bearer), None).await;
    let (uid,): (Uuid,) = sqlx::query_as("SELECT id FROM users WHERE email = 'bravo@example.com'").fetch_one(&t.pool).await.unwrap();

    let (st, list) = send(&app, "GET", &format!("/auth/admin/users/{uid}/api-keys"), ("cookie", &admin), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(list["items"][0]["name"], "cyanotype CLI on test-laptop");
    assert!(list["items"][0]["last_used_at"].is_string());
    assert!(list["items"][0].get("key").is_none() && list["items"][0].get("key_hash").is_none());
    // Members cannot.
    let (st, _) = send(&app, "GET", &format!("/auth/admin/users/{uid}/api-keys"), ("cookie", &member), None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    let kid = list["items"][0]["id"].as_str().unwrap();
    let (st, _) = send(&app, "DELETE", &format!("/auth/admin/users/{uid}/api-keys/{kid}"), ("cookie", &admin), None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _) = send(&app, "GET", "/auth/me", ("authorization", &bearer), None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    let (st, _) = send(&app, "DELETE", &format!("/auth/admin/users/{uid}/api-keys/{kid}"), ("cookie", &admin), None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM audit_log WHERE action = 'account.api_key_revoked'").fetch_one(&t.pool).await.unwrap();
    assert_eq!(n, 1);
    t.teardown().await;
}

#[tokio::test]
async fn anonymous_device_starts_stop_at_the_server_wide_cap_and_expired_rows_are_cleared() {
    let Some(t) = TestDb::new().await else { return };
    let app = open_app(t.pool.clone());
    // A full house of open requests, and one that expired long ago.
    sqlx::query(
        "INSERT INTO cli_device_codes (device_code_hash, user_code_hash, device_name, scope, expires_at)
         SELECT sha256(('d' || g)::bytea), sha256(('u' || g)::bytea), 'x', 'import', now() + interval '5 minutes'
         FROM generate_series(1, 500) g",
    )
    .execute(&t.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO cli_device_codes (device_code_hash, user_code_hash, device_name, scope, expires_at)
         VALUES (sha256('old-d'::bytea), sha256('old-u'::bytea), 'old', 'import', now() - interval '1 hour')",
    )
    .execute(&t.pool)
    .await
    .unwrap();
    let res = app
        .clone()
        .oneshot(limited_call("POST", "/auth/cli/device", ("x-none", ""), Some(r#"{"device":"test-laptop"}"#)))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(res.headers().get("retry-after").and_then(|v| v.to_str().ok()), Some("60"));
    let v = json_of(res).await;
    assert_eq!(v["code"], "too_many_login_requests");
    assert!(v["error"].as_str().unwrap().contains("cyanotype login"), "{v}");
    // The expired row went with the attempt.
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM cli_device_codes WHERE device_name = 'old'")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
    // Room again once one expires.
    sqlx::query("UPDATE cli_device_codes SET expires_at = now() - interval '1 minute' WHERE id = (SELECT id FROM cli_device_codes LIMIT 1)")
        .execute(&t.pool)
        .await
        .unwrap();
    let _ = start_device(&app).await;
    t.teardown().await;
}

#[tokio::test]
async fn an_account_cannot_leave_more_than_five_approved_logins_uncollected() {
    let Some(t) = TestDb::new().await else { return };
    let app = open_app(t.pool.clone());
    let alpha = register_cookie(&app, "alpha@example.com").await;
    let bravo = register_cookie(&app, "bravo@example.com").await;

    // Device flow: five approved codes nobody polled for.
    let mut codes = Vec::new();
    for _ in 0..6 {
        codes.push(start_device(&app).await.1);
    }
    for c in &codes[..5] {
        let (st, v) = send(&app, "POST", "/auth/cli/device/approve", ("cookie", &alpha), Some(&json!({"user_code": c}).to_string())).await;
        assert_eq!(st, StatusCode::OK, "{v}");
    }
    let body = json!({"user_code": codes[5]}).to_string();
    let (st, v) = send(&app, "POST", "/auth/cli/device/approve", ("cookie", &alpha), Some(&body)).await;
    assert_eq!((st, v["code"].as_str()), (StatusCode::TOO_MANY_REQUESTS, Some("too_many_login_requests")));
    let (st, _) = send(&app, "POST", "/auth/cli/device/lookup", ("cookie", &alpha), Some(&body)).await;
    assert_eq!(st, StatusCode::TOO_MANY_REQUESTS, "the page says so before asking");
    // The loopback flow counts the same pile.
    let (st, v) = send(&app, "POST", "/auth/cli/authorize/approve", ("cookie", &alpha), Some(&authorize_body(CALLBACK))).await;
    assert_eq!((st, v["code"].as_str()), (StatusCode::TOO_MANY_REQUESTS, Some("too_many_login_requests")));
    // Denying is still allowed, and another account is not affected.
    let (st, _) = send(&app, "POST", "/auth/cli/device/deny", ("cookie", &alpha), Some(&body)).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = send(&app, "POST", "/auth/cli/authorize/approve", ("cookie", &bravo), Some(&authorize_body(CALLBACK))).await;
    assert_eq!(st, StatusCode::OK);

    // One expiring makes room.
    sqlx::query("UPDATE cli_device_codes SET expires_at = now() - interval '1 minute' WHERE id = (SELECT id FROM cli_device_codes WHERE status = 'approved' LIMIT 1)")
        .execute(&t.pool)
        .await
        .unwrap();
    let (st, v) = send(&app, "POST", "/auth/cli/authorize/approve", ("cookie", &alpha), Some(&authorize_body(CALLBACK))).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    t.teardown().await;
}
