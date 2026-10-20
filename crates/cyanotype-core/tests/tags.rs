//! Importing a file's keywords as tags (`cyanotype_core::tags`) against a real Postgres (skipped without `DATABASE_URL`).
use cyanotype_core::{db, tags};
use sqlx::{Connection, Executor, PgConnection, PgPool};
use uuid::Uuid;

struct Fx {
    pool: PgPool,
    admin_url: String,
    name: String,
    owner: Uuid,
}

impl Fx {
    async fn new() -> Option<Self> {
        let admin_url = std::env::var("DATABASE_URL").ok().filter(|v| !v.is_empty())?;
        let name = format!("cyanotype_test_{}", Uuid::new_v4().simple());
        let mut admin = PgConnection::connect(&admin_url).await.unwrap();
        admin.execute(format!("CREATE DATABASE {name}").as_str()).await.unwrap();
        let (base, _) = admin_url.rsplit_once('/').unwrap();
        let pool = db::connect(&format!("{base}/{name}")).await.unwrap();
        db::migrate(&pool).await.unwrap();
        let owner =
            sqlx::query_scalar("INSERT INTO users (email, password_hash) VALUES ('o@example.com','x') RETURNING id")
                .fetch_one(&pool)
                .await
                .unwrap();
        Some(Self {
            pool,
            admin_url,
            name,
            owner,
        })
    }

    async fn teardown(self) {
        self.pool.close().await;
        let mut admin = PgConnection::connect(&self.admin_url).await.unwrap();
        admin
            .execute(format!("DROP DATABASE IF EXISTS {} WITH (FORCE)", self.name).as_str())
            .await
            .unwrap();
    }

    async fn photo(&self, name: &str) -> Uuid {
        sqlx::query_scalar(
            "INSERT INTO assets (owner_id, sha256, original_key, original_name, content_type, size_bytes)
             VALUES ($1, sha256($2::bytea), $2, $2, 'image/jpeg', 1) RETURNING id",
        )
        .bind(self.owner)
        .bind(name)
        .fetch_one(&self.pool)
        .await
        .unwrap()
    }

    async fn tags(&self, asset: Uuid) -> Vec<(String, bool)> {
        sqlx::query_as(
            "SELECT t.name, at.imported FROM asset_tags at JOIN tags t ON t.id = at.tag_id WHERE at.asset_id = $1
             ORDER BY lower(t.name)",
        )
        .bind(asset)
        .fetch_all(&self.pool)
        .await
        .unwrap()
    }
}

fn kw(words: &[&str]) -> Vec<String> {
    words.iter().map(|w| w.to_string()).collect()
}

#[tokio::test]
async fn keywords_become_imported_tags_once_per_photo() {
    let Some(fx) = Fx::new().await else { return };
    let a = fx.photo("a.jpg").await;
    let n = tags::import_keywords(
        &fx.pool,
        a,
        &kw(&["Beach", "beach", " Sun, set ", "trip/2024", "", "x\u{0}y"]),
    )
    .await
    .unwrap();
    // Case-insensitive duplicates fold, commas and control characters become spaces, empty ones are dropped.
    assert_eq!(n, 4);
    assert_eq!(
        fx.tags(a).await,
        [
            ("Beach".to_string(), true),
            ("Sun set".to_string(), true),
            ("trip/2024".to_string(), true),
            ("x y".to_string(), true)
        ]
    );
    // The owner removes one and adds one of their own; running the import again changes nothing (the keywords were read).
    sqlx::query("DELETE FROM asset_tags WHERE asset_id = $1 AND tag_id = (SELECT id FROM tags WHERE name = 'Beach')")
        .bind(a)
        .execute(&fx.pool)
        .await
        .unwrap();
    assert_eq!(
        tags::import_keywords(&fx.pool, a, &kw(&["Beach", "new one"]))
            .await
            .unwrap(),
        0
    );
    assert_eq!(fx.tags(a).await.len(), 3, "a removed tag does not come back");
    let marked: Option<time::OffsetDateTime> = sqlx::query_scalar("SELECT tags_imported_at FROM assets WHERE id = $1")
        .bind(a)
        .fetch_one(&fx.pool)
        .await
        .unwrap();
    assert!(marked.is_some());
    // A file with no keywords is also "read".
    let b = fx.photo("b.jpg").await;
    assert_eq!(tags::import_keywords(&fx.pool, b, &[]).await.unwrap(), 0);
    assert_eq!(tags::import_keywords(&fx.pool, b, &kw(&["late"])).await.unwrap(), 0);
    // The same keyword on another photo is the same tag (one row per owner and name).
    let c = fx.photo("c.jpg").await;
    tags::import_keywords(&fx.pool, c, &kw(&["SUN SET"])).await.unwrap();
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM tags WHERE lower(name) = 'sun set'")
        .fetch_one(&fx.pool)
        .await
        .unwrap();
    assert_eq!(rows, 1);
    fx.teardown().await;
}

#[tokio::test]
async fn keywords_go_to_the_visible_photo_of_a_pair_are_capped_and_an_owner_tag_stays_theirs() {
    let Some(fx) = Fx::new().await else { return };
    let (jpg, raw) = (fx.photo("p.jpg").await, fx.photo("p.dng").await);
    sqlx::query("UPDATE assets SET primary_id = $1 WHERE id = $2")
        .bind(jpg)
        .bind(raw)
        .execute(&fx.pool)
        .await
        .unwrap();
    // The owner already has "Mine" on the JPEG; the RAW's file says it too, plus a keyword of its own.
    let mut tx = fx.pool.begin().await.unwrap();
    let mine = tags::ensure(&mut tx, fx.owner, "Mine").await.unwrap();
    tags::attach(&mut tx, jpg, mine, false).await.unwrap();
    tx.commit().await.unwrap();
    tags::import_keywords(&fx.pool, raw, &kw(&["mine", "From the RAW"]))
        .await
        .unwrap();
    assert!(fx.tags(raw).await.is_empty());
    assert_eq!(
        fx.tags(jpg).await,
        [("From the RAW".to_string(), true), ("Mine".to_string(), false)]
    );
    // A file with hundreds of keywords brings at most MAX_IMPORTED.
    let many = fx.photo("many.jpg").await;
    let words: Vec<String> = (0..500).map(|n| format!("k{n}")).collect();
    tags::import_keywords(&fx.pool, many, &words).await.unwrap();
    assert!(fx.tags(many).await.len() <= tags::MAX_IMPORTED);
    // A photo that is being deleted for good, or does not exist, imports nothing.
    assert_eq!(
        tags::import_keywords(&fx.pool, Uuid::new_v4(), &kw(&["x"]))
            .await
            .unwrap(),
        0
    );
    fx.teardown().await;
}

#[tokio::test]
async fn deleting_a_photo_or_an_account_removes_the_links_and_tags() {
    let Some(fx) = Fx::new().await else { return };
    let a = fx.photo("a.jpg").await;
    tags::import_keywords(&fx.pool, a, &kw(&["one", "two"])).await.unwrap();
    sqlx::query("DELETE FROM assets WHERE id = $1")
        .bind(a)
        .execute(&fx.pool)
        .await
        .unwrap();
    let links: i64 = sqlx::query_scalar("SELECT count(*) FROM asset_tags")
        .fetch_one(&fx.pool)
        .await
        .unwrap();
    assert_eq!(links, 0, "the cascade of a purged photo");
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM tags")
        .fetch_one(&fx.pool)
        .await
        .unwrap();
    assert_eq!(left, 2, "the tags stay: they are the owner's");
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(fx.owner)
        .execute(&fx.pool)
        .await
        .unwrap();
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM tags")
        .fetch_one(&fx.pool)
        .await
        .unwrap();
    assert_eq!(left, 0);
    fx.teardown().await;
}
