// Included at the end of `routes::tests` (it shares that module's helpers). Two-factor authentication, API key scopes and
// expiry on the auth service's own routes. The assets service's scope enforcement is tested in apps/assets.
use crate::totp::{self, TotpKey};

/// The test clock: a mid-step instant, so "the previous step" and "the next one" are both a whole step away.
const NOW: u64 = 1_800_000_015;
const STEP: u64 = totp::STEP_SECS;

fn fixed_clock() -> u64 {
    NOW
}

fn two_factor_app(db: PgPool) -> Router {
    app(db, |c| {
        c.registration = RegistrationMode::Open;
        c.totp_key = Some(TotpKey::new(&[9u8; 32]));
        c.clock = fixed_clock;
    })
}

fn post_as(path: &str, cookie: &str, body: &str) -> Request<Body> {
    call("POST", path, ("cookie", cookie), Some(body))
}

fn pw_login(email: &str) -> Request<Body> {
    post_json(
        "/auth/login",
        &format!(r#"{{"email":"{email}","password":"correct horse battery"}}"#),
        "172.18.0.2",
        None,
    )
}

fn second_step(challenge: &str, code: &str, peer: &str) -> Request<Body> {
    post_json(
        "/auth/login/2fa",
        &format!(r#"{{"challenge":"{challenge}","code":"{code}"}}"#),
        peer,
        None,
    )
}

/// Enrols the account behind `cookie`; returns its secret and recovery codes.
async fn enrol(app: &Router, cookie: &str) -> (Vec<u8>, Vec<String>) {
    let res = app.clone().oneshot(post_as("/auth/2fa/enrol", cookie, "{}")).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let v = json_of(res).await;
    assert!(v["otpauth_uri"].as_str().unwrap().starts_with("otpauth://totp/Cyanotype:"));
    let secret = totp::base32_decode(v["secret"].as_str().unwrap()).unwrap();
    let code = totp::code_at(&secret, NOW);
    let res = app
        .clone()
        .oneshot(post_as("/auth/2fa/confirm", cookie, &format!(r#"{{"code":"{code}"}}"#)))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let v = json_of(res).await;
    let codes: Vec<String> = v["recovery_codes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap().to_string())
        .collect();
    assert_eq!(codes.len(), 10);
    (secret, codes)
}

async fn register(app: &Router, email: &str) -> String {
    register_cookie(app, email).await
}

async fn challenge_for(app: &Router, email: &str) -> String {
    let res = app.clone().oneshot(pw_login(email)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(res.headers().get(header::SET_COOKIE).is_none(), "no session before the code");
    let v = json_of(res).await;
    assert_eq!(v["mfa_required"], true);
    v["challenge"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn enrol_login_with_code_and_with_recovery_code() {
    let Some(t) = TestDb::new().await else { return };
    let app = two_factor_app(t.pool.clone());
    let cookie = register(&app, "a@example.com").await;

    // Before enrolling, login is password-only.
    let st = json_of(app.clone().oneshot(call("GET", "/auth/2fa", ("cookie", &cookie), None)).await.unwrap()).await;
    assert_eq!((st["available"].clone(), st["enabled"].clone()), (json!(true), json!(false)));

    let (secret, codes) = enrol(&app, &cookie).await;
    let me = json_of(app.clone().oneshot(call("GET", "/auth/me", ("cookie", &cookie), None)).await.unwrap()).await;
    assert_eq!(me["two_factor"], true);

    // The secret is not stored in clear anywhere in the row, and recovery codes only as hashes.
    let (blob,): (Vec<u8>,) = sqlx::query_as("SELECT secret_enc FROM user_totp").fetch_one(&t.pool).await.unwrap();
    assert!(!blob.windows(secret.len()).any(|w| w == secret.as_slice()));
    let hashes: Vec<(Vec<u8>,)> = sqlx::query_as("SELECT code_hash FROM totp_recovery_codes").fetch_all(&t.pool).await.unwrap();
    assert_eq!(hashes.len(), 10);
    for (h, c) in hashes.iter().zip(&codes) {
        assert_ne!(h.0, c.as_bytes());
        assert_ne!(h.0, totp::normalise_recovery(c).as_bytes());
    }

    // Password alone no longer gives a session.
    let ch = challenge_for(&app, "a@example.com").await;
    // A wrong code is refused; the right one (the next step, since this one was used to confirm) opens the session.
    let res = app.clone().oneshot(second_step(&ch, "000000", "172.18.0.2")).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(json_of(res).await["code"], "invalid_code");
    let code = totp::code_at(&secret, NOW + STEP);
    let res = app.clone().oneshot(second_step(&ch, &code, "172.18.0.2")).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let session = cookie_of(&res);
    let me = app.clone().oneshot(call("GET", "/auth/me", ("cookie", &session), None)).await.unwrap();
    assert_eq!(me.status(), StatusCode::OK);

    // The challenge is single use.
    let res = app.clone().oneshot(second_step(&ch, &code, "172.18.0.2")).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    // The same code cannot be replayed on a new challenge.
    let ch = challenge_for(&app, "a@example.com").await;
    let res = app.clone().oneshot(second_step(&ch, &code, "172.18.0.2")).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    // A recovery code works once, in any case and spacing; the response says how many are left.
    let typed = codes[0].to_ascii_uppercase().replace('-', " ");
    let res = app.clone().oneshot(second_step(&ch, &typed, "172.18.0.2")).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let v = json_of(res).await;
    assert_eq!((v["recovery_code_used"].clone(), v["recovery_codes_remaining"].clone()), (json!(true), json!(9)));
    let ch = challenge_for(&app, "a@example.com").await;
    let res = app.clone().oneshot(second_step(&ch, &codes[0], "172.18.0.2")).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED, "a recovery code is single use");
    let res = app.clone().oneshot(second_step(&ch, &codes[1], "172.18.0.2")).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    t.teardown().await;
}

#[tokio::test]
async fn an_unconfirmed_enrolment_is_not_two_factor() {
    let Some(t) = TestDb::new().await else { return };
    let app = two_factor_app(t.pool.clone());
    let cookie = register(&app, "a@example.com").await;
    let res = app.clone().oneshot(post_as("/auth/2fa/enrol", &cookie, "{}")).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    // Login still needs only the password: the secret was never proven.
    let res = app.clone().oneshot(pw_login("a@example.com")).await.unwrap();
    assert!(res.headers().get(header::SET_COOKIE).is_some());
    // A wrong confirmation code does not enable it.
    let res = app.clone().oneshot(post_as("/auth/2fa/confirm", &cookie, r#"{"code":"123456"}"#)).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    let st = json_of(app.clone().oneshot(call("GET", "/auth/2fa", ("cookie", &cookie), None)).await.unwrap()).await;
    assert_eq!(st["enabled"], false);
    // Enrolling again replaces the secret (the old QR code no longer works).
    let first: Value = json_of(app.clone().oneshot(post_as("/auth/2fa/enrol", &cookie, "{}")).await.unwrap()).await;
    let second: Value = json_of(app.clone().oneshot(post_as("/auth/2fa/enrol", &cookie, "{}")).await.unwrap()).await;
    assert_ne!(first["secret"], second["secret"]);
    t.teardown().await;
}

#[tokio::test]
async fn disabling_needs_the_password_and_a_code() {
    let Some(t) = TestDb::new().await else { return };
    let app = two_factor_app(t.pool.clone());
    let cookie = register(&app, "a@example.com").await;
    let (secret, _codes) = enrol(&app, &cookie).await;
    let code = totp::code_at(&secret, NOW + STEP);

    // Right code, wrong password.
    let body = format!(r#"{{"password":"nope nope nope","code":"{code}"}}"#);
    let res = app.clone().oneshot(post_as("/auth/2fa/disable", &cookie, &body)).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(json_of(res).await["code"], "wrong_password");
    // Right password, wrong code.
    let res = app
        .clone()
        .oneshot(post_as("/auth/2fa/disable", &cookie, r#"{"password":"correct horse battery","code":"000000"}"#))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    assert!(has_2fa_row(&t.pool).await);
    // Both right.
    let body = format!(r#"{{"password":"correct horse battery","code":"{code}"}}"#);
    let res = app.clone().oneshot(post_as("/auth/2fa/disable", &cookie, &body)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(!has_2fa_row(&t.pool).await);
    let (codes,): (i64,) = sqlx::query_as("SELECT count(*) FROM totp_recovery_codes").fetch_one(&t.pool).await.unwrap();
    assert_eq!(codes, 0);
    // Password-only login again.
    let res = app.clone().oneshot(pw_login("a@example.com")).await.unwrap();
    assert!(res.headers().get(header::SET_COOKIE).is_some());
    t.teardown().await;
}

async fn has_2fa_row(pool: &PgPool) -> bool {
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM user_totp").fetch_one(pool).await.unwrap();
    n > 0
}

#[tokio::test]
async fn regenerating_recovery_codes_retires_the_old_ones() {
    let Some(t) = TestDb::new().await else { return };
    let app = two_factor_app(t.pool.clone());
    let cookie = register(&app, "a@example.com").await;
    let (secret, old) = enrol(&app, &cookie).await;
    let body = format!(
        r#"{{"password":"correct horse battery","code":"{}"}}"#,
        totp::code_at(&secret, NOW + STEP)
    );
    let res = app.clone().oneshot(post_as("/auth/2fa/recovery-codes", &cookie, &body)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let new: Vec<String> = json_of(res).await["recovery_codes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap().to_string())
        .collect();
    assert_eq!(new.len(), 10);
    let ch = challenge_for(&app, "a@example.com").await;
    let res = app.clone().oneshot(second_step(&ch, &old[0], "172.18.0.2")).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    let res = app.clone().oneshot(second_step(&ch, &new[0], "172.18.0.2")).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    t.teardown().await;
}

#[tokio::test]
async fn without_the_server_key_nothing_is_enrolled_and_nothing_fails_open() {
    let Some(t) = TestDb::new().await else { return };
    let with_key = two_factor_app(t.pool.clone());
    let cookie = register(&with_key, "a@example.com").await;
    enrol(&with_key, &cookie).await;
    let other = register(&with_key, "b@example.com").await;

    // The same database, the server started without AUTH_TOTP_KEY.
    let no_key = app(t.pool.clone(), |c| {
        c.registration = RegistrationMode::Open;
        c.clock = fixed_clock;
    });
    let st = json_of(no_key.clone().oneshot(call("GET", "/auth/2fa", ("cookie", &other), None)).await.unwrap()).await;
    assert_eq!(st["available"], false);
    let res = no_key.clone().oneshot(post_as("/auth/2fa/enrol", &other, "{}")).await.unwrap();
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM user_totp WHERE user_id = (SELECT id FROM users WHERE email = 'b@example.com')")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(n, 0, "nothing stored without a key");
    // The account that has 2FA cannot get in without it: the password alone is refused, no session.
    let res = no_key.clone().oneshot(pw_login("a@example.com")).await.unwrap();
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(res.headers().get(header::SET_COOKIE).is_none());
    // A different key cannot open the secrets either: the code is refused and the error says to ask the admin.
    let wrong_key = app(t.pool.clone(), |c| {
        c.registration = RegistrationMode::Open;
        c.totp_key = Some(TotpKey::new(&[1u8; 32]));
        c.clock = fixed_clock;
    });
    let ch = challenge_for(&wrong_key, "a@example.com").await;
    let res = wrong_key.clone().oneshot(second_step(&ch, "123456", "172.18.0.2")).await.unwrap();
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
    t.teardown().await;
}

#[tokio::test]
async fn wrong_codes_lock_the_account_and_the_lock_holds_against_the_right_code() {
    let Some(t) = TestDb::new().await else { return };
    let app = two_factor_app(t.pool.clone());
    let cookie = register(&app, "a@example.com").await;
    let (secret, _) = enrol(&app, &cookie).await;
    let ch = challenge_for(&app, "a@example.com").await;
    // Each attempt from a different address: the per-account lock is what stops this, not the address bucket.
    for i in 0..crate::twofactor::MAX_FAILURES {
        let res = app
            .clone()
            .oneshot(second_step(&ch, "000000", &format!("172.18.1.{}", i + 1)))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED, "attempt {i}");
    }
    let good = totp::code_at(&secret, NOW + STEP);
    let res = app.clone().oneshot(second_step(&ch, &good, "172.18.1.99")).await.unwrap();
    assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(res.headers().get(header::RETRY_AFTER).is_some());
    assert_eq!(json_of(res).await["code"], "two_factor_locked");
    // The lock is also in force for disabling.
    let body = format!(r#"{{"password":"correct horse battery","code":"{good}"}}"#);
    let res = app.clone().oneshot(post_as("/auth/2fa/disable", &cookie, &body)).await.unwrap();
    assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
    // After the lock expires the right code works, and the count starts over.
    sqlx::query("UPDATE user_totp SET locked_until = now() - interval '1 second'")
        .execute(&t.pool)
        .await
        .unwrap();
    let res = app.clone().oneshot(second_step(&ch, &good, "172.18.1.99")).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let (failed,): (i32,) = sqlx::query_as("SELECT failed_attempts FROM user_totp").fetch_one(&t.pool).await.unwrap();
    assert_eq!(failed, 0);
    t.teardown().await;
}

#[tokio::test]
async fn code_attempts_are_limited_per_address() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(t.pool.clone(), |c| {
        c.registration = RegistrationMode::Open;
        c.totp_key = Some(TotpKey::new(&[9u8; 32]));
        c.clock = fixed_clock;
        c.rate_limit.burst = 3;
        c.rate_limit.per_minute = 1;
    });
    // A challenge that does not exist: every attempt costs a token whatever its answer.
    let mut statuses = Vec::new();
    for _ in 0..5 {
        statuses.push(app.clone().oneshot(second_step("nope", "000000", "203.0.113.7")).await.unwrap().status());
    }
    assert_eq!(
        statuses,
        [
            StatusCode::UNAUTHORIZED,
            StatusCode::UNAUTHORIZED,
            StatusCode::UNAUTHORIZED,
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::TOO_MANY_REQUESTS
        ]
    );
    // Another address is unaffected.
    let res = app.clone().oneshot(second_step("nope", "000000", "203.0.113.8")).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    t.teardown().await;
}

#[tokio::test]
async fn an_expired_challenge_is_refused() {
    let Some(t) = TestDb::new().await else { return };
    let app = two_factor_app(t.pool.clone());
    let cookie = register(&app, "a@example.com").await;
    let (secret, _) = enrol(&app, &cookie).await;
    let ch = challenge_for(&app, "a@example.com").await;
    sqlx::query("UPDATE login_challenges SET expires_at = now() - interval '1 second'")
        .execute(&t.pool)
        .await
        .unwrap();
    let res = app
        .clone()
        .oneshot(second_step(&ch, &totp::code_at(&secret, NOW + STEP), "172.18.0.2"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(json_of(res).await["code"], "challenge_expired");
    t.teardown().await;
}

#[tokio::test]
async fn admin_requires_two_factor_and_unenrolled_accounts_can_only_enrol() {
    let Some(t) = TestDb::new().await else { return };
    let app = two_factor_app(t.pool.clone());
    let admin = register(&app, "admin@example.com").await;
    let member = register(&app, "member@example.com").await;
    let member_sess = ("cookie", member.as_str());

    // Members cannot read or change the setting.
    let res = app.clone().oneshot(call("PUT", "/auth/admin/2fa/require", member_sess, Some(r#"{"required":true}"#))).await.unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    // The admin must have 2FA before requiring it.
    let res = app.clone().oneshot(call("PUT", "/auth/admin/2fa/require", ("cookie", &admin), Some(r#"{"required":true}"#))).await.unwrap();
    assert_eq!(res.status(), StatusCode::CONFLICT);
    let (_admin_secret, _) = enrol(&app, &admin).await;
    let res = app.clone().oneshot(call("PUT", "/auth/admin/2fa/require", ("cookie", &admin), Some(r#"{"required":true}"#))).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // The member's live session is now limited to enrolment: /auth/me says so, everything else is refused.
    let me = json_of(app.clone().oneshot(call("GET", "/auth/me", member_sess, None)).await.unwrap()).await;
    assert_eq!(me["mfa_enrol_required"], true);
    let res = app.clone().oneshot(call("GET", "/auth/sessions", member_sess, None)).await.unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    assert_eq!(json_of(res).await["code"], "mfa_enrol_required");
    let res = app.clone().oneshot(call("GET", "/auth/api-keys", member_sess, None)).await.unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    // ... and the admin overview is the admin's alone.
    let res = app.clone().oneshot(call("GET", "/auth/admin/2fa", member_sess, None)).await.unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);

    // A new login of the member is pending too.
    let res = app.clone().oneshot(pw_login("member@example.com")).await.unwrap();
    let fresh = cookie_of(&res);
    assert_eq!(json_of(res).await["mfa_enrol_required"], true);
    let res = app.clone().oneshot(call("GET", "/auth/api-keys", ("cookie", &fresh), None)).await.unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);

    // Enrolling releases every session of the account at once.
    enrol(&app, &member).await;
    for c in [&member, &fresh] {
        let res = app.clone().oneshot(call("GET", "/auth/api-keys", ("cookie", c), None)).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }
    // The admin sees who has it on.
    let v = json_of(app.clone().oneshot(call("GET", "/auth/admin/2fa", ("cookie", &admin), None)).await.unwrap()).await;
    assert_eq!((v["required"].clone(), v["key_configured"].clone(), v["enforced"].clone()), (json!(true), json!(true), json!(true)));
    let users = v["users"].as_array().unwrap();
    assert_eq!(users.len(), 2);
    assert!(users.iter().all(|u| u["enabled"] == true && u["recovery_codes_remaining"] == 10));
    let text = v.to_string();
    assert!(!text.contains("secret"), "the overview carries nothing secret");

    // Switching it off frees accounts without 2FA again.
    let res = app.clone().oneshot(call("PUT", "/auth/admin/2fa/require", ("cookie", &admin), Some(r#"{"required":false}"#))).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(audit_actions(&t.pool).await.contains(&"2fa.require".to_string()));
    assert!(audit_actions(&t.pool).await.contains(&"2fa.unrequire".to_string()));
    t.teardown().await;
}

#[tokio::test]
async fn requirement_is_not_enforced_on_a_server_without_the_key() {
    let Some(t) = TestDb::new().await else { return };
    let keyed = two_factor_app(t.pool.clone());
    let admin = register(&keyed, "admin@example.com").await;
    enrol(&keyed, &admin).await;
    let res = keyed.clone().oneshot(call("PUT", "/auth/admin/2fa/require", ("cookie", &admin), Some(r#"{"required":true}"#))).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    // The key is lost: nobody is locked into an enrolment that cannot succeed.
    let no_key = app(t.pool.clone(), |c| {
        c.registration = RegistrationMode::Open;
        c.clock = fixed_clock;
    });
    let member = register_cookie(&no_key, "member@example.com").await;
    let me = json_of(no_key.clone().oneshot(call("GET", "/auth/me", ("cookie", &member), None)).await.unwrap()).await;
    assert_eq!(me["mfa_enrol_required"], false);
    t.teardown().await;
}

#[tokio::test]
async fn admin_reset_removes_two_factor_signs_out_and_is_audited() {
    let Some(t) = TestDb::new().await else { return };
    let app = two_factor_app(t.pool.clone());
    let admin = register(&app, "admin@example.com").await;
    let member = register(&app, "member@example.com").await;
    enrol(&app, &member).await;
    let (id,): (Uuid,) = sqlx::query_as("SELECT id FROM users WHERE email = 'member@example.com'")
        .fetch_one(&t.pool)
        .await
        .unwrap();

    // A member cannot reset anyone's.
    let res = app.clone().oneshot(post_as(&format!("/auth/admin/users/{id}/2fa/reset"), &member, "{}")).await.unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    let res = app.clone().oneshot(post_as(&format!("/auth/admin/users/{id}/2fa/reset"), &admin, "{}")).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(json_of(res).await["had_2fa"], true);
    assert!(!has_2fa_row(&t.pool).await);
    // Their old session is gone, and the next login is password-only.
    let res = app.clone().oneshot(call("GET", "/auth/me", ("cookie", &member), None)).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    let res = app.clone().oneshot(pw_login("member@example.com")).await.unwrap();
    assert!(res.headers().get(header::SET_COOKIE).is_some());
    // Audited, with the target and without anything secret.
    let (action, target, detail): (String, Option<String>, Value) =
        sqlx::query_as("SELECT action, target_id, detail FROM audit_log WHERE action = 'account.2fa_reset'")
            .fetch_one(&t.pool)
            .await
            .unwrap();
    assert_eq!((action.as_str(), target), ("account.2fa_reset", Some(id.to_string())));
    assert_eq!(detail["email"], "member@example.com");
    // An unknown account is a 404.
    let res = app.clone().oneshot(post_as(&format!("/auth/admin/users/{}/2fa/reset", Uuid::new_v4()), &admin, "{}")).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    t.teardown().await;
}

// ---- API key scopes and expiry (the auth service's own routes) --------------------------------------------------

async fn make_key(app: &Router, cookie: &str, body: &str) -> Value {
    let res = app.clone().oneshot(post_as("/auth/api-keys", cookie, body)).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED, "{body}");
    json_of(res).await
}

fn bearer(key: &str, method: &str, path: &str) -> Request<Body> {
    call(method, path, ("authorization", &format!("Bearer {key}")), None)
}

#[tokio::test]
async fn keys_have_a_scope_and_an_optional_expiry() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(t.pool.clone(), |c| c.registration = RegistrationMode::Open);
    let cookie = register_cookie(&app, "k@example.com").await;

    // No scope given: full, no expiry (what keys were before).
    let k = make_key(&app, &cookie, r#"{"name":"old style"}"#).await;
    assert_eq!((k["scope"].clone(), k["expires_at"].clone()), (json!("full"), Value::Null));
    let k = make_key(&app, &cookie, r#"{"name":"nas","scope":"import","expires_in_days":30}"#).await;
    assert_eq!(k["scope"], "import");
    assert!(k["expires_at"].as_str().is_some());
    let key = k["key"].as_str().unwrap().to_string();

    // Bad scope and bad expiry are refused.
    for body in [
        r#"{"name":"x","scope":"admin"}"#,
        r#"{"name":"x","scope":""}"#,
        r#"{"name":"x","expires_in_days":0}"#,
        r#"{"name":"x","expires_in_days":-3}"#,
        r#"{"name":"x","expires_in_days":99999}"#,
    ] {
        let res = app.clone().oneshot(post_as("/auth/api-keys", &cookie, body)).await.unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST, "{body}");
    }

    // The list shows scope and expiry.
    let list = json_of(app.clone().oneshot(call("GET", "/auth/api-keys", ("cookie", &cookie), None)).await.unwrap()).await;
    let items = list["items"].as_array().unwrap();
    assert!(items.iter().any(|i| i["scope"] == "import" && i["expired"] == false && i["expires_at"].is_string()));
    assert!(items.iter().any(|i| i["scope"] == "full" && i["expires_at"].is_null()));

    // The import key may ask who it is (that is `cyanotype status`), and says what it can do.
    let res = app.clone().oneshot(bearer(&key, "GET", "/auth/me")).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(json_of(res).await["scope"], "import");

    // No key, whatever its scope, mints keys or reaches 2FA or the sessions list: those are session-only.
    for (m, p) in [
        ("POST", "/auth/api-keys"),
        ("GET", "/auth/api-keys"),
        ("GET", "/auth/sessions"),
        ("GET", "/auth/2fa"),
        ("POST", "/auth/2fa/enrol"),
        ("GET", "/auth/admin/2fa"),
        ("GET", "/auth/admin/invites"),
    ] {
        for scope in ["import", "read", "full"] {
            let k = make_key(&app, &cookie, &format!(r#"{{"name":"{scope}","scope":"{scope}"}}"#)).await;
            let res = app.clone().oneshot(bearer(k["key"].as_str().unwrap(), m, p)).await.unwrap();
            assert_eq!(res.status(), StatusCode::FORBIDDEN, "{scope} key on {m} {p}");
        }
    }
    t.teardown().await;
}

#[tokio::test]
async fn an_expired_key_stops_working_and_is_listed_as_expired() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(t.pool.clone(), |c| c.registration = RegistrationMode::Open);
    let cookie = register_cookie(&app, "k@example.com").await;
    let k = make_key(&app, &cookie, r#"{"name":"short","scope":"full","expires_in_days":1}"#).await;
    let key = k["key"].as_str().unwrap();
    assert_eq!(app.clone().oneshot(bearer(key, "GET", "/auth/me")).await.unwrap().status(), StatusCode::OK);
    sqlx::query("UPDATE api_keys SET expires_at = now() - interval '1 second'")
        .execute(&t.pool)
        .await
        .unwrap();
    assert_eq!(app.clone().oneshot(bearer(key, "GET", "/auth/me")).await.unwrap().status(), StatusCode::UNAUTHORIZED);
    let list = json_of(app.clone().oneshot(call("GET", "/auth/api-keys", ("cookie", &cookie), None)).await.unwrap()).await;
    assert_eq!(list["items"][0]["expired"], true);
    // The session still works, and the key can still be revoked.
    let id = list["items"][0]["id"].as_str().unwrap();
    let res = app.clone().oneshot(call("DELETE", &format!("/auth/api-keys/{id}"), ("cookie", &cookie), None)).await.unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    t.teardown().await;
}

#[tokio::test]
async fn migration_turns_existing_keys_into_full_keys_without_expiry() {
    let Some(t) = TestDb::new_at(19).await else { return };
    sqlx::query("INSERT INTO users (email, password_hash) VALUES ('o@example.com', 'x')")
        .execute(&t.pool)
        .await
        .unwrap();
    let key = "cyk_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    sqlx::query("INSERT INTO api_keys (user_id, name, key_hash, prefix) SELECT id, 'cli', $1, 'cyk_aaaaaa' FROM users")
        .bind(cyanotype_service::auth::sha256(key.as_bytes()))
        .execute(&t.pool)
        .await
        .unwrap();
    db::MIGRATOR.run(&t.pool).await.unwrap();
    let (scope, exp): (String, Option<time::OffsetDateTime>) = sqlx::query_as("SELECT scope, expires_at FROM api_keys")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!((scope.as_str(), exp), ("full", None));
    let app = app(t.pool.clone(), |_| {});
    assert_eq!(app.oneshot(bearer(key, "GET", "/auth/me")).await.unwrap().status(), StatusCode::OK);
    t.teardown().await;
}
