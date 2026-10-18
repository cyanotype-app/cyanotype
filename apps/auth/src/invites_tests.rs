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
