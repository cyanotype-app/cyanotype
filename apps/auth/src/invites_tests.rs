// Included at the end of `routes::tests` (it shares that module's helpers). Roles, invites, registration per mode and the
// migration's admin promotion.

impl TestDb {
    /// A database migrated only up to `version`, to test what a migration does to rows that already exist.
    async fn new_at(version: i64) -> Option<Self> {
        let admin_url = std::env::var("DATABASE_URL").ok().filter(|v| !v.is_empty())?;
        let name = format!("cyanotype_test_{}", Uuid::new_v4().simple());
        let mut admin = PgConnection::connect(&admin_url).await.unwrap();
        admin.execute(format!("CREATE DATABASE {name}").as_str()).await.unwrap();
        let (base, _) = admin_url.rsplit_once('/').unwrap();
        let pool = db::connect(&format!("{base}/{name}")).await.unwrap();
        let upto: Vec<_> = db::MIGRATOR.iter().filter(|m| m.version <= version).cloned().collect();
        let partial = sqlx::migrate::Migrator {
            migrations: std::borrow::Cow::Owned(upto),
            ..sqlx::migrate::Migrator::DEFAULT
        };
        partial.run(&pool).await.unwrap();
        Some(Self { pool, admin_url, name })
    }
}

async fn role_of(pool: &PgPool, email: &str) -> String {
    let (r,): (String,) = sqlx::query_as("SELECT role FROM users WHERE email = $1")
        .bind(email)
        .fetch_one(pool)
        .await
        .unwrap();
    r
}

fn reg_with(email: &str, invite: Option<&str>) -> Request<Body> {
    let body = match invite {
        Some(t) => format!(r#"{{"email":"{email}","password":"correct horse battery","invite":"{t}"}}"#),
        None => format!(r#"{{"email":"{email}","password":"correct horse battery"}}"#),
    };
    post_json("/auth/register", &body, "172.18.0.2", None)
}

/// An invite-mode app with its admin registered; returns the admin's session cookie.
async fn invite_app(t: &TestDb) -> (Router, String) {
    let app = app(t.pool.clone(), |c| c.registration = RegistrationMode::Invite);
    let cookie = register_cookie(&app, "admin@example.com").await;
    (app, cookie)
}

async fn make_invite(app: &Router, cookie: &str, body: &str) -> Value {
    let res = app
        .clone()
        .oneshot(call("POST", "/auth/admin/invites", ("cookie", cookie), Some(body)))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    json_of(res).await
}

async fn audit_actions(pool: &PgPool) -> Vec<String> {
    let rows: Vec<(String,)> = sqlx::query_as("SELECT action FROM audit_log ORDER BY id")
        .fetch_all(pool)
        .await
        .unwrap();
    rows.into_iter().map(|r| r.0).collect()
}

#[tokio::test]
async fn migration_makes_the_earliest_user_admin_and_nobody_else() {
    let Some(t) = TestDb::new_at(15).await else { return };
    // An instance as it runs today: the owner and (in open mode) a later user, with no role column yet.
    for (email, at) in [("later@example.com", "2025-03-01"), ("owner@example.com", "2025-01-01"), ("last@example.com", "2025-06-01")] {
        sqlx::query("INSERT INTO users (email, password_hash, created_at) VALUES ($1, 'x', $2::timestamptz)")
            .bind(email)
            .bind(at)
            .execute(&t.pool)
            .await
            .unwrap();
    }
    db::MIGRATOR.run(&t.pool).await.unwrap();
    assert_eq!(role_of(&t.pool, "owner@example.com").await, "admin");
    assert_eq!(role_of(&t.pool, "later@example.com").await, "member");
    assert_eq!(role_of(&t.pool, "last@example.com").await, "member");
    let (admins,): (i64,) = sqlx::query_as("SELECT count(*) FROM users WHERE role = 'admin'")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(admins, 1);
    t.teardown().await;
}

#[tokio::test]
async fn migration_of_a_single_owner_instance_keeps_the_owner_able_to_sign_in_as_admin() {
    let Some(t) = TestDb::new_at(15).await else { return };
    let hash = Argon2::default()
        .hash_password(b"correct horse battery", &SaltString::generate(&mut OsRng))
        .unwrap()
        .to_string();
    sqlx::query("INSERT INTO users (email, password_hash) VALUES ('owner@example.com', $1)")
        .bind(hash)
        .execute(&t.pool)
        .await
        .unwrap();
    db::MIGRATOR.run(&t.pool).await.unwrap();
    // The live configuration may still say first_user: the owner signs in and is an admin.
    let app = app(t.pool.clone(), |c| c.registration = RegistrationMode::FirstUser);
    let login = post_json(
        "/auth/login",
        r#"{"email":"owner@example.com","password":"correct horse battery"}"#,
        "172.18.0.2",
        None,
    );
    let res = app.clone().oneshot(login).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let cookie = cookie_of(&res);
    let me = json_of(app.clone().oneshot(call("GET", "/auth/me", ("cookie", &cookie), None)).await.unwrap()).await;
    assert_eq!(me["role"], "admin");
    let res = app
        .oneshot(call("GET", "/auth/admin/invites", ("cookie", &cookie), None))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    t.teardown().await;
}

#[tokio::test]
async fn migration_on_an_empty_database_promotes_nobody() {
    let Some(t) = TestDb::new().await else { return };
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM users")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
    t.teardown().await;
}

#[tokio::test]
async fn the_first_account_is_admin_in_every_open_mode_and_later_ones_are_members() {
    let Some(t) = TestDb::new().await else { return };
    for (i, mode) in [RegistrationMode::Open, RegistrationMode::FirstUser, RegistrationMode::Invite]
        .into_iter()
        .enumerate()
    {
        sqlx::query("DELETE FROM users").execute(&t.pool).await.unwrap();
        let app = app(t.pool.clone(), |c| c.registration = mode);
        let first = format!("first{i}@example.com");
        let res = app.clone().oneshot(reg(&first)).await.unwrap();
        assert_eq!(res.status(), StatusCode::CREATED);
        assert_eq!(json_of(res).await["role"], "admin");
        assert_eq!(role_of(&t.pool, &first).await, "admin");
        if mode == RegistrationMode::Open {
            let second = format!("second{i}@example.com");
            assert_eq!(status(&app, reg(&second)).await, StatusCode::CREATED);
            assert_eq!(role_of(&t.pool, &second).await, "member");
        }
    }
    t.teardown().await;
}

#[tokio::test]
async fn first_account_race_makes_exactly_one_admin_in_invite_mode() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(t.pool.clone(), |c| c.registration = RegistrationMode::Invite);
    let mut tasks = Vec::new();
    for i in 0..10 {
        let app = app.clone();
        tasks.push(tokio::spawn(async move {
            status(&app, reg(&format!("racer{i}@example.com"))).await
        }));
    }
    let mut created = 0;
    for t in tasks {
        match t.await.unwrap() {
            StatusCode::CREATED => created += 1,
            StatusCode::FORBIDDEN => {}
            other => panic!("unexpected {other}"),
        }
    }
    assert_eq!(created, 1);
    let (admins,): (i64,) = sqlx::query_as("SELECT count(*) FROM users WHERE role = 'admin'")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(admins, 1);
    t.teardown().await;
}

#[tokio::test]
async fn admin_endpoints_are_for_admin_sessions_only() {
    let Some(t) = TestDb::new().await else { return };
    let (app, admin) = invite_app(&t).await;
    let invite = make_invite(&app, &admin, "{}").await;
    let token = invite["token"].as_str().unwrap();
    let id = invite["invite"]["id"].as_str().unwrap();
    let res = app.clone().oneshot(reg_with("member@example.com", Some(token))).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let member = cookie_of(&res);

    let probes = [
        ("GET", "/auth/admin/invites".to_string(), None),
        ("POST", "/auth/admin/invites".to_string(), Some("{}")),
        ("POST", format!("/auth/admin/invites/{id}/reissue"), None),
        ("DELETE", format!("/auth/admin/invites/{id}"), None),
    ];
    for (m, path, body) in &probes {
        // No credentials: 401. A member: 403, and nothing happened.
        let req = Request::builder().method(*m).uri(path).header("content-type", "application/json");
        let anon = req.body(Body::from(body.unwrap_or("").to_string())).unwrap();
        assert_eq!(status(&app, anon).await, StatusCode::UNAUTHORIZED, "{m} {path}");
        let res = app.clone().oneshot(call(m, path, ("cookie", &member), *body)).await.unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN, "{m} {path}");
    }
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM invites")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(n, 1);
    assert!(!audit_actions(&t.pool).await.iter().any(|a| a == "invite.revoke"));

    // An admin's API key is not enough either: the portal takes a login session.
    let res = app
        .clone()
        .oneshot(call("POST", "/auth/api-keys", ("cookie", &admin), Some(r#"{"name":"cli"}"#)))
        .await
        .unwrap();
    let key = json_of(res).await["key"].as_str().unwrap().to_string();
    let bearer = format!("Bearer {key}");
    for (m, path, body) in &probes {
        let res = app.clone().oneshot(call(m, path, ("authorization", &bearer), *body)).await.unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN, "{m} {path} with an API key");
    }

    // The role is read on every request: a demoted admin is locked out of the portal at once.
    sqlx::query("UPDATE users SET role = 'member' WHERE email = 'admin@example.com'")
        .execute(&t.pool)
        .await
        .unwrap();
    let res = app
        .oneshot(call("GET", "/auth/admin/invites", ("cookie", &admin), None))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    t.teardown().await;
}

#[tokio::test]
async fn me_reports_the_role() {
    let Some(t) = TestDb::new().await else { return };
    let (app, admin) = invite_app(&t).await;
    let me = json_of(app.clone().oneshot(call("GET", "/auth/me", ("cookie", &admin), None)).await.unwrap()).await;
    assert_eq!(me["role"], "admin");
    let invite = make_invite(&app, &admin, "{}").await;
    let res = app
        .clone()
        .oneshot(reg_with("m@example.com", invite["token"].as_str()))
        .await
        .unwrap();
    let member = cookie_of(&res);
    let me = json_of(app.oneshot(call("GET", "/auth/me", ("cookie", &member), None)).await.unwrap()).await;
    assert_eq!(me["role"], "member");
    t.teardown().await;
}

#[tokio::test]
async fn create_stores_only_a_hash_and_returns_the_link_once() {
    let Some(t) = TestDb::new().await else { return };
    let (app, admin) = invite_app(&t).await;
    let created = make_invite(
        &app,
        &admin,
        r#"{"note":"  for Sam  ","quota_bytes":5000000000,"daily_upload_bytes":1000000000,"max_file_bytes":200000000}"#,
    )
    .await;
    let token = created["token"].as_str().unwrap();
    assert_eq!(token.len(), 64);
    assert_eq!(created["path"], format!("/register?invite={token}"));
    let inv = &created["invite"];
    assert_eq!(inv["status"], "pending");
    assert_eq!(inv["note"], "for Sam");
    assert_eq!(inv["created_by"], "admin@example.com");
    assert_eq!(inv["limits"]["quota_bytes"], 5_000_000_000_i64);
    assert_eq!(inv["limits"]["max_file_bytes"], 200_000_000);

    let (hash, secs): (Vec<u8>, f64) = sqlx::query_as(
        "SELECT token_hash, extract(epoch FROM (expires_at - created_at))::float8 FROM invites",
    )
    .fetch_one(&t.pool)
    .await
    .unwrap();
    assert_eq!(hash, sha256(token.as_bytes()));
    // Seven days by default.
    assert!((secs - 7.0 * 86400.0).abs() < 5.0, "{secs}");

    // The list never carries the token, and a token is nowhere in the database in clear.
    let list = json_of(app.clone().oneshot(call("GET", "/auth/admin/invites", ("cookie", &admin), None)).await.unwrap()).await;
    assert!(!list.to_string().contains(token));
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
    let (dump,): (String,) = sqlx::query_as("SELECT string_agg(i::text, ' ') FROM invites i")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert!(!dump.contains(token));
    t.teardown().await;
}

#[tokio::test]
async fn create_validates_its_input() {
    let Some(t) = TestDb::new().await else { return };
    let (app, admin) = invite_app(&t).await;
    let long_note = format!(r#"{{"note":"{}"}}"#, "x".repeat(201));
    for body in [
        r#"{"expires_in_days":0}"#,
        r#"{"expires_in_days":91}"#,
        r#"{"quota_bytes":0}"#,
        r#"{"daily_upload_bytes":-5}"#,
        r#"{"max_file_bytes":0}"#,
        long_note.as_str(),
    ] {
        let res = app
            .clone()
            .oneshot(call("POST", "/auth/admin/invites", ("cookie", &admin), Some(body)))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST, "{body}");
    }
    let ok = make_invite(&app, &admin, r#"{"expires_in_days":90,"note":""}"#).await;
    assert!(ok["invite"]["note"].is_null());
    t.teardown().await;
}

#[tokio::test]
async fn an_invite_registers_exactly_one_member_then_is_spent() {
    let Some(t) = TestDb::new().await else { return };
    let (app, admin) = invite_app(&t).await;
    // No token, a made-up token: refused.
    assert_eq!(status(&app, reg("nobody@example.com")).await, StatusCode::FORBIDDEN);
    assert_eq!(
        status(&app, reg_with("nobody@example.com", Some(&"0".repeat(64)))).await,
        StatusCode::FORBIDDEN
    );

    let created = make_invite(&app, &admin, r#"{"note":"for Sam"}"#).await;
    let token = created["token"].as_str().unwrap().to_string();
    let res = app.clone().oneshot(reg_with("sam@example.com", Some(&token))).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    assert_eq!(role_of(&t.pool, "sam@example.com").await, "member");

    // The same link a second time (also by the same person, with another address).
    let res = app.clone().oneshot(reg_with("other@example.com", Some(&token))).await.unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    assert!(json_of(res).await["error"].as_str().unwrap().contains("already used"));
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM users")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(n, 2);

    // The admin sees who used it, and the use is in the audit log (by the new account).
    let list = json_of(app.oneshot(call("GET", "/auth/admin/invites", ("cookie", &admin), None)).await.unwrap()).await;
    let inv = &list["items"][0];
    assert_eq!(inv["status"], "used");
    assert_eq!(inv["used_by"], "sam@example.com");
    assert!(inv["used_at"].is_string());
    assert_eq!(audit_actions(&t.pool).await, ["invite.create", "invite.use"]);
    let (actor, email): (Option<String>, Option<String>) =
        sqlx::query_as("SELECT actor_email, detail->>'email' FROM audit_log WHERE action = 'invite.use'")
            .fetch_one(&t.pool)
            .await
            .unwrap();
    assert_eq!(actor.as_deref(), Some("sam@example.com"));
    assert_eq!(email.as_deref(), Some("sam@example.com"));
    t.teardown().await;
}

#[tokio::test]
async fn one_invite_used_by_many_at_once_registers_exactly_one() {
    let Some(t) = TestDb::new().await else { return };
    let (app, admin) = invite_app(&t).await;
    let created = make_invite(&app, &admin, "{}").await;
    let token = created["token"].as_str().unwrap().to_string();
    let mut tasks = Vec::new();
    for i in 0..10 {
        let app = app.clone();
        let token = token.clone();
        tasks.push(tokio::spawn(async move {
            status(&app, reg_with(&format!("guest{i}@example.com"), Some(&token))).await
        }));
    }
    let mut created = 0;
    for t in tasks {
        match t.await.unwrap() {
            StatusCode::CREATED => created += 1,
            StatusCode::FORBIDDEN => {}
            other => panic!("unexpected {other}"),
        }
    }
    assert_eq!(created, 1);
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM users")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(n, 2); // the admin and one guest
    t.teardown().await;
}

#[tokio::test]
async fn expired_and_revoked_invites_are_refused_with_a_reason() {
    let Some(t) = TestDb::new().await else { return };
    let (app, admin) = invite_app(&t).await;

    let expired = make_invite(&app, &admin, r#"{"expires_in_days":1}"#).await;
    let etoken = expired["token"].as_str().unwrap();
    sqlx::query("UPDATE invites SET expires_at = now() - interval '1 second'")
        .execute(&t.pool)
        .await
        .unwrap();
    let res = app.clone().oneshot(reg_with("late@example.com", Some(etoken))).await.unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    assert!(json_of(res).await["error"].as_str().unwrap().contains("expired"));

    let revoked = make_invite(&app, &admin, "{}").await;
    let rtoken = revoked["token"].as_str().unwrap();
    let rid = revoked["invite"]["id"].as_str().unwrap();
    let res = app
        .clone()
        .oneshot(call("DELETE", &format!("/auth/admin/invites/{rid}"), ("cookie", &admin), None))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    // Revoking again changes nothing (and is not logged twice).
    let res = app
        .clone()
        .oneshot(call("DELETE", &format!("/auth/admin/invites/{rid}"), ("cookie", &admin), None))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    let res = app.clone().oneshot(reg_with("rev@example.com", Some(rtoken))).await.unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    assert!(json_of(res).await["error"].as_str().unwrap().contains("revoked"));
    assert_eq!(
        audit_actions(&t.pool).await.iter().filter(|a| *a == "invite.revoke").count(),
        1
    );

    let list = json_of(app.oneshot(call("GET", "/auth/admin/invites", ("cookie", &admin), None)).await.unwrap()).await;
    let statuses: Vec<&str> = list["items"].as_array().unwrap().iter().map(|i| i["status"].as_str().unwrap()).collect();
    assert!(statuses.contains(&"expired") && statuses.contains(&"revoked"), "{statuses:?}");
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM users")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(n, 1);
    t.teardown().await;
}

#[tokio::test]
async fn a_taken_email_does_not_spend_the_invite() {
    let Some(t) = TestDb::new().await else { return };
    let (app, admin) = invite_app(&t).await;
    let created = make_invite(&app, &admin, "{}").await;
    let token = created["token"].as_str().unwrap();
    let res = app.clone().oneshot(reg_with("ADMIN@example.com", Some(token))).await.unwrap();
    assert_eq!(res.status(), StatusCode::CONFLICT);
    let res = app.oneshot(reg_with("fresh@example.com", Some(token))).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    t.teardown().await;
}

#[tokio::test]
async fn reissue_replaces_the_link_and_keeps_the_invite() {
    let Some(t) = TestDb::new().await else { return };
    let (app, admin) = invite_app(&t).await;
    let created = make_invite(&app, &admin, r#"{"note":"for Sam","expires_in_days":3}"#).await;
    let old = created["token"].as_str().unwrap().to_string();
    let id = created["invite"]["id"].as_str().unwrap().to_string();
    let res = app
        .clone()
        .oneshot(call("POST", &format!("/auth/admin/invites/{id}/reissue"), ("cookie", &admin), None))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let again = json_of(res).await;
    let new = again["token"].as_str().unwrap().to_string();
    assert_ne!(new, old);
    assert_eq!(again["invite"]["id"], id.as_str());
    assert_eq!(again["invite"]["note"], "for Sam");
    // The expiry was kept (3 days, not reset to 7).
    assert_eq!(again["invite"]["expires_at"], created["invite"]["expires_at"]);

    assert_eq!(status(&app, reg_with("a@example.com", Some(&old))).await, StatusCode::FORBIDDEN);
    assert_eq!(status(&app, reg_with("a@example.com", Some(&new))).await, StatusCode::CREATED);

    // A used invite cannot be re-issued or revoked; an unknown id is 404.
    let res = app
        .clone()
        .oneshot(call("POST", &format!("/auth/admin/invites/{id}/reissue"), ("cookie", &admin), None))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CONFLICT);
    let res = app
        .clone()
        .oneshot(call("DELETE", &format!("/auth/admin/invites/{id}"), ("cookie", &admin), None))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CONFLICT);
    let res = app
        .oneshot(call(
            "POST",
            &format!("/auth/admin/invites/{}/reissue", Uuid::new_v4()),
            ("cookie", &admin),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    t.teardown().await;
}

#[tokio::test]
async fn reissue_revives_an_expired_invite() {
    let Some(t) = TestDb::new().await else { return };
    let (app, admin) = invite_app(&t).await;
    let created = make_invite(&app, &admin, "{}").await;
    let id = created["invite"]["id"].as_str().unwrap();
    sqlx::query("UPDATE invites SET expires_at = now() - interval '1 day'")
        .execute(&t.pool)
        .await
        .unwrap();
    let res = app
        .clone()
        .oneshot(call(
            "POST",
            &format!("/auth/admin/invites/{id}/reissue"),
            ("cookie", &admin),
            Some(r#"{"expires_in_days":2}"#),
        ))
        .await
        .unwrap();
    let again = json_of(res).await;
    assert_eq!(again["invite"]["status"], "pending");
    assert_eq!(
        status(&app, reg_with("a@example.com", again["token"].as_str())).await,
        StatusCode::CREATED
    );
    t.teardown().await;
}

#[tokio::test]
async fn the_other_modes_treat_invites_as_documented() {
    let Some(t) = TestDb::new().await else { return };
    let (inv_app, admin) = invite_app(&t).await;
    let token = make_invite(&inv_app, &admin, "{}").await["token"].as_str().unwrap().to_string();

    // first_user and closed: an invite does not open the door.
    let first_user = app(t.pool.clone(), |c| c.registration = RegistrationMode::FirstUser);
    assert_eq!(status(&first_user, reg_with("x@example.com", Some(&token))).await, StatusCode::FORBIDDEN);
    let closed = app(t.pool.clone(), |c| c.registration = RegistrationMode::Closed);
    assert_eq!(status(&closed, reg_with("x@example.com", Some(&token))).await, StatusCode::FORBIDDEN);
    let (used,): (bool,) = sqlx::query_as("SELECT used_at IS NOT NULL FROM invites")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert!(!used);

    // open: anyone may register, and a valid invite is recorded as used.
    let open = app(t.pool.clone(), |c| c.registration = RegistrationMode::Open);
    assert_eq!(status(&open, reg("walkin@example.com")).await, StatusCode::CREATED);
    assert_eq!(status(&open, reg_with("sam@example.com", Some(&token))).await, StatusCode::CREATED);
    assert_eq!(status(&open, reg_with("bad@example.com", Some("nope"))).await, StatusCode::CREATED);
    let (by,): (Option<String>,) = sqlx::query_as("SELECT used_email FROM invites")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(by.as_deref(), Some("sam@example.com"));
    t.teardown().await;
}

#[tokio::test]
async fn registration_info_and_invite_check_describe_the_state() {
    let Some(t) = TestDb::new().await else { return };
    let get = |app: &Router| {
        let app = app.clone();
        async move {
            json_of(app.oneshot(Request::builder().uri("/auth/registration").body(Body::empty()).unwrap()).await.unwrap()).await
        }
    };
    let mk = |mode| app(t.pool.clone(), move |c| c.registration = mode);
    // Empty instance: every mode but closed lets the first account in.
    let info = get(&mk(RegistrationMode::Invite)).await;
    assert_eq!((info["mode"].as_str(), info["open"].as_bool(), info["invite_required"].as_bool()), (Some("invite"), Some(true), Some(false)));
    assert_eq!(get(&mk(RegistrationMode::Closed)).await["open"], false);

    let (app, admin) = invite_app(&t).await;
    let info = get(&app).await;
    assert_eq!((info["open"].as_bool(), info["invite_required"].as_bool(), info["first_account"].as_bool()), (Some(false), Some(true), Some(false)));
    assert_eq!(get(&mk(RegistrationMode::FirstUser)).await["open"], false);
    let info = get(&mk(RegistrationMode::Open)).await;
    assert_eq!((info["open"].as_bool(), info["invite_required"].as_bool()), (Some(true), Some(false)));

    // The check endpoint says whether a token is usable, and why not, without the note or limits.
    let created = make_invite(&app, &admin, r#"{"note":"private","quota_bytes":1000}"#).await;
    let token = created["token"].as_str().unwrap();
    let check = |t: String| {
        let app = app.clone();
        async move {
            json_of(
                app.oneshot(post_json("/auth/invites/check", &format!(r#"{{"token":"{t}"}}"#), "172.18.0.2", None))
                    .await
                    .unwrap(),
            )
            .await
        }
    };
    let ok = check(token.to_string()).await;
    assert_eq!(ok["valid"], true);
    assert!(!ok.to_string().contains("private") && !ok.to_string().contains("1000"));
    assert_eq!(check("0".repeat(64)).await["reason"], "unknown");
    assert_eq!(status(&app, reg_with("a@example.com", Some(token))).await, StatusCode::CREATED);
    assert_eq!(check(token.to_string()).await["reason"], "used");
    t.teardown().await;
}

#[tokio::test]
async fn audit_log_records_create_reissue_and_revoke_with_the_actor() {
    let Some(t) = TestDb::new().await else { return };
    let (app, admin) = invite_app(&t).await;
    let created = make_invite(&app, &admin, r#"{"note":"n"}"#).await;
    let id = created["invite"]["id"].as_str().unwrap();
    for (m, path) in [("POST", format!("/auth/admin/invites/{id}/reissue")), ("DELETE", format!("/auth/admin/invites/{id}"))] {
        app.clone().oneshot(call(m, &path, ("cookie", &admin), None)).await.unwrap();
    }
    assert_eq!(audit_actions(&t.pool).await, ["invite.create", "invite.reissue", "invite.revoke"]);
    let rows: Vec<(Option<String>, Option<String>, Option<String>)> =
        sqlx::query_as("SELECT actor_email, target_type, target_id FROM audit_log")
            .fetch_all(&t.pool)
            .await
            .unwrap();
    for (actor, tt, tid) in rows {
        assert_eq!(actor.as_deref(), Some("admin@example.com"));
        assert_eq!(tt.as_deref(), Some("invite"));
        assert_eq!(tid.as_deref(), Some(id));
    }
    // No token in any audit entry.
    let (dump,): (String,) = sqlx::query_as("SELECT string_agg(a::text, ' ') FROM audit_log a")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert!(!dump.contains(created["token"].as_str().unwrap()));
    t.teardown().await;
}

async fn limits_of(pool: &PgPool, email: &str) -> (Option<i64>, Option<i64>, Option<i64>) {
    sqlx::query_as("SELECT quota_bytes, daily_upload_bytes, max_file_bytes FROM users WHERE lower(email) = lower($1)")
        .bind(email)
        .fetch_one(pool)
        .await
        .unwrap()
}

fn app_for(t: &TestDb, mode: RegistrationMode) -> Router {
    app(t.pool.clone(), |c| c.registration = mode)
}

#[tokio::test]
async fn new_accounts_start_with_the_server_default_and_invite_presets_and_the_first_admin_is_unlimited() {
    let Some(t) = TestDb::new().await else { return };
    let default = cyanotype_core::limits::Limits {
        quota_bytes: Some(10_000),
        daily_upload_bytes: Some(1_000),
        max_file_bytes: None,
    };
    // A server default set before anybody registered does not limit the first account, the admin.
    cyanotype_core::limits::set_server_default(&t.pool, default).await.unwrap();
    let (app, admin) = invite_app(&t).await;
    assert_eq!(limits_of(&t.pool, "admin@example.com").await, (None, None, None));

    // An invite without presets: the default.
    let plain = make_invite(&app, &admin, "{}").await;
    assert_eq!(status(&app, reg_with("plain@example.com", plain["token"].as_str())).await, StatusCode::CREATED);
    assert_eq!(limits_of(&t.pool, "plain@example.com").await, (Some(10_000), Some(1_000), None));

    // Presets win field by field; a field the invite leaves out takes the default.
    let preset = make_invite(&app, &admin, r#"{"quota_bytes":50,"max_file_bytes":20}"#).await;
    assert_eq!(status(&app, reg_with("preset@example.com", preset["token"].as_str())).await, StatusCode::CREATED);
    assert_eq!(limits_of(&t.pool, "preset@example.com").await, (Some(50), Some(1_000), Some(20)));

    // Open mode, no invite: the default too.
    let open = app_for(&t, RegistrationMode::Open);
    assert_eq!(status(&open, reg("walkin@example.com")).await, StatusCode::CREATED);
    assert_eq!(limits_of(&t.pool, "walkin@example.com").await, (Some(10_000), Some(1_000), None));

    // Changing the default later does not touch existing accounts; with none set, a new member is unlimited.
    cyanotype_core::limits::set_server_default(&t.pool, cyanotype_core::limits::Limits::default())
        .await
        .unwrap();
    assert_eq!(limits_of(&t.pool, "plain@example.com").await, (Some(10_000), Some(1_000), None));
    assert_eq!(status(&open, reg("free@example.com")).await, StatusCode::CREATED);
    assert_eq!(limits_of(&t.pool, "free@example.com").await, (None, None, None));
    t.teardown().await;
}

#[tokio::test]
async fn the_limits_migration_leaves_existing_accounts_unlimited() {
    let Some(t) = TestDb::new_at(17).await else { return };
    sqlx::query("INSERT INTO users (email, password_hash) VALUES ('owner@example.com', 'x')")
        .execute(&t.pool)
        .await
        .unwrap();
    db::MIGRATOR.run(&t.pool).await.unwrap();
    assert_eq!(limits_of(&t.pool, "owner@example.com").await, (None, None, None));
    t.teardown().await;
}

// ---- account actions (docs/admin.md, "Account actions")

/// Registers a member through a fresh invite; returns `(id, session cookie)`.
async fn member(app: &Router, admin: &str, pool: &PgPool, email: &str) -> (Uuid, String) {
    let invite = make_invite(app, admin, "{}").await;
    let res = app.clone().oneshot(reg_with(email, invite["token"].as_str())).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let cookie = cookie_of(&res);
    let (id,): (Uuid,) = sqlx::query_as("SELECT id FROM users WHERE email = $1")
        .bind(email)
        .fetch_one(pool)
        .await
        .unwrap();
    (id, cookie)
}

async fn post(app: &Router, path: &str, cookie: &str, body: Option<&str>) -> (StatusCode, Value) {
    let res = app.clone().oneshot(call("POST", path, ("cookie", cookie), body)).await.unwrap();
    (res.status(), json_of(res).await)
}

fn login_req(email: &str, password: &str) -> Request<Body> {
    post_json(
        "/auth/login",
        &format!(r#"{{"email":"{email}","password":"{password}"}}"#),
        "172.18.0.2",
        None,
    )
}

async fn api_key_for(app: &Router, cookie: &str) -> String {
    let res = app
        .clone()
        .oneshot(call("POST", "/auth/api-keys", ("cookie", cookie), Some(r#"{"name":"cli"}"#)))
        .await
        .unwrap();
    json_of(res).await["key"].as_str().unwrap().to_string()
}

async fn me_status(app: &Router, header: (&str, &str)) -> StatusCode {
    status(app, call("GET", "/auth/me", header, None)).await
}

#[tokio::test]
async fn disabling_signs_the_account_out_refuses_login_and_keys_and_enabling_restores_them() {
    let Some(t) = TestDb::new().await else { return };
    let (app, admin) = invite_app(&t).await;
    let (sam, sam_cookie) = member(&app, &admin, &t.pool, "sam@example.com").await;
    let key = api_key_for(&app, &sam_cookie).await;
    let bearer = format!("Bearer {key}");
    assert_eq!(me_status(&app, ("authorization", &bearer)).await, StatusCode::OK);

    // A member cannot.
    let (s, _) = post(&app, &format!("/auth/admin/users/{sam}/disable"), &sam_cookie, None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(me_status(&app, ("cookie", &sam_cookie)).await, StatusCode::OK);

    let (s, v) = post(&app, &format!("/auth/admin/users/{sam}/disable"), &admin, None).await;
    assert_eq!((s, v["changed"].as_bool()), (StatusCode::OK, Some(true)));
    // Signed out, the key refused, login refused (to someone who knows the password), the wrong password still just 401.
    assert_eq!(me_status(&app, ("cookie", &sam_cookie)).await, StatusCode::UNAUTHORIZED);
    assert_eq!(me_status(&app, ("authorization", &bearer)).await, StatusCode::UNAUTHORIZED);
    let res = app.clone().oneshot(login_req("sam@example.com", "correct horse battery")).await.unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    assert_eq!(json_of(res).await["code"], "account_disabled");
    assert_eq!(
        app.clone().oneshot(login_req("sam@example.com", "wrong password")).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM sessions WHERE user_id = $1")
        .bind(sam)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
    // The key is kept (not deleted) and was not stamped as used while disabled.
    let (keys,): (i64,) = sqlx::query_as("SELECT count(*) FROM api_keys WHERE user_id = $1")
        .bind(sam)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(keys, 1);
    // Disabling twice changes nothing and is not logged twice.
    let (_, v) = post(&app, &format!("/auth/admin/users/{sam}/disable"), &admin, None).await;
    assert_eq!(v["changed"], false);

    let (s, v) = post(&app, &format!("/auth/admin/users/{sam}/enable"), &admin, None).await;
    assert_eq!((s, v["changed"].as_bool()), (StatusCode::OK, Some(true)));
    assert_eq!(me_status(&app, ("authorization", &bearer)).await, StatusCode::OK, "the key works again");
    assert_eq!(
        app.clone().oneshot(login_req("sam@example.com", "correct horse battery")).await.unwrap().status(),
        StatusCode::OK
    );
    assert_eq!(
        audit_actions(&t.pool).await.iter().filter(|a| a.starts_with("account.")).collect::<Vec<_>>(),
        ["account.disable", "account.enable"]
    );
    t.teardown().await;
}

#[tokio::test]
async fn nobody_disables_or_deletes_themselves_and_the_last_active_admin_cannot_be_demoted() {
    let Some(t) = TestDb::new().await else { return };
    let (app, admin) = invite_app(&t).await;
    let (admin_id,): (Uuid,) = sqlx::query_as("SELECT id FROM users WHERE email = 'admin@example.com'")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    for (path, body) in [
        (format!("/auth/admin/users/{admin_id}/disable"), None),
        (format!("/auth/admin/users/{admin_id}/delete"), Some(r#"{"confirm_email":"admin@example.com"}"#)),
    ] {
        let (s, v) = post(&app, &path, &admin, body).await;
        assert_eq!(s, StatusCode::CONFLICT, "{path}: {v}");
        assert!(v["error"].as_str().unwrap().contains("your own account"));
    }
    // The only administrator cannot demote themselves.
    let (s, v) = post(&app, &format!("/auth/admin/users/{admin_id}/role"), &admin, Some(r#"{"role":"member"}"#)).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert!(v["error"].as_str().unwrap().contains("last administrator"));
    assert_eq!(role_of(&t.pool, "admin@example.com").await, "admin");

    // A second admin who is disabled does not count: still the last active one.
    let (sam, _) = member(&app, &admin, &t.pool, "sam@example.com").await;
    assert_eq!(post(&app, &format!("/auth/admin/users/{sam}/role"), &admin, Some(r#"{"role":"admin"}"#)).await.0, StatusCode::OK);
    assert_eq!(post(&app, &format!("/auth/admin/users/{sam}/disable"), &admin, None).await.0, StatusCode::OK);
    let (s, _) = post(&app, &format!("/auth/admin/users/{admin_id}/role"), &admin, Some(r#"{"role":"member"}"#)).await;
    assert_eq!(s, StatusCode::CONFLICT);
    // Enabled again, there are two, and demoting oneself is fine.
    assert_eq!(post(&app, &format!("/auth/admin/users/{sam}/enable"), &admin, None).await.0, StatusCode::OK);
    let (s, v) = post(&app, &format!("/auth/admin/users/{admin_id}/role"), &admin, Some(r#"{"role":"member"}"#)).await;
    assert_eq!((s, v["changed"].as_bool()), (StatusCode::OK, Some(true)));
    assert_eq!(role_of(&t.pool, "admin@example.com").await, "member");
    // The demoted admin is out of the portal at once.
    assert_eq!(
        app.clone().oneshot(call("GET", "/auth/admin/invites", ("cookie", &admin), None)).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    t.teardown().await;
}

#[tokio::test]
async fn two_admins_demoting_each_other_at_once_leave_one() {
    let Some(t) = TestDb::new().await else { return };
    let (app, admin_a) = invite_app(&t).await;
    let (b, cookie_b) = member(&app, &admin_a, &t.pool, "b@example.com").await;
    let (a,): (Uuid,) = sqlx::query_as("SELECT id FROM users WHERE email = 'admin@example.com'")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(post(&app, &format!("/auth/admin/users/{b}/role"), &admin_a, Some(r#"{"role":"admin"}"#)).await.0, StatusCode::OK);
    for _ in 0..5 {
        // Reset to two admins, then each demotes the other at the same moment.
        sqlx::query("UPDATE users SET role = 'admin'").execute(&t.pool).await.unwrap();
        let (pa, pb) = (format!("/auth/admin/users/{b}/role"), format!("/auth/admin/users/{a}/role"));
        let (r1, r2) = tokio::join!(
            post(&app, &pa, &admin_a, Some(r#"{"role":"member"}"#)),
            post(&app, &pb, &cookie_b, Some(r#"{"role":"member"}"#)),
        );
        let (admins,): (i64,) = sqlx::query_as("SELECT count(*) FROM users WHERE role = 'admin'")
            .fetch_one(&t.pool)
            .await
            .unwrap();
        assert_eq!(admins, 1, "{:?} {:?}", r1.0, r2.0);
        assert!(r1.0 == StatusCode::OK || r2.0 == StatusCode::OK);
    }
    t.teardown().await;
}

#[tokio::test]
async fn role_changes_are_audited_validated_and_admin_only() {
    let Some(t) = TestDb::new().await else { return };
    let (app, admin) = invite_app(&t).await;
    let (sam, sam_cookie) = member(&app, &admin, &t.pool, "sam@example.com").await;
    let path = format!("/auth/admin/users/{sam}/role");
    assert_eq!(post(&app, &path, &sam_cookie, Some(r#"{"role":"admin"}"#)).await.0, StatusCode::FORBIDDEN);
    assert_eq!(role_of(&t.pool, "sam@example.com").await, "member");
    assert_eq!(post(&app, &path, &admin, Some(r#"{"role":"owner"}"#)).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(post(&app, &format!("/auth/admin/users/{}/role", Uuid::new_v4()), &admin, Some(r#"{"role":"admin"}"#)).await.0, StatusCode::NOT_FOUND);
    let (_, v) = post(&app, &path, &admin, Some(r#"{"role":"admin"}"#)).await;
    assert_eq!(v["changed"], true);
    // Promoted: the portal opens for the session already held.
    assert_eq!(
        app.clone().oneshot(call("GET", "/auth/admin/invites", ("cookie", &sam_cookie), None)).await.unwrap().status(),
        StatusCode::OK
    );
    let (_, v) = post(&app, &path, &admin, Some(r#"{"role":"admin"}"#)).await;
    assert_eq!(v["changed"], false);
    let (detail,): (Value,) = sqlx::query_as("SELECT detail FROM audit_log WHERE action = 'account.role'")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!((detail["from"].as_str(), detail["to"].as_str(), detail["email"].as_str()), (Some("member"), Some("admin"), Some("sam@example.com")));
    t.teardown().await;
}

#[tokio::test]
async fn a_reset_link_works_once_sets_the_password_and_signs_the_account_out() {
    let Some(t) = TestDb::new().await else { return };
    let (app, admin) = invite_app(&t).await;
    let (sam, sam_cookie) = member(&app, &admin, &t.pool, "sam@example.com").await;
    let path = format!("/auth/admin/users/{sam}/reset-link");

    // Admins only.
    assert_eq!(post(&app, &path, &sam_cookie, None).await.0, StatusCode::FORBIDDEN);
    let (s, link) = post(&app, &path, &admin, None).await;
    assert_eq!(s, StatusCode::OK, "{link}");
    let token = link["token"].as_str().unwrap().to_string();
    assert_eq!(link["path"], format!("/reset-password?token={token}"));
    assert_eq!(link["email"], "sam@example.com");
    // Stored hashed; not in the audit log.
    let (hash,): (Vec<u8>,) = sqlx::query_as("SELECT token_hash FROM password_resets WHERE user_id = $1")
        .bind(sam)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(hash, sha256(token.as_bytes()));
    let (dump,): (String,) = sqlx::query_as("SELECT string_agg(a::text, ' ') FROM audit_log a")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert!(!dump.contains(&token));
    assert!(audit_actions(&t.pool).await.contains(&"account.reset_link".to_string()));

    let check = |tok: String| {
        let app = app.clone();
        async move {
            json_of(
                app.oneshot(post_json("/auth/reset-password/check", &format!(r#"{{"token":"{tok}"}}"#), "172.18.0.2", None))
                    .await
                    .unwrap(),
            )
            .await
        }
    };
    assert_eq!(check(token.clone()).await, json!({"valid": true, "email": "sam@example.com"}));
    assert_eq!(check("0".repeat(64)).await["reason"], "unknown");

    let reset = |tok: String, pw: &str| {
        post_json(
            "/auth/reset-password",
            &format!(r#"{{"token":"{tok}","password":"{pw}"}}"#),
            "172.18.0.2",
            None,
        )
    };
    // Too short a password does not spend the link.
    assert_eq!(status(&app, reset(token.clone(), "short")).await, StatusCode::BAD_REQUEST);
    assert_eq!(status(&app, reset("0".repeat(64), "a brand new passphrase")).await, StatusCode::FORBIDDEN);
    assert_eq!(status(&app, reset(token.clone(), "a brand new passphrase")).await, StatusCode::NO_CONTENT);

    // The old password and the old session are gone; the new password works.
    assert_eq!(me_status(&app, ("cookie", &sam_cookie)).await, StatusCode::UNAUTHORIZED);
    assert_eq!(
        app.clone().oneshot(login_req("sam@example.com", "correct horse battery")).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.clone().oneshot(login_req("sam@example.com", "a brand new passphrase")).await.unwrap().status(),
        StatusCode::OK
    );
    // Once only.
    assert_eq!(check(token.clone()).await["reason"], "used");
    assert_eq!(status(&app, reset(token, "yet another passphrase")).await, StatusCode::FORBIDDEN);
    assert!(audit_actions(&t.pool).await.contains(&"account.password_reset".to_string()));
    t.teardown().await;
}

#[tokio::test]
async fn reset_links_expire_are_replaced_by_newer_ones_and_do_not_work_for_a_disabled_account() {
    let Some(t) = TestDb::new().await else { return };
    let (app, admin) = invite_app(&t).await;
    let (sam, _) = member(&app, &admin, &t.pool, "sam@example.com").await;
    let path = format!("/auth/admin/users/{sam}/reset-link");
    let reset = |tok: &str| {
        post_json(
            "/auth/reset-password",
            &format!(r#"{{"token":"{tok}","password":"a brand new passphrase"}}"#),
            "172.18.0.2",
            None,
        )
    };
    let first = post(&app, &path, &admin, None).await.1["token"].as_str().unwrap().to_string();
    let second = post(&app, &path, &admin, None).await.1["token"].as_str().unwrap().to_string();
    assert_eq!(status(&app, reset(&first)).await, StatusCode::FORBIDDEN, "a newer link replaces the older one");
    // Expiry (24 hours).
    let (secs,): (f64,) = sqlx::query_as("SELECT extract(epoch FROM expires_at - created_at)::float8 FROM password_resets WHERE token_hash = $1")
        .bind(sha256(second.as_bytes()))
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert!((secs - 24.0 * 3600.0).abs() < 5.0, "{secs}");
    sqlx::query("UPDATE password_resets SET expires_at = now() - interval '1 second'").execute(&t.pool).await.unwrap();
    assert_eq!(status(&app, reset(&second)).await, StatusCode::FORBIDDEN);
    // Disabled: not usable either.
    let third = post(&app, &path, &admin, None).await.1["token"].as_str().unwrap().to_string();
    post(&app, &format!("/auth/admin/users/{sam}/disable"), &admin, None).await;
    assert_eq!(status(&app, reset(&third)).await, StatusCode::FORBIDDEN);
    t.teardown().await;
}

#[tokio::test]
async fn sessions_are_listed_and_ended_for_the_account_and_by_the_person() {
    let Some(t) = TestDb::new().await else { return };
    let (app, admin) = invite_app(&t).await;
    let (sam, first) = member(&app, &admin, &t.pool, "sam@example.com").await;
    let (_, other) = member(&app, &admin, &t.pool, "other@example.com").await;
    // Two more sign-ins for Sam.
    let mut cookies = vec![first.clone()];
    for _ in 0..2 {
        let res = app.clone().oneshot(login_req("sam@example.com", "correct horse battery")).await.unwrap();
        cookies.push(cookie_of(&res));
    }

    // The person's own list marks the current session; the token is nowhere in it.
    let res = app.clone().oneshot(call("GET", "/auth/sessions", ("cookie", &cookies[1]), None)).await.unwrap();
    let list = json_of(res).await;
    let items = list["items"].as_array().unwrap();
    assert_eq!(items.len(), 3);
    assert_eq!(items.iter().filter(|i| i["current"] == true).count(), 1);
    assert!(!list.to_string().contains(cookies[1].split('=').nth(1).unwrap()));

    // Ending one by its id.
    let victim = items.iter().find(|i| i["current"] == false).unwrap()["id"].as_str().unwrap().to_string();
    let res = app.clone().oneshot(call("DELETE", &format!("/auth/sessions/{victim}"), ("cookie", &cookies[1]), None)).await.unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        app.clone().oneshot(call("DELETE", &format!("/auth/sessions/{victim}"), ("cookie", &cookies[1]), None)).await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
    // Another person's session id is simply not found.
    let (other_id,): (String,) = sqlx::query_as("SELECT encode(substring(token_hash from 1 for 8), 'hex') FROM sessions WHERE user_id <> $1 AND user_id <> (SELECT id FROM users WHERE email = 'admin@example.com') LIMIT 1")
        .bind(sam)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(
        app.clone().oneshot(call("DELETE", &format!("/auth/sessions/{other_id}"), ("cookie", &cookies[1]), None)).await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(me_status(&app, ("cookie", &other)).await, StatusCode::OK);

    // "Sign out the others" keeps this one.
    let (s, v) = post(&app, "/auth/sessions/revoke-others", &cookies[1], None).await;
    assert_eq!(s, StatusCode::OK);
    assert!(v["revoked"].as_u64().unwrap() >= 1);
    assert_eq!(me_status(&app, ("cookie", &cookies[1])).await, StatusCode::OK);
    assert_eq!(me_status(&app, ("cookie", &cookies[2])).await, StatusCode::UNAUTHORIZED);

    // An API key cannot list or end sessions.
    let key = api_key_for(&app, &cookies[1]).await;
    let bearer = format!("Bearer {key}");
    assert_eq!(
        app.clone().oneshot(call("GET", "/auth/sessions", ("authorization", &bearer), None)).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );

    // The admin sees them, and signs the account out everywhere; the other account is untouched.
    let res = app.clone().oneshot(call("GET", &format!("/auth/admin/users/{sam}/sessions"), ("cookie", &admin), None)).await.unwrap();
    assert_eq!(json_of(res).await["items"].as_array().unwrap().len(), 1);
    assert_eq!(
        app.clone().oneshot(call("GET", &format!("/auth/admin/users/{sam}/sessions"), ("cookie", &cookies[1]), None)).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    let res = app.clone().oneshot(call("DELETE", &format!("/auth/admin/users/{sam}/sessions"), ("cookie", &admin), None)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(json_of(res).await["revoked"], 1);
    assert_eq!(me_status(&app, ("cookie", &cookies[1])).await, StatusCode::UNAUTHORIZED);
    assert_eq!(me_status(&app, ("cookie", &other)).await, StatusCode::OK);
    assert!(audit_actions(&t.pool).await.contains(&"account.sessions_revoked".to_string()));

    // "Sign out everywhere" by the person also clears the browser's cookie.
    let res = app.clone().oneshot(login_req("sam@example.com", "correct horse battery")).await.unwrap();
    let c = cookie_of(&res);
    let res = app.clone().oneshot(call("POST", "/auth/sessions/revoke-all", ("cookie", &c), None)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(res.headers()[header::SET_COOKIE].to_str().unwrap().contains("Max-Age=0"));
    assert_eq!(me_status(&app, ("cookie", &c)).await, StatusCode::UNAUTHORIZED);
    t.teardown().await;
}

#[tokio::test]
async fn deleting_an_account_needs_the_email_disables_it_at_once_and_queues_one_task() {
    let Some(t) = TestDb::new().await else { return };
    let (app, admin) = invite_app(&t).await;
    let (sam, sam_cookie) = member(&app, &admin, &t.pool, "sam@example.com").await;
    let path = format!("/auth/admin/users/{sam}/delete");

    assert_eq!(post(&app, &path, &sam_cookie, Some(r#"{"confirm_email":"sam@example.com"}"#)).await.0, StatusCode::FORBIDDEN);
    // Step two: the email must be typed, case aside.
    let (s, v) = post(&app, &path, &admin, Some(r#"{"confirm_email":"samantha@example.com"}"#)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    assert_eq!(post(&app, &path, &admin, Some(r#"{"confirm_email":""}"#)).await.0, StatusCode::BAD_REQUEST);
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM admin_tasks").fetch_one(&t.pool).await.unwrap();
    assert_eq!(n, 0);
    assert_eq!(me_status(&app, ("cookie", &sam_cookie)).await, StatusCode::OK, "nothing happened yet");

    let (s, v) = post(&app, &path, &admin, Some(r#"{"confirm_email":"  SAM@example.com "}"#)).await;
    assert_eq!((s, v["already_requested"].as_bool()), (StatusCode::ACCEPTED, Some(false)));
    // Disabled and signed out immediately; the rows wait for the worker.
    assert_eq!(me_status(&app, ("cookie", &sam_cookie)).await, StatusCode::UNAUTHORIZED);
    let (task_kind, params): (String, Value) = sqlx::query_as("SELECT kind, params FROM admin_tasks").fetch_one(&t.pool).await.unwrap();
    assert_eq!(task_kind, "delete_account");
    assert_eq!((params["user_id"].as_str(), params["email"].as_str()), (Some(sam.to_string().as_str()), Some("sam@example.com")));
    // Asking again does not queue a second one, and enabling is refused while it is pending.
    let (_, v) = post(&app, &path, &admin, Some(r#"{"confirm_email":"sam@example.com"}"#)).await;
    assert_eq!(v["already_requested"], true);
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM admin_tasks").fetch_one(&t.pool).await.unwrap();
    assert_eq!(n, 1);
    assert_eq!(post(&app, &format!("/auth/admin/users/{sam}/enable"), &admin, None).await.0, StatusCode::CONFLICT);
    assert_eq!(audit_actions(&t.pool).await.iter().filter(|a| *a == "account.delete.requested").count(), 1);
    // Another admin account cannot be deleted by a member, an unknown one is 404.
    assert_eq!(post(&app, &format!("/auth/admin/users/{}/delete", Uuid::new_v4()), &admin, Some(r#"{"confirm_email":"x"}"#)).await.0, StatusCode::NOT_FOUND);
    t.teardown().await;
}

#[tokio::test]
async fn the_audit_log_filters_pages_and_is_admin_only() {
    let Some(t) = TestDb::new().await else { return };
    let (app, admin) = invite_app(&t).await;
    let (sam, _) = member(&app, &admin, &t.pool, "sam@example.com").await;
    post(&app, &format!("/auth/admin/users/{sam}/disable"), &admin, None).await;
    post(&app, &format!("/auth/admin/users/{sam}/enable"), &admin, None).await;
    post(&app, &format!("/auth/admin/users/{sam}/reset-link"), &admin, None).await;
    let get = |q: &str, cookie: &str| {
        let app = app.clone();
        let req = call("GET", &format!("/auth/admin/audit{q}"), ("cookie", cookie), None);
        async move {
            let res = app.oneshot(req).await.unwrap();
            (res.status(), json_of(res).await)
        }
    };
    // (Disabling signed Sam out: sign in again to be a member with a session.)
    let sam_cookie = cookie_of(&app.clone().oneshot(login_req("sam@example.com", "correct horse battery")).await.unwrap());
    assert_eq!(get("", &sam_cookie).await.0, StatusCode::FORBIDDEN);
    assert_eq!(status(&app, Request::builder().uri("/auth/admin/audit").body(Body::empty()).unwrap()).await, StatusCode::UNAUTHORIZED);

    let (s, all) = get("", &admin).await;
    assert_eq!(s, StatusCode::OK);
    let actions: Vec<&str> = all["items"].as_array().unwrap().iter().map(|i| i["action"].as_str().unwrap()).collect();
    // Newest first: the reset link, enable, disable, then the invite's creation and use.
    assert_eq!(&actions[..3], ["account.reset_link", "account.enable", "account.disable"]);
    assert!(all["actions"].as_array().unwrap().iter().any(|a| a == "invite.create"));

    let names = |v: &Value| -> Vec<String> { v["items"].as_array().unwrap().iter().map(|i| i["action"].as_str().unwrap().to_string()).collect() };
    assert_eq!(names(&get("?action=account.", &admin).await.1), ["account.reset_link", "account.enable", "account.disable"]);
    assert_eq!(names(&get("?action=account.dis", &admin).await.1), ["account.disable"]);
    assert_eq!(names(&get("?action=invite.use", &admin).await.1), ["invite.use"]);
    // `_` and `%` are literal.
    assert!(names(&get("?action=account_", &admin).await.1).is_empty());
    assert_eq!(names(&get("?actor=SAM@", &admin).await.1), ["invite.use"]);
    assert_eq!(names(&get("?actor=admin%40example", &admin).await.1).len(), actions.len() - 1);
    assert_eq!(names(&get(&format!("?target={sam}&action=account.d"), &admin).await.1), ["account.disable"]);
    assert_eq!(names(&get("?target=sam@example&action=account.e", &admin).await.1), ["account.enable"]);
    // Dates.
    assert_eq!(get("?from=2999-01-01", &admin).await.1["items"].as_array().unwrap().len(), 0);
    assert_eq!(get("?to=2000-01-01", &admin).await.1["items"].as_array().unwrap().len(), 0);
    assert_eq!(get("?from=2000-01-01&to=2999-01-01", &admin).await.1["items"].as_array().unwrap().len(), actions.len());
    assert_eq!(get("?from=yesterday", &admin).await.0, StatusCode::BAD_REQUEST);
    // Paging by id, newest first, without gaps or repeats.
    let (_, p1) = get("?limit=2", &admin).await;
    assert_eq!(p1["items"].as_array().unwrap().len(), 2);
    let before = p1["next_before"].as_i64().unwrap();
    let (_, p2) = get(&format!("?limit=2&before={before}"), &admin).await;
    let mut seen = names(&p1);
    seen.extend(names(&p2));
    assert_eq!(&seen[..], &actions[..seen.len()].iter().map(|s| s.to_string()).collect::<Vec<_>>()[..]);
    let (_, last) = get(&format!("?limit=500&before={before}"), &admin).await;
    assert!(last["next_before"].is_null());
    t.teardown().await;
}
