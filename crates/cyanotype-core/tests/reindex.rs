//! `reindex` and the recovery manifests against a real Postgres and a real S3 API.
//! Needs `DATABASE_URL` (role may CREATE DATABASE) and `S3_ENDPOINT`, `S3_ACCESS_KEY`, `S3_SECRET_KEY`
//! (+ `S3_FORCE_PATH_STYLE=true` for SeaweedFS). Each test creates its own bucket (versioning on, like
//! production) and database; without both variables the tests return early.
use std::time::Duration;

use aws_sdk_s3::{
    Client,
    config::{Credentials, Region},
    types::{BucketVersioningStatus, VersioningConfiguration},
};
use cyanotype_core::{
    config::S3Config,
    db, jobs, keys, manifest, objmeta,
    reindex::{self, Options, Report},
    storage::Storage,
};
use sha2::{Digest, Sha256};
use sqlx::{Connection, Executor, PgConnection, PgPool};
use time::{OffsetDateTime, macros::datetime};
use uuid::Uuid;

struct Fx {
    pool: PgPool,
    admin_url: String,
    db_name: String,
    storage: Storage,
    client: Client,
    bucket: String,
    user: Uuid,
}

impl Fx {
    async fn new() -> Option<Self> {
        let admin_url = std::env::var("DATABASE_URL").ok().filter(|v| !v.is_empty())?;
        std::env::var("S3_ENDPOINT").ok().filter(|v| !v.is_empty())?;
        let mut cfg = S3Config::from_env().expect("S3_* env");
        cfg.bucket = format!("cyx-{}", Uuid::new_v4().simple());
        let client = Client::from_conf(
            aws_sdk_s3::Config::builder()
                .behavior_version_latest()
                .region(Region::new(cfg.region.clone()))
                .credentials_provider(Credentials::new(&cfg.access_key, &cfg.secret_key, None, None, "static"))
                .force_path_style(cfg.force_path_style)
                .endpoint_url(cfg.endpoint.clone().unwrap())
                .build(),
        );
        client
            .create_bucket()
            .bucket(&cfg.bucket)
            .send()
            .await
            .expect("create bucket");
        client
            .put_bucket_versioning()
            .bucket(&cfg.bucket)
            .versioning_configuration(
                VersioningConfiguration::builder()
                    .status(BucketVersioningStatus::Enabled)
                    .build(),
            )
            .send()
            .await
            .expect("enable versioning");

        let db_name = format!("cyanotype_test_{}", Uuid::new_v4().simple());
        let mut admin = PgConnection::connect(&admin_url).await.unwrap();
        admin
            .execute(format!("CREATE DATABASE {db_name}").as_str())
            .await
            .unwrap();
        let (base, _) = admin_url.rsplit_once('/').unwrap();
        let pool = db::connect(&format!("{base}/{db_name}")).await.unwrap();
        db::migrate(&pool).await.unwrap();
        let user = make_user(&pool, "owner@example.com").await;
        Some(Self {
            pool,
            admin_url,
            db_name,
            storage: Storage::new(&cfg),
            client,
            bucket: cfg.bucket,
            user,
        })
    }

    async fn teardown(self) {
        self.pool.close().await;
        let mut admin = PgConnection::connect(&self.admin_url).await.unwrap();
        admin
            .execute(format!("DROP DATABASE IF EXISTS {} WITH (FORCE)", self.db_name).as_str())
            .await
            .unwrap();
    }

    /// Store an original the way the upload path does (multipart, metadata on create). `meta` None = an object
    /// from before self-describing originals existed. Returns (key, sha256).
    async fn put_original(
        &self,
        owner: Uuid,
        filename: &str,
        body: &[u8],
        meta: Option<objmeta::OriginalMeta>,
    ) -> (String, [u8; 32]) {
        let sha: [u8; 32] = Sha256::digest(body).into();
        let key = keys::original(owner, &sha, &keys::sanitize_ext(filename));
        let ct = "image/jpeg";
        let md = meta.map(|m| objmeta::encode(&m)).unwrap_or_default();
        let upload = self.storage.create_multipart(&key, ct, md).await.unwrap();
        let etag = self
            .storage
            .upload_part(&key, &upload, 1, body.to_vec().into())
            .await
            .unwrap();
        self.storage
            .complete_multipart(&key, &upload, &[(1, etag)])
            .await
            .unwrap();
        (key, sha)
    }

    fn meta(&self, name: &str) -> objmeta::OriginalMeta {
        objmeta::OriginalMeta {
            name: Some(name.into()),
            owner: Some(self.user),
            created_at: Some(datetime!(2020-03-04 05:06:07 UTC)),
            file_modified_at: Some(datetime!(2019-12-31 23:00:00 UTC)),
        }
    }

    async fn insert_row(&self, owner: Uuid, key: &str, sha: &[u8; 32], name: &str) -> Uuid {
        sqlx::query_scalar(
            "INSERT INTO assets (owner_id, sha256, original_key, original_name, content_type, size_bytes)
             VALUES ($1,$2,$3,$4,'image/jpeg',10) RETURNING id",
        )
        .bind(owner)
        .bind(&sha[..])
        .bind(key)
        .bind(name)
        .fetch_one(&self.pool)
        .await
        .unwrap()
    }

    async fn run(&self, f: impl FnOnce(&mut Options)) -> Report {
        let mut o = Options {
            min_age: Duration::ZERO,
            ..Options::default()
        };
        f(&mut o);
        reindex::run(&self.pool, &self.storage, &o).await.unwrap()
    }

    async fn count(&self, table: &str) -> i64 {
        sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }
}

async fn make_user(pool: &PgPool, email: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO users (email, password_hash) VALUES ($1, 'x') RETURNING id")
        .bind(email)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn orphans_are_reported_then_recreated_with_names_and_jobs_and_a_rerun_changes_nothing() {
    let Some(fx) = Fx::new().await else { return };
    let tricky = "Süd-Tirol \"Urlaub\" 🌅.JPG";
    let (k1, s1) = fx.put_original(fx.user, tricky, b"one", Some(fx.meta(tricky))).await;
    let (k2, s2) = fx.put_original(fx.user, "legacy.heic", b"two", None).await;
    let (_k3, _) = fx
        .put_original(fx.user, "plain.png", b"three", Some(fx.meta("plain.png")))
        .await;

    let dry = fx.run(|_| {}).await;
    assert_eq!(
        (dry.mode, dry.scanned_objects, dry.orphans, dry.orphans_recreated),
        ("dry-run", 3, 3, 3)
    );
    assert_eq!(fx.count("assets").await, 0, "a dry run writes nothing");
    assert_eq!(fx.count("jobs").await, 0);
    assert_eq!(dry.users[&fx.user].orphans, 3);
    assert!(dry.text().contains("DRY RUN"));

    let applied = fx.run(|o| o.apply = true).await;
    assert_eq!((applied.orphans_recreated, applied.errors), (3, 0));
    assert_eq!(fx.count("assets").await, 3);
    assert_eq!(fx.count("jobs").await, 6, "metadata + thumbnail for each");

    let (name, ct, size, created, mtime, owner): (String, String, i64, OffsetDateTime, Option<OffsetDateTime>, Uuid) =
        sqlx::query_as("SELECT original_name, content_type, size_bytes, created_at, file_modified_at, owner_id FROM assets WHERE original_key = $1")
            .bind(&k1)
            .fetch_one(&fx.pool)
            .await
            .unwrap();
    assert_eq!(name, tricky);
    assert_eq!((ct.as_str(), size, owner), ("image/jpeg", 3, fx.user));
    assert_eq!(created, datetime!(2020-03-04 05:06:07 UTC));
    assert_eq!(mtime, Some(datetime!(2019-12-31 23:00:00 UTC)));
    let (legacy_name, legacy_created): (String, OffsetDateTime) =
        sqlx::query_as("SELECT original_name, created_at FROM assets WHERE original_key = $1")
            .bind(&k2)
            .fetch_one(&fx.pool)
            .await
            .unwrap();
    assert_eq!(
        legacy_name,
        format!("{}.heic", keys::hex(&s2)),
        "no metadata: falls back to {{hash}}.{{ext}}"
    );
    assert!(legacy_created.year() >= 2026, "falls back to LastModified");
    let sha_db: Vec<u8> = sqlx::query_scalar("SELECT sha256 FROM assets WHERE original_key = $1")
        .bind(&k1)
        .fetch_one(&fx.pool)
        .await
        .unwrap();
    assert_eq!(sha_db, s1.to_vec());

    // Idempotent: a second run, and a run after an "interruption" (one row deleted), only fills the gap.
    let again = fx.run(|o| o.apply = true).await;
    assert_eq!((again.present, again.orphans, again.orphans_recreated), (3, 0, 0));
    assert_eq!((fx.count("assets").await, fx.count("jobs").await), (3, 6));
    sqlx::query("DELETE FROM assets WHERE original_key = $1")
        .bind(&k2)
        .execute(&fx.pool)
        .await
        .unwrap();
    let resumed = fx.run(|o| o.apply = true).await;
    assert_eq!((resumed.present, resumed.orphans_recreated), (2, 1));
    assert_eq!((fx.count("assets").await, fx.count("jobs").await), (3, 6));
    fx.teardown().await;
}

#[tokio::test]
async fn too_recent_objects_are_left_for_the_upload_that_is_registering_them() {
    let Some(fx) = Fx::new().await else { return };
    fx.put_original(fx.user, "fresh.jpg", b"fresh", None).await;
    let r = fx.run(|o| o.min_age = Duration::from_secs(3600)).await;
    assert_eq!((r.too_recent, r.orphans), (1, 0));
    fx.teardown().await;
}

#[tokio::test]
async fn orphans_of_unknown_users_are_skipped_unless_assigned() {
    let Some(fx) = Fx::new().await else { return };
    let ghost = Uuid::new_v4();
    let (key, _) = fx
        .put_original(ghost, "ghost.jpg", b"boo", Some(fx.meta("ghost.jpg")))
        .await;

    let r = fx.run(|o| o.apply = true).await;
    assert_eq!((r.orphans_unknown_user, r.orphans_recreated), (1, 0));
    assert!(r.users[&ghost].unknown_user && r.users[&ghost].orphans_skipped == 1);
    assert_eq!(fx.count("assets").await, 0);

    let to = fx.user;
    let r = fx
        .run(|o| {
            o.apply = true;
            o.assign_to = Some(to);
        })
        .await;
    assert_eq!((r.orphans_reassigned, r.orphans_recreated), (1, 1));
    let (owner, k): (Uuid, String) = sqlx::query_as("SELECT owner_id, original_key FROM assets")
        .fetch_one(&fx.pool)
        .await
        .unwrap();
    assert_eq!((owner, k), (fx.user, key));

    let bad = reindex::run(
        &fx.pool,
        &fx.storage,
        &Options {
            assign_to: Some(Uuid::new_v4()),
            ..Options::default()
        },
    )
    .await;
    assert!(bad.is_err(), "--assign-to must name an existing user");
    fx.teardown().await;
}

#[tokio::test]
async fn rows_with_missing_originals_are_reported_and_only_trashed_when_asked() {
    let Some(fx) = Fx::new().await else { return };
    let gone = fx
        .insert_row(fx.user, &keys::original(fx.user, &[7; 32], "jpg"), &[7; 32], "gone.jpg")
        .await;
    let (k, s) = fx.put_original(fx.user, "kept.jpg", b"kept", None).await;
    fx.insert_row(fx.user, &k, &s, "kept.jpg").await;
    // An object whose newest version is a delete marker counts as gone.
    let (k2, s2) = fx.put_original(fx.user, "deleted.jpg", b"deleted", None).await;
    fx.insert_row(fx.user, &k2, &s2, "deleted.jpg").await;
    fx.client
        .delete_object()
        .bucket(&fx.bucket)
        .key(&k2)
        .send()
        .await
        .unwrap();

    let deleted = |id: Uuid| {
        let pool = fx.pool.clone();
        async move {
            sqlx::query_scalar::<_, Option<OffsetDateTime>>("SELECT deleted_at FROM assets WHERE id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    let r = fx.run(|o| o.apply = true).await;
    assert_eq!((r.missing_originals, r.missing_marked_trashed), (2, 0));
    assert_eq!(r.users[&fx.user].missing_originals, 2);
    assert!(deleted(gone).await.is_none(), "reported only by default");

    let r = fx.run(|o| o.mark_missing = true).await;
    assert_eq!(r.missing_marked_trashed, 1 + 1, "dry run counts what it would do");
    assert!(deleted(gone).await.is_none());

    let r = fx
        .run(|o| {
            o.apply = true;
            o.mark_missing = true;
        })
        .await;
    assert_eq!(r.missing_marked_trashed, 2);
    let first = deleted(gone).await.expect("trashed");
    assert_eq!(fx.count("assets").await, 3, "rows are never deleted");
    // Re-run: nothing live is left to trash, the timestamp is untouched.
    let r = fx
        .run(|o| {
            o.apply = true;
            o.mark_missing = true;
        })
        .await;
    assert_eq!(r.missing_marked_trashed, 0);
    assert_eq!(deleted(gone).await, Some(first));
    fx.teardown().await;
}

#[tokio::test]
async fn trashed_rows_are_untouched_and_purging_rows_are_never_resurrected() {
    let Some(fx) = Fx::new().await else { return };
    let (kt, st) = fx.put_original(fx.user, "trashed.jpg", b"t", None).await;
    let (kp, sp) = fx.put_original(fx.user, "purging.jpg", b"p", None).await;
    let t = fx.insert_row(fx.user, &kt, &st, "trashed.jpg").await;
    let p = fx.insert_row(fx.user, &kp, &sp, "purging.jpg").await;
    sqlx::query("UPDATE assets SET deleted_at = now() - interval '3 days' WHERE id = $1")
        .bind(t)
        .execute(&fx.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE assets SET deleted_at = now(), purging_at = now() WHERE id = $1")
        .bind(p)
        .execute(&fx.pool)
        .await
        .unwrap();
    let before: Vec<(Uuid, Option<OffsetDateTime>, Option<OffsetDateTime>)> =
        sqlx::query_as("SELECT id, deleted_at, purging_at FROM assets ORDER BY id")
            .fetch_all(&fx.pool)
            .await
            .unwrap();

    let r = fx
        .run(|o| {
            o.apply = true;
            o.mark_missing = true;
            o.rethumb = true;
            o.backfill_metadata = true;
        })
        .await;
    assert_eq!((r.present, r.trashed_rows, r.purging_rows, r.orphans), (2, 1, 1, 0));
    let after: Vec<(Uuid, Option<OffsetDateTime>, Option<OffsetDateTime>)> =
        sqlx::query_as("SELECT id, deleted_at, purging_at FROM assets ORDER BY id")
            .fetch_all(&fx.pool)
            .await
            .unwrap();
    assert_eq!(before, after);
    assert_eq!(fx.count("assets").await, 2);
    // The purging row's object got no backfill (only the trashed one is described).
    assert!(!objmeta::is_described(
        &fx.storage.head(&kp).await.unwrap().unwrap().metadata
    ));
    assert!(objmeta::is_described(
        &fx.storage.head(&kt).await.unwrap().unwrap().metadata
    ));
    // Thumbnails are wanted for the trash view too, so the trashed (not purging) row gets a job.
    let jobs: Vec<(Uuid,)> = sqlx::query_as("SELECT asset_id FROM jobs")
        .fetch_all(&fx.pool)
        .await
        .unwrap();
    assert_eq!(jobs, vec![(t,)]);
    fx.teardown().await;
}

#[tokio::test]
async fn rethumb_requeues_only_rows_whose_derived_images_are_gone() {
    let Some(fx) = Fx::new().await else { return };
    let mut ids = Vec::new();
    for (i, body) in [b"a".as_slice(), b"b", b"c"].into_iter().enumerate() {
        let name = format!("{i}.jpg");
        let (k, s) = fx.put_original(fx.user, &name, body, None).await;
        ids.push(fx.insert_row(fx.user, &k, &s, &name).await);
    }
    for id in &ids {
        jobs::enqueue(&fx.pool, jobs::KIND_THUMBNAIL, *id, 1).await.unwrap();
        sqlx::query("UPDATE jobs SET state = 'done', attempts = 1 WHERE asset_id = $1")
            .bind(id)
            .execute(&fx.pool)
            .await
            .unwrap();
    }
    // 0: has thumbnail + preview. 1: has nothing. 2: has nothing, but its job is still waiting.
    fx.storage
        .put_bytes(&keys::thumb(fx.user, ids[0]), vec![1], "image/webp")
        .await
        .unwrap();
    fx.storage
        .put_bytes(&keys::preview(fx.user, ids[0]), vec![1], "image/webp")
        .await
        .unwrap();
    sqlx::query("UPDATE jobs SET state = 'queued' WHERE asset_id = $1")
        .bind(ids[2])
        .execute(&fx.pool)
        .await
        .unwrap();

    let r = fx.run(|o| o.rethumb = true).await;
    assert_eq!((r.thumbs_missing, r.thumbs_requeued), (1, 1), "dry run reports");
    let state = |id: Uuid| {
        let pool = fx.pool.clone();
        async move {
            sqlx::query_scalar::<_, String>("SELECT state FROM jobs WHERE asset_id = $1 AND kind = 'thumbnail'")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    assert_eq!(state(ids[1]).await, "done");

    let r = fx
        .run(|o| {
            o.apply = true;
            o.rethumb = true;
        })
        .await;
    assert_eq!((r.thumbs_missing, r.thumbs_requeued), (1, 1));
    assert_eq!(state(ids[1]).await, "queued");
    assert_eq!(
        (state(ids[0]).await.as_str(), state(ids[2]).await.as_str()),
        ("done", "queued")
    );
    // Without --rethumb it is reported but not touched.
    sqlx::query("UPDATE jobs SET state = 'done' WHERE asset_id = $1")
        .bind(ids[1])
        .execute(&fx.pool)
        .await
        .unwrap();
    let r = fx.run(|o| o.apply = true).await;
    assert_eq!((r.thumbs_missing, r.thumbs_requeued), (1, 0));
    assert_eq!(state(ids[1]).await, "done");
    fx.teardown().await;
}

#[tokio::test]
async fn foreign_keys_and_extra_copies_are_reported_and_never_touched() {
    let Some(fx) = Fx::new().await else { return };
    fx.storage
        .put_bytes("originals/README.txt", b"hi".to_vec(), "text/plain")
        .await
        .unwrap();
    fx.storage
        .put_bytes(
            &format!("originals/{}/zz/{}.jpg", fx.user, "ab".repeat(32)),
            b"x".to_vec(),
            "x/y",
        )
        .await
        .unwrap();
    let (k, s) = fx.put_original(fx.user, "a.jpg", b"same", None).await;
    fx.insert_row(fx.user, &k, &s, "a.jpg").await;
    // Same content under another extension: an alias of an indexed photo, not a new one.
    let alias = keys::original(fx.user, &s, "jpeg");
    fx.storage
        .put_bytes(&alias, b"same".to_vec(), "image/jpeg")
        .await
        .unwrap();

    let r = fx.run(|o| o.apply = true).await;
    assert_eq!(
        (r.unparseable_keys, r.alias_objects, r.orphans, r.present),
        (2, 1, 0, 1)
    );
    assert_eq!(fx.count("assets").await, 1);
    // Reindex never deletes or overwrites.
    assert!(fx.storage.head(&alias).await.unwrap().is_some());
    assert!(fx.storage.head("originals/README.txt").await.unwrap().is_some());
    fx.teardown().await;
}

#[tokio::test]
async fn more_than_one_listing_page_is_scanned() {
    let Some(fx) = Fx::new().await else { return };
    let mut set = tokio::task::JoinSet::new();
    for i in 0..1_050u32 {
        let (st, user) = (fx.storage.clone(), fx.user);
        set.spawn(async move {
            let sha: [u8; 32] = Sha256::digest(i.to_be_bytes()).into();
            st.put_bytes(&keys::original(user, &sha, "jpg"), vec![0], "image/jpeg")
                .await
                .unwrap();
        });
        if set.len() >= 32 {
            set.join_next().await;
        }
    }
    while set.join_next().await.is_some() {}
    let r = fx.run(|_| {}).await;
    assert_eq!((r.scanned_objects, r.orphans), (1_050, 1_050));
    fx.teardown().await;
}

#[tokio::test]
async fn backfill_writes_metadata_onto_existing_originals_only_when_asked_and_applied() {
    let Some(fx) = Fx::new().await else { return };
    let (k, s) = fx.put_original(fx.user, "old photo.jpg", b"legacy bytes", None).await;
    let id = fx.insert_row(fx.user, &k, &s, "Älteres Foto \"1\".jpg").await;
    let head = |k: String| {
        let st = fx.storage.clone();
        async move { st.head(&k).await.unwrap().unwrap() }
    };

    let r = fx.run(|o| o.apply = true).await;
    assert_eq!(r.backfill_needed, 0, "opt-in");
    let r = fx.run(|o| o.backfill_metadata = true).await;
    assert_eq!((r.backfill_needed, r.backfilled), (1, 0));
    assert!(!objmeta::is_described(&head(k.clone()).await.metadata));

    let r = fx
        .run(|o| {
            o.apply = true;
            o.backfill_metadata = true;
        })
        .await;
    assert_eq!((r.backfill_needed, r.backfilled, r.errors), (1, 1, 0));
    let h = head(k.clone()).await;
    let m = objmeta::decode(&h.metadata);
    assert_eq!(m.name.as_deref(), Some("Älteres Foto \"1\".jpg"));
    assert_eq!(
        (m.owner, h.content_type.as_deref(), h.size),
        (Some(fx.user), Some("image/jpeg"), 12)
    );
    assert_eq!(fx.storage.get_bytes(&k).await.unwrap(), b"legacy bytes");
    let r = fx
        .run(|o| {
            o.apply = true;
            o.backfill_metadata = true;
        })
        .await;
    assert_eq!(
        (r.backfill_needed, r.backfilled),
        (0, 0),
        "already described: idempotent"
    );
    let _ = id;
    fx.teardown().await;
}

#[tokio::test]
async fn manifest_round_trip_restores_names_trash_albums_and_pairs() {
    let Some(fx) = Fx::new().await else { return };
    let (kj, sj) = fx.put_original(fx.user, "IMG_1.jpg", b"jpeg", None).await;
    let (kr, sr) = fx.put_original(fx.user, "IMG_1.dng", b"raw!", None).await;
    let (kt, st) = fx.put_original(fx.user, "bin.jpg", b"trash", None).await;
    let (ks, ss) = fx.put_original(fx.user, "solo.jpg", b"solo", None).await;
    let jpeg = fx.insert_row(fx.user, &kj, &sj, "IMG_1.jpg").await;
    let raw = fx.insert_row(fx.user, &kr, &sr, "IMG_1.dng").await;
    let trashed = fx.insert_row(fx.user, &kt, &st, "Papierkorb ü.jpg").await;
    let solo = fx.insert_row(fx.user, &ks, &ss, "Solo \"x\".jpg").await;
    sqlx::query("UPDATE assets SET primary_id = $2 WHERE id = $1")
        .bind(raw)
        .bind(jpeg)
        .execute(&fx.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE assets SET deleted_at = now() WHERE id = $1")
        .bind(trashed)
        .execute(&fx.pool)
        .await
        .unwrap();
    let album: Uuid = sqlx::query_scalar(
        "INSERT INTO albums (owner_id, title, description, cover_asset_id) VALUES ($1,'Trip','desc',$2) RETURNING id",
    )
    .bind(fx.user)
    .bind(solo)
    .fetch_one(&fx.pool)
    .await
    .unwrap();
    for a in [jpeg, solo] {
        sqlx::query("INSERT INTO album_assets (album_id, asset_id) VALUES ($1,$2)")
            .bind(album)
            .bind(a)
            .execute(&fx.pool)
            .await
            .unwrap();
    }

    let n = manifest::write_user(&fx.pool, &fx.storage, fx.user, 14).await.unwrap();
    assert_eq!(n, 4);
    let listed = manifest::list_all(&fx.storage).await.unwrap();
    assert_eq!(listed[&fx.user].len(), 1);
    assert_eq!(
        manifest::write_missing(&fx.pool, &fx.storage, 14).await.unwrap(),
        0,
        "already written today"
    );

    // The disaster: the database loses everything about photos.
    sqlx::query("DELETE FROM assets").execute(&fx.pool).await.unwrap();
    sqlx::query("DELETE FROM albums").execute(&fx.pool).await.unwrap();

    let dry = fx.run(|_| {}).await;
    assert_eq!(
        (dry.orphans_recreated, dry.albums_restored, dry.manifest_users),
        (4, 1, 1)
    );
    assert_eq!((fx.count("assets").await, fx.count("albums").await), (0, 0));

    let r = fx.run(|o| o.apply = true).await;
    assert_eq!((r.orphans_recreated, r.albums_restored, r.errors), (4, 1, 0));
    assert_eq!(r.album_items_restored, 2);
    assert_eq!(r.pairs_restored, 1);
    // Same ids, so thumbnails already in the bucket still belong to their rows; names from the manifest.
    let names: Vec<(Uuid, String, Option<OffsetDateTime>, Option<Uuid>)> =
        sqlx::query_as("SELECT id, original_name, deleted_at, primary_id FROM assets ORDER BY original_name")
            .fetch_all(&fx.pool)
            .await
            .unwrap();
    let by_id = |id: Uuid| names.iter().find(|n| n.0 == id).unwrap().clone();
    assert_eq!(by_id(solo).1, "Solo \"x\".jpg");
    assert_eq!(by_id(jpeg).1, "IMG_1.jpg");
    assert!(
        by_id(trashed).2.is_some() && by_id(solo).2.is_none(),
        "trash state restored"
    );
    assert_eq!(by_id(raw).3, Some(jpeg), "RAW stacked behind its JPEG again");
    let (title, cover): (String, Option<Uuid>) =
        sqlx::query_as("SELECT title, cover_asset_id FROM albums WHERE id = $1")
            .bind(album)
            .fetch_one(&fx.pool)
            .await
            .unwrap();
    assert_eq!((title.as_str(), cover), ("Trip", Some(solo)));
    let members: i64 = sqlx::query_scalar("SELECT count(*) FROM album_assets WHERE album_id = $1")
        .bind(album)
        .fetch_one(&fx.pool)
        .await
        .unwrap();
    assert_eq!(members, 2);

    // Idempotent, and an album that exists is not rewritten.
    sqlx::query("DELETE FROM album_assets").execute(&fx.pool).await.unwrap();
    let again = fx.run(|o| o.apply = true).await;
    assert_eq!(
        (again.orphans, again.albums_restored, again.album_items_restored),
        (0, 0, 0)
    );
    assert_eq!(
        fx.count("album_assets").await,
        0,
        "existing albums are only extended for photos recreated in this run"
    );
    assert_eq!(fx.count("assets").await, 4);

    // --no-manifest falls back to bare object data.
    sqlx::query("DELETE FROM assets").execute(&fx.pool).await.unwrap();
    sqlx::query("DELETE FROM albums").execute(&fx.pool).await.unwrap();
    let plain = fx
        .run(|o| {
            o.apply = true;
            o.use_manifests = false;
        })
        .await;
    assert_eq!(
        (plain.orphans_recreated, plain.manifest_users, plain.albums_restored),
        (4, 0, 0)
    );
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM assets WHERE original_name LIKE '%.%' AND original_name ~ '^[0-9a-f]{64}\\.'",
    )
    .fetch_one(&fx.pool)
    .await
    .unwrap();
    assert_eq!(n, 4);
    fx.teardown().await;
}

#[tokio::test]
async fn manifests_merge_newest_first_and_are_pruned_to_the_keep_count() {
    let Some(fx) = Fx::new().await else { return };
    let (k, s) = fx.put_original(fx.user, "a.jpg", b"a", None).await;
    let id = fx.insert_row(fx.user, &k, &s, "new name.jpg").await;
    manifest::write_user(&fx.pool, &fx.storage, fx.user, 14).await.unwrap();
    // Older manifests, written by hand: one that disagrees about the name, one with a photo the new one lacks.
    let old = |name: &str, sha: &str, id: Uuid| {
        format!(
            "{{\"t\":\"meta\",\"v\":1,\"user\":\"{u}\",\"generated_at\":\"2026-01-01T00:00:00Z\",\"assets\":1}}\n\
             {{\"t\":\"asset\",\"id\":\"{id}\",\"sha256\":\"{sha}\",\"key\":\"k\",\"name\":\"{name}\",\"content_type\":\"image/jpeg\",\"size_bytes\":1}}\n",
            u = fx.user
        )
    };
    let other = "cd".repeat(32);
    fx.storage
        .put_bytes(
            &keys::manifest(fx.user, "20260101"),
            old("ancient.jpg", &keys::hex(&s), id).into_bytes(),
            "application/x-ndjson",
        )
        .await
        .unwrap();
    fx.storage
        .put_bytes(
            &keys::manifest(fx.user, "20260102"),
            old("only-old.jpg", &other, Uuid::new_v4()).into_bytes(),
            "application/x-ndjson",
        )
        .await
        .unwrap();
    // A manifest naming a different user is ignored.
    fx.storage
        .put_bytes(
            &keys::manifest(fx.user, "20260103"),
            old("x", &other, Uuid::new_v4())
                .replace(&fx.user.to_string(), &Uuid::new_v4().to_string())
                .into_bytes(),
            "application/x-ndjson",
        )
        .await
        .unwrap();

    let ks = &manifest::list_all(&fx.storage).await.unwrap()[&fx.user];
    assert_eq!(ks.len(), 4);
    assert!(ks[0] > ks[1], "newest first");
    let m = manifest::load(&fx.storage, fx.user, ks).await.unwrap();
    assert_eq!(m.assets[&s].name, "new name.jpg", "today's manifest wins");
    assert_eq!(
        m.assets[&keys::parse_sha256(&other).unwrap()].name,
        "only-old.jpg",
        "older manifests still contribute"
    );
    assert_eq!(m.files, 4);

    manifest::write_user(&fx.pool, &fx.storage, fx.user, 2).await.unwrap();
    let ks = &manifest::list_all(&fx.storage).await.unwrap()[&fx.user];
    assert_eq!(ks.len(), 2, "pruned to the newest two");
    fx.teardown().await;
}

#[tokio::test]
async fn upload_metadata_survives_a_multipart_round_trip_for_odd_names() {
    let Some(fx) = Fx::new().await else { return };
    for name in [
        "日本語の写真.heic",
        "with \"quotes\" & 100%.jpg",
        &format!("{}.jpg", "x".repeat(700)),
    ] {
        let (k, _) = fx
            .put_original(fx.user, name, name.as_bytes(), Some(fx.meta(name)))
            .await;
        let h = fx.storage.head(&k).await.unwrap().unwrap();
        let m = objmeta::decode(&h.metadata);
        assert_eq!(m.owner, Some(fx.user));
        assert_eq!(m.created_at, Some(datetime!(2020-03-04 05:06:07 UTC)));
        assert_eq!(
            m.name.as_deref(),
            Some(objmeta::clip_name(name, objmeta::MAX_NAME_ENCODED).as_str())
        );
        assert_eq!(h.content_type.as_deref(), Some("image/jpeg"));
    }
    fx.teardown().await;
}

#[tokio::test]
async fn edits_come_back_with_a_rebuilt_database() {
    use cyanotype_core::edits;
    let Some(fx) = Fx::new().await else { return };
    let (_, sha) = fx
        .put_original(fx.user, "edited.jpg", b"edited", Some(fx.meta("edited.jpg")))
        .await;
    let (_, _) = fx
        .put_original(fx.user, "plain.jpg", b"plain", Some(fx.meta("plain.jpg")))
        .await;
    let mut e = edits::Edits::none(edits::now());
    e.taken_at = Some(datetime!(2001-02-03 04:05:06 UTC));
    e.taken_offset = Some(3600);
    e.latitude = Some(48.85);
    e.longitude = Some(2.35);
    e.caption = Some("kept in the bucket".into());
    edits::put(&fx.storage, fx.user, &sha, &e).await.unwrap();
    // Objects reindex must leave alone: a user that does not exist, a key it does not know, and garbage.
    let ghost = Uuid::new_v4();
    edits::put(&fx.storage, ghost, &[9; 32], &e).await.unwrap();
    fx.storage
        .put_bytes("edits/junk.json", b"{}".to_vec(), "application/json")
        .await
        .unwrap();
    fx.storage
        .put_bytes(
            &keys::edits(fx.user, &[5; 32]),
            b"not json".to_vec(),
            "application/json",
        )
        .await
        .unwrap();

    // Dry run: the photo has no row yet, so its edits are waiting for it; nothing is written.
    let dry = fx.run(|_| {}).await;
    assert_eq!(
        (dry.orphans, dry.edits_objects, dry.edits_restored),
        (2, 4, 0),
        "unexpected reindex counters"
    );
    assert_eq!(fx.count("assets").await, 0);

    let r = fx.run(|o| o.apply = true).await;
    assert_eq!(
        (r.orphans_recreated, r.edits_objects, r.edits_restored, r.errors),
        (2, 4, 1, 0),
        "unexpected reindex counters"
    );
    assert_eq!(
        (r.edits_without_photo, r.edits_bad),
        (2, 1),
        "no photo: the ghost's and the garbage one; not valid: the unknown key"
    );
    let (taken, off, lat, edited, caption, exif_lat): (
        OffsetDateTime,
        Option<i32>,
        f64,
        bool,
        Option<String>,
        Option<f64>,
    ) = sqlx::query_as(
        "SELECT taken_at, taken_offset_edit, latitude, date_edited AND location_edited, caption, latitude_exif
         FROM assets WHERE sha256 = $1",
    )
    .bind(&sha[..])
    .fetch_one(&fx.pool)
    .await
    .unwrap();
    assert_eq!(
        (taken, off, lat, edited),
        (datetime!(2001-02-03 04:05:06 UTC), Some(3600), 48.85, true)
    );
    assert_eq!((caption.as_deref(), exif_lat), (Some("kept in the bucket"), None));
    // The place is looked up again for the restored coordinates.
    let geocode: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind = 'geocode'")
        .fetch_one(&fx.pool)
        .await
        .unwrap();
    assert_eq!(geocode, 1);

    // The metadata job that now runs for the recreated row keeps them (the worker's test), and a second reindex has nothing to do.
    let again = fx.run(|o| o.apply = true).await;
    assert_eq!((again.edits_restored, again.edits_current), (0, 1));
    fx.teardown().await;
}

// ---- accounts, limits, server defaults and share links (docs/recovery.md, path B) -------------------------------------

/// Insert a share link row; returns its id.
async fn make_share(
    fx: &Fx,
    owner: Uuid,
    token: &str,
    album: Option<Uuid>,
    title: &str,
    password_hash: Option<&str>,
) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO shares (owner_id, token, kind, album_id, title, password_hash, allow_download, include_originals)
         VALUES ($1, $2, $3, $4, $5, $6, false, true) RETURNING id",
    )
    .bind(owner)
    .bind(token)
    .bind(if album.is_some() { "album" } else { "selection" })
    .bind(album)
    .bind(title)
    .bind(password_hash)
    .fetch_one(&fx.pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn a_bucket_only_rebuild_brings_back_accounts_roles_limits_defaults_and_share_links() {
    let Some(fx) = Fx::new().await else { return };
    // The state the manifests must carry: an admin with limits, a disabled member with none, the server defaults.
    sqlx::query("UPDATE users SET role = 'admin', quota_bytes = 1000, daily_upload_bytes = 500, max_file_bytes = 100 WHERE id = $1")
        .bind(fx.user)
        .execute(&fx.pool)
        .await
        .unwrap();
    let member = make_user(&fx.pool, "member@example.com").await;
    sqlx::query("UPDATE users SET disabled_at = now() WHERE id = $1")
        .bind(member)
        .execute(&fx.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO server_state (key, value) VALUES ('default_limits', '{\"quota_bytes\":7,\"daily_upload_bytes\":null,\"max_file_bytes\":null}'), ('require_2fa', '{\"required\":true}'), ('worker:abc', '{}')")
        .execute(&fx.pool)
        .await
        .unwrap();
    // Two-factor state must NOT travel (docs/recovery.md): it would be worthless without the password hash it belongs with,
    // and a secret or recovery-code hash in the bucket is a credential. Only the admin's "require 2FA" setting does.
    sqlx::query("INSERT INTO user_totp (user_id, secret_enc, confirmed_at) VALUES ($1, '\\x01', now())")
        .bind(fx.user)
        .execute(&fx.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO totp_recovery_codes (user_id, code_hash) VALUES ($1, '\\x02')")
        .bind(fx.user)
        .execute(&fx.pool)
        .await
        .unwrap();
    // Credentials that must NOT travel: an API key and a session.
    sqlx::query("INSERT INTO api_keys (user_id, name, key_hash, prefix) VALUES ($1, 'cli', '\\x01', 'cy_1')")
        .bind(fx.user)
        .execute(&fx.pool)
        .await
        .unwrap();

    let (k1, s1) = fx.put_original(fx.user, "a.jpg", b"aaa", Some(fx.meta("a.jpg"))).await;
    let (k2, s2) = fx.put_original(fx.user, "b.jpg", b"bbb", Some(fx.meta("b.jpg"))).await;
    let a = fx.insert_row(fx.user, &k1, &s1, "a.jpg").await;
    let b = fx.insert_row(fx.user, &k2, &s2, "b.jpg").await;
    let album: Uuid = sqlx::query_scalar("INSERT INTO albums (owner_id, title) VALUES ($1, 'Trip') RETURNING id")
        .bind(fx.user)
        .fetch_one(&fx.pool)
        .await
        .unwrap();
    for x in [a, b] {
        sqlx::query("INSERT INTO album_assets (album_id, asset_id) VALUES ($1, $2)")
            .bind(album)
            .bind(x)
            .execute(&fx.pool)
            .await
            .unwrap();
    }
    let open = make_share(
        &fx,
        fx.user,
        "open-token-aaaaaaaaaaaaaaaaaaaa",
        Some(album),
        "Album link",
        None,
    )
    .await;
    let locked = make_share(
        &fx,
        fx.user,
        "locked-token-bbbbbbbbbbbbbbbbbbb",
        None,
        "Selection link",
        Some("$argon2id$secret-not-for-the-bucket"),
    )
    .await;
    sqlx::query("INSERT INTO share_assets (share_id, asset_id) VALUES ($1, $2)")
        .bind(locked)
        .bind(b)
        .execute(&fx.pool)
        .await
        .unwrap();

    cyanotype_core::identity::write(&fx.pool, &fx.storage, 14)
        .await
        .unwrap();
    manifest::write_user(&fx.pool, &fx.storage, fx.user, 14).await.unwrap();
    // Nothing secret reached the bucket: no password hash, no API key hash.
    let acc_key = cyanotype_core::identity::list(&fx.storage).await.unwrap().remove(0);
    let acc = String::from_utf8(fx.storage.get_bytes(&acc_key).await.unwrap()).unwrap();
    let user_key = &manifest::list_all(&fx.storage).await.unwrap()[&fx.user][0];
    let um = String::from_utf8(fx.storage.get_bytes(user_key).await.unwrap()).unwrap();
    for text in [&acc, &um] {
        assert!(!text.contains("argon2") && !text.contains("secret-not-for-the-bucket") && !text.contains("key_hash"));
    }
    assert!(!acc.contains("worker:abc"), "heartbeats are not settings");

    // The disaster: the whole database is gone (users cascade to everything they own).
    sqlx::query("DELETE FROM users").execute(&fx.pool).await.unwrap();
    sqlx::query("DELETE FROM server_state").execute(&fx.pool).await.unwrap();
    assert_eq!(fx.count("users").await, 0);

    let dry = fx.run(|_| {}).await;
    assert_eq!(
        (
            dry.users_in_manifest,
            dry.users_restored,
            dry.orphans_recreated,
            dry.orphans_unknown_user
        ),
        (2, 2, 2, 0),
        "a dry run counts the accounts it would recreate and does not treat their photos as ownerless"
    );
    assert_eq!((dry.shares_restored, dry.settings_restored), (2, 2));
    assert_eq!(
        (fx.count("users").await, fx.count("shares").await),
        (0, 0),
        "a dry run writes nothing"
    );

    let r = fx.run(|o| o.apply = true).await;
    assert_eq!((r.users_restored, r.orphans_recreated, r.errors), (2, 2, 0));
    assert_eq!((r.albums_restored, r.shares_restored, r.settings_restored), (1, 2, 2));
    // The report (printed, and sent as JSON by the CLI) carries counts and ids only: no hash, token, email or password.
    let shown = format!("{}\n{}", r.text(), serde_json::to_string(&r).unwrap());
    for (label, planted) in [
        ("a password hash", "argon2"),
        ("a share token", "open-token-"),
        ("a share token", "locked-token-"),
        ("an email", "@example.com"),
    ] {
        assert!(!shown.contains(planted), "the report leaks {label}");
    }

    // Identity, role, limits, disabled flag: same ids as before.
    let (role, quota, daily, max_file, disabled): (String, Option<i64>, Option<i64>, Option<i64>, bool) =
        sqlx::query_as(
            "SELECT role, quota_bytes, daily_upload_bytes, max_file_bytes, disabled_at IS NOT NULL FROM users WHERE id = $1",
        )
        .bind(fx.user)
        .fetch_one(&fx.pool)
        .await
        .unwrap();
    assert_eq!(
        (role.as_str(), quota, daily, max_file, disabled),
        ("admin", Some(1000), Some(500), Some(100), false)
    );
    let (mrole, mdisabled, mquota): (String, bool, Option<i64>) =
        sqlx::query_as("SELECT role, disabled_at IS NOT NULL, quota_bytes FROM users WHERE id = $1")
            .bind(member)
            .fetch_one(&fx.pool)
            .await
            .unwrap();
    assert_eq!((mrole.as_str(), mdisabled, mquota), ("member", true, None));
    let defaults: serde_json::Value = sqlx::query_scalar("SELECT value FROM server_state WHERE key = 'default_limits'")
        .fetch_one(&fx.pool)
        .await
        .unwrap();
    assert_eq!(defaults["quota_bytes"], 7);
    let require: serde_json::Value = sqlx::query_scalar("SELECT value FROM server_state WHERE key = 'require_2fa'")
        .fetch_one(&fx.pool)
        .await
        .unwrap();
    assert_eq!(require["required"], true);
    assert_eq!(fx.count("server_state").await, 2);
    assert_eq!(
        (
            fx.count("user_totp").await,
            fx.count("totp_recovery_codes").await,
            fx.count("login_challenges").await
        ),
        (0, 0, 0),
        "2FA secrets and recovery codes are not in the bucket"
    );

    // Credentials: a locked password nobody can use, and no API key or session came back.
    let hash: String = sqlx::query_scalar("SELECT password_hash FROM users WHERE id = $1")
        .bind(fx.user)
        .fetch_one(&fx.pool)
        .await
        .unwrap();
    assert!(hash.starts_with("$argon2id$"));
    assert_eq!((fx.count("api_keys").await, fx.count("sessions").await), (0, 0));

    // Share links: same ids and tokens, settings kept, the protected one is closed (locked password), never open.
    let (token, kind, album_id, allow, orig): (String, String, Option<Uuid>, bool, bool) =
        sqlx::query_as("SELECT token, kind, album_id, allow_download, include_originals FROM shares WHERE id = $1")
            .bind(open)
            .fetch_one(&fx.pool)
            .await
            .unwrap();
    assert_eq!(
        (token.as_str(), kind.as_str(), album_id, allow, orig),
        ("open-token-aaaaaaaaaaaaaaaaaaaa", "album", Some(album), false, true)
    );
    let (ph, n_items): (Option<String>, i64) = sqlx::query_as(
        "SELECT s.password_hash, (SELECT count(*) FROM share_assets WHERE share_id = s.id) FROM shares s WHERE s.id = $1",
    )
    .bind(locked)
    .fetch_one(&fx.pool)
    .await
    .unwrap();
    let ph = ph.expect("a protected link comes back with a (locked) password, not open");
    assert!(ph.starts_with("$argon2id$") && !ph.contains("secret-not-for-the-bucket"));
    assert_eq!(n_items, 1);

    // A second run has nothing to do.
    let again = fx.run(|o| o.apply = true).await;
    assert_eq!(
        (
            again.users_restored,
            again.users_present,
            again.orphans,
            again.shares_restored
        ),
        (0, 2, 0, 0)
    );
    fx.teardown().await;
}

#[tokio::test]
async fn a_revoked_share_link_does_not_come_back_from_an_older_manifest() {
    let Some(fx) = Fx::new().await else { return };
    let (k, s) = fx.put_original(fx.user, "a.jpg", b"aaa", Some(fx.meta("a.jpg"))).await;
    let a = fx.insert_row(fx.user, &k, &s, "a.jpg").await;
    let sel = make_share(&fx, fx.user, "revoked-later-cccccccccccccccc", None, "old", None).await;
    sqlx::query("INSERT INTO share_assets (share_id, asset_id) VALUES ($1, $2)")
        .bind(sel)
        .bind(a)
        .execute(&fx.pool)
        .await
        .unwrap();
    // Yesterday's manifest lists the link; today's (written after the owner revoked it) does not.
    let (older, _) = manifest::build(&fx.pool, fx.user).await.unwrap();
    fx.storage
        .put_bytes(&keys::manifest(fx.user, "20200101"), older, "application/x-ndjson")
        .await
        .unwrap();
    sqlx::query("DELETE FROM shares").execute(&fx.pool).await.unwrap();
    let (newer, _) = manifest::build(&fx.pool, fx.user).await.unwrap();
    fx.storage
        .put_bytes(&keys::manifest(fx.user, "20200102"), newer, "application/x-ndjson")
        .await
        .unwrap();

    sqlx::query("DELETE FROM assets").execute(&fx.pool).await.unwrap();
    let r = fx.run(|o| o.apply = true).await;
    assert_eq!((r.orphans_recreated, r.shares_restored, r.errors), (1, 0, 0));
    assert_eq!(fx.count("shares").await, 0, "a revoked link stays revoked");
    fx.teardown().await;
}

#[tokio::test]
async fn share_links_are_only_restored_into_an_account_that_has_none() {
    let Some(fx) = Fx::new().await else { return };
    let (k, s) = fx.put_original(fx.user, "a.jpg", b"aaa", Some(fx.meta("a.jpg"))).await;
    let a = fx.insert_row(fx.user, &k, &s, "a.jpg").await;
    let kept = make_share(&fx, fx.user, "kept-token-dddddddddddddddddd", None, "kept", None).await;
    sqlx::query("INSERT INTO share_assets (share_id, asset_id) VALUES ($1, $2)")
        .bind(kept)
        .bind(a)
        .execute(&fx.pool)
        .await
        .unwrap();
    let gone = make_share(&fx, fx.user, "gone-token-eeeeeeeeeeeeeeeeeee", None, "gone", None).await;
    sqlx::query("INSERT INTO share_assets (share_id, asset_id) VALUES ($1, $2)")
        .bind(gone)
        .bind(a)
        .execute(&fx.pool)
        .await
        .unwrap();
    manifest::write_user(&fx.pool, &fx.storage, fx.user, 14).await.unwrap();
    // The owner revokes one link after the manifest; the database (a dump taken after that) lacks it.
    sqlx::query("DELETE FROM shares WHERE id = $1")
        .bind(gone)
        .execute(&fx.pool)
        .await
        .unwrap();
    let r = fx.run(|o| o.apply = true).await;
    assert_eq!((r.shares_restored, r.errors), (0, 0));
    assert_eq!(
        fx.count("shares").await,
        1,
        "the revoked link is not resurrected from the manifest"
    );
    fx.teardown().await;
}

#[tokio::test]
async fn old_manifests_without_share_or_account_lines_still_reindex() {
    let Some(fx) = Fx::new().await else { return };
    let (_k, s) = fx.put_original(fx.user, "a.jpg", b"aaa", None).await;
    let id = Uuid::new_v4();
    // Exactly what version 1 wrote: no share lines, `v` 1, and a line type from the future that every reader skips.
    let v1 = format!(
        "{{\"t\":\"meta\",\"v\":1,\"user\":\"{u}\",\"generated_at\":\"2026-01-01T00:00:00Z\",\"assets\":1}}\n\
         {{\"t\":\"asset\",\"id\":\"{id}\",\"sha256\":\"{sha}\",\"key\":\"k\",\"name\":\"Old name.jpg\",\"content_type\":\"image/jpeg\",\"size_bytes\":3,\"favorite\":true}}\n\
         {{\"t\":\"album\",\"id\":\"{album}\",\"title\":\"Old album\",\"items\":[\"{sha}\"]}}\n\
         {{\"t\":\"from_the_future\"}}\n",
        u = fx.user,
        sha = keys::hex(&s),
        album = Uuid::new_v4()
    );
    fx.storage
        .put_bytes(
            &keys::manifest(fx.user, "20260101"),
            v1.into_bytes(),
            "application/x-ndjson",
        )
        .await
        .unwrap();
    let r = fx.run(|o| o.apply = true).await;
    assert_eq!(
        (
            r.orphans_recreated,
            r.albums_restored,
            r.favorites_restored,
            r.users_in_manifest,
            r.shares_restored,
            r.errors
        ),
        (1, 1, 1, 0, 0, 0)
    );
    assert_eq!(r.manifest_bad_lines, 1, "only the line type from the future");
    let name: String = sqlx::query_scalar("SELECT original_name FROM assets WHERE id = $1")
        .bind(id)
        .fetch_one(&fx.pool)
        .await
        .unwrap();
    assert_eq!(name, "Old name.jpg");
    fx.teardown().await;
}

#[tokio::test]
async fn an_email_taken_by_a_new_account_is_not_overwritten_and_an_empty_database_never_writes_an_account_manifest() {
    let Some(fx) = Fx::new().await else { return };
    let (_k, _s) = fx.put_original(fx.user, "a.jpg", b"aaa", Some(fx.meta("a.jpg"))).await;
    cyanotype_core::identity::write(&fx.pool, &fx.storage, 14)
        .await
        .unwrap();
    // The owner registered again (under the old runbook) before the reindex: same email, a new id.
    sqlx::query("DELETE FROM users").execute(&fx.pool).await.unwrap();
    let fresh = make_user(&fx.pool, "OWNER@example.com").await;
    let r = fx.run(|o| o.apply = true).await;
    assert_eq!((r.users_restored, r.users_conflict, r.errors), (0, 1, 0));
    assert_eq!(
        (r.orphans_unknown_user, r.orphans_recreated),
        (1, 0),
        "the photos still need --assign-to, which says so"
    );
    let r = fx
        .run(|o| {
            o.apply = true;
            o.assign_to = Some(fresh);
        })
        .await;
    assert_eq!(r.orphans_reassigned, 1);

    // A database with no users has nothing to protect: it must not replace the real accounts as the newest file.
    sqlx::query("DELETE FROM users").execute(&fx.pool).await.unwrap();
    let before = cyanotype_core::identity::list(&fx.storage).await.unwrap();
    assert_eq!(
        cyanotype_core::identity::write(&fx.pool, &fx.storage, 14)
            .await
            .unwrap(),
        0
    );
    assert_eq!(cyanotype_core::identity::list(&fx.storage).await.unwrap(), before);
    fx.teardown().await;
}

#[tokio::test]
async fn reset_links_replace_each_other_and_store_only_a_hash() {
    let Some(fx) = Fx::new().await else { return };
    let first = cyanotype_core::identity::create_reset_link(&fx.pool, "Owner@Example.com", 24)
        .await
        .unwrap();
    let second = cyanotype_core::identity::create_reset_link(&fx.pool, "owner@example.com", 1)
        .await
        .unwrap();
    assert_ne!(first.token, second.token);
    assert_eq!(first.token.len(), 64);
    let live: i64 =
        sqlx::query_scalar("SELECT count(*) FROM password_resets WHERE used_at IS NULL AND expires_at > now()")
            .fetch_one(&fx.pool)
            .await
            .unwrap();
    assert_eq!(live, 1, "an earlier unused link stops working");
    let stored: Vec<u8> = sqlx::query_scalar("SELECT token_hash FROM password_resets WHERE expires_at > now()")
        .fetch_one(&fx.pool)
        .await
        .unwrap();
    assert_eq!(stored, Sha256::digest(second.token.as_bytes()).to_vec());
    assert!(
        cyanotype_core::identity::create_reset_link(&fx.pool, "nobody@example.com", 1)
            .await
            .is_err()
    );
    fx.teardown().await;
}

/// Hidden and archived survive a bucket-only rebuild: a rebuilt library must not show a photo its owner hid.
#[tokio::test]
async fn hidden_and_archived_come_back_with_a_rebuilt_database() {
    let Some(fx) = Fx::new().await else { return };
    let mut rows = Vec::new();
    for (name, body) in [
        ("plain.jpg", &b"plain"[..]),
        ("hidden.jpg", b"hidden"),
        ("archived.jpg", b"archived"),
        ("both.jpg", b"both"),
        ("IMG_9.jpg", b"pair-jpg"),
        ("IMG_9.dng", b"pair-raw"),
    ] {
        let (k, s) = fx.put_original(fx.user, name, body, None).await;
        rows.push((name, fx.insert_row(fx.user, &k, &s, name).await));
    }
    let id = |n: &str| rows.iter().find(|r| r.0 == n).unwrap().1;
    // A pair is hidden through its visible photo, and the RAW takes the state with it (as `POST /assets/hide` does).
    sqlx::query("UPDATE assets SET primary_id = $2 WHERE id = $1")
        .bind(id("IMG_9.dng"))
        .bind(id("IMG_9.jpg"))
        .execute(&fx.pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE assets SET hidden = true WHERE original_name IN ('hidden.jpg', 'both.jpg', 'IMG_9.jpg', 'IMG_9.dng')",
    )
    .execute(&fx.pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE assets SET archived = true, archived_at = now() WHERE original_name IN ('archived.jpg', 'both.jpg')",
    )
    .execute(&fx.pool)
    .await
    .unwrap();
    let album: Uuid =
        sqlx::query_scalar("INSERT INTO albums (owner_id, title, cover_asset_id) VALUES ($1,'Trip',$2) RETURNING id")
            .bind(fx.user)
            .bind(id("plain.jpg"))
            .fetch_one(&fx.pool)
            .await
            .unwrap();
    for n in ["plain.jpg", "hidden.jpg", "archived.jpg"] {
        sqlx::query("INSERT INTO album_assets (album_id, asset_id) VALUES ($1,$2)")
            .bind(album)
            .bind(id(n))
            .execute(&fx.pool)
            .await
            .unwrap();
    }

    // The manifest says it, in one additive field per state, and an old reader would only skip them.
    manifest::write_user(&fx.pool, &fx.storage, fx.user, 14).await.unwrap();
    let key = manifest::list_all(&fx.storage).await.unwrap()[&fx.user][0].clone();
    let text = String::from_utf8(fx.storage.get_bytes_limited(&key, 1 << 20).await.unwrap().to_vec()).unwrap();
    assert_eq!(text.matches("\"hidden\":true").count(), 4, "{text}");
    assert_eq!(text.matches("\"archived\":true").count(), 2, "{text}");

    // The disaster: the database loses everything about photos.
    sqlx::query("DELETE FROM assets").execute(&fx.pool).await.unwrap();
    sqlx::query("DELETE FROM albums").execute(&fx.pool).await.unwrap();

    let dry = fx.run(|_| {}).await;
    assert_eq!((dry.hidden_restored, dry.archived_restored), (4, 2));
    assert_eq!(fx.count("assets").await, 0, "a dry run writes nothing");

    let r = fx.run(|o| o.apply = true).await;
    assert_eq!((r.orphans_recreated, r.errors), (6, 0));
    assert_eq!((r.hidden_restored, r.archived_restored), (4, 2));
    let flags: Vec<(String, bool, bool, bool)> =
        sqlx::query_as("SELECT original_name, hidden, archived, archived_at IS NOT NULL FROM assets ORDER BY original_name COLLATE \"C\"")
            .fetch_all(&fx.pool)
            .await
            .unwrap();
    assert_eq!(
        flags,
        [
            ("IMG_9.dng".to_string(), true, false, false),
            ("IMG_9.jpg".to_string(), true, false, false),
            ("archived.jpg".to_string(), false, true, true),
            ("both.jpg".to_string(), true, true, true),
            ("hidden.jpg".to_string(), true, false, false),
            ("plain.jpg".to_string(), false, false, false),
        ]
    );
    // What a rebuilt timeline would list: only the plain photo.
    let visible: Vec<String> = sqlx::query_scalar(
        "SELECT original_name FROM assets
         WHERE deleted_at IS NULL AND primary_id IS NULL AND NOT hidden AND NOT archived",
    )
    .fetch_all(&fx.pool)
    .await
    .unwrap();
    assert_eq!(visible, ["plain.jpg"]);
    // Albums come back with every member (a hidden one stays a member; the album views leave it out).
    let members: i64 = sqlx::query_scalar("SELECT count(*) FROM album_assets WHERE album_id = $1")
        .bind(album)
        .fetch_one(&fx.pool)
        .await
        .unwrap();
    assert_eq!(members, 3);
    // A second run has nothing to do, and the state stays.
    let again = fx.run(|o| o.apply = true).await;
    assert_eq!(
        (again.orphans, again.hidden_restored, again.archived_restored),
        (0, 0, 0)
    );
    let hidden: i64 = sqlx::query_scalar("SELECT count(*) FROM assets WHERE hidden")
        .fetch_one(&fx.pool)
        .await
        .unwrap();
    assert_eq!(hidden, 4);
    fx.teardown().await;
}

/// A manifest written before hidden and archived existed has neither field: nothing is hidden or archived by it.
#[tokio::test]
async fn manifests_without_hidden_and_archived_still_read_and_hide_nothing() {
    let Some(fx) = Fx::new().await else { return };
    let (_k, s) = fx.put_original(fx.user, "a.jpg", b"aaa", None).await;
    let id = Uuid::new_v4();
    let old = format!(
        "{{\"t\":\"meta\",\"v\":2,\"user\":\"{u}\",\"generated_at\":\"2026-01-01T00:00:00Z\",\"assets\":1}}\n\
         {{\"t\":\"asset\",\"id\":\"{id}\",\"sha256\":\"{sha}\",\"key\":\"k\",\"name\":\"Before.jpg\",\"content_type\":\"image/jpeg\",\"size_bytes\":3,\"favorite\":false}}\n",
        u = fx.user,
        sha = keys::hex(&s),
    );
    fx.storage
        .put_bytes(
            &keys::manifest(fx.user, "20260101"),
            old.into_bytes(),
            "application/x-ndjson",
        )
        .await
        .unwrap();
    let r = fx.run(|o| o.apply = true).await;
    assert_eq!(
        (r.orphans_recreated, r.hidden_restored, r.archived_restored, r.errors),
        (1, 0, 0, 0)
    );
    let (hidden, archived): (bool, bool) = sqlx::query_as("SELECT hidden, archived FROM assets WHERE id = $1")
        .bind(id)
        .fetch_one(&fx.pool)
        .await
        .unwrap();
    assert!(!hidden && !archived);
    fx.teardown().await;
}

/// Star ratings (migration 0024) survive a bucket-only rebuild, with who set them; an unrated photo and a manifest from before
/// ratings existed bring nothing back.
#[tokio::test]
async fn star_ratings_come_back_with_a_rebuilt_database() {
    let Some(fx) = Fx::new().await else { return };
    for (name, body) in [
        ("five.jpg", &b"five"[..]),
        ("file3.jpg", b"file3"),
        ("cleared.jpg", b"cleared"),
        ("none.jpg", b"none"),
    ] {
        let (k, s) = fx.put_original(fx.user, name, body, None).await;
        fx.insert_row(fx.user, &k, &s, name).await;
    }
    for (name, stars, source) in [
        ("five.jpg", 5i16, Some("user")),
        ("file3.jpg", 3, Some("file")),
        ("cleared.jpg", 0, Some("user")),
    ] {
        sqlx::query("UPDATE assets SET rating = $2, rating_source = $3 WHERE original_name = $1")
            .bind(name)
            .bind(stars)
            .bind(source)
            .execute(&fx.pool)
            .await
            .unwrap();
    }
    manifest::write_user(&fx.pool, &fx.storage, fx.user, 14).await.unwrap();
    let key = manifest::list_all(&fx.storage).await.unwrap()[&fx.user][0].clone();
    let text = String::from_utf8(fx.storage.get_bytes_limited(&key, 1 << 20).await.unwrap().to_vec()).unwrap();
    // Additive fields on the asset lines, and only where they say something.
    assert_eq!(text.matches("\"rating\":").count(), 3, "{text}");
    assert_eq!(
        text.matches("\"rating\":0,").count(),
        1,
        "a rating cleared on purpose is kept: {text}"
    );
    assert_eq!(text.matches("\"rating_source\":\"file\"").count(), 1, "{text}");

    sqlx::query("DELETE FROM assets").execute(&fx.pool).await.unwrap();
    let dry = fx.run(|_| {}).await;
    assert_eq!(dry.ratings_restored, 3);
    assert_eq!(fx.count("assets").await, 0);
    let r = fx.run(|o| o.apply = true).await;
    assert_eq!((r.orphans_recreated, r.ratings_restored, r.errors), (4, 3, 0));
    let got: Vec<(String, i16, Option<String>)> =
        sqlx::query_as("SELECT original_name, rating, rating_source FROM assets ORDER BY original_name COLLATE \"C\"")
            .fetch_all(&fx.pool)
            .await
            .unwrap();
    assert_eq!(
        got,
        [
            ("cleared.jpg".to_string(), 0, Some("user".to_string())),
            ("file3.jpg".to_string(), 3, Some("file".to_string())),
            ("five.jpg".to_string(), 5, Some("user".to_string())),
            ("none.jpg".to_string(), 0, None),
        ]
    );
    // A second run restores nothing more and changes nothing.
    let again = fx.run(|o| o.apply = true).await;
    assert_eq!((again.orphans, again.ratings_restored), (0, 0));
    fx.teardown().await;
}

/// Tags come back with a rebuilt database: the tag lines of the manifest carry names, the photos (by hash), the imported mark and
/// empty tags; a restored photo's keywords are not read from the file again; and a tag that exists is only given back to the photos
/// this run recreated.
#[tokio::test]
async fn tags_come_back_with_a_rebuilt_database() {
    let Some(fx) = Fx::new().await else { return };
    let mut ids = Vec::new();
    for (name, body) in [
        ("p1.jpg", &b"p1"[..]),
        ("p2.jpg", b"p2"),
        ("p3.jpg", b"p3"),
        ("p4.jpg", b"p4"),
    ] {
        let (k, s) = fx.put_original(fx.user, name, body, None).await;
        ids.push(fx.insert_row(fx.user, &k, &s, name).await);
    }
    let tag = |name: &'static str| {
        let pool = fx.pool.clone();
        let owner = fx.user;
        async move {
            sqlx::query_scalar::<_, Uuid>(
                "INSERT INTO tags (owner_id, name) VALUES ($1, $2) ON CONFLICT (owner_id, lower(name)) DO UPDATE SET name = tags.name RETURNING id",
            )
            .bind(owner)
            .bind(name)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    let (holiday, kw, _empty) = (tag("Holiday").await, tag("From file").await, tag("Empty").await);
    for (asset, tag_id, imported) in [
        (ids[0], holiday, false),
        (ids[1], holiday, false),
        (ids[2], holiday, true),
        (ids[0], kw, true),
    ] {
        sqlx::query("INSERT INTO asset_tags (asset_id, tag_id, imported) VALUES ($1,$2,$3)")
            .bind(asset)
            .bind(tag_id)
            .bind(imported)
            .execute(&fx.pool)
            .await
            .unwrap();
    }
    manifest::write_user(&fx.pool, &fx.storage, fx.user, 14).await.unwrap();
    let key = manifest::list_all(&fx.storage).await.unwrap()[&fx.user][0].clone();
    let text = String::from_utf8(fx.storage.get_bytes_limited(&key, 1 << 20).await.unwrap().to_vec()).unwrap();
    assert!(text.contains("\"v\":3"), "{text}");
    assert_eq!(
        text.matches("\"t\":\"tag\"").count(),
        3,
        "one line per tag, an empty one too: {text}"
    );
    // The loaded manifest knows tags, and only the newest file's lines count.
    let loaded = manifest::load(&fx.storage, fx.user, std::slice::from_ref(&key))
        .await
        .unwrap();
    assert!(loaded.tags_known);
    assert_eq!(loaded.tags.len(), 3);
    assert_eq!(loaded.tags[&holiday].items.len(), 2);
    assert_eq!(loaded.tags[&holiday].imported.len(), 1);

    // The disaster: photos and tags are gone.
    sqlx::query("DELETE FROM assets").execute(&fx.pool).await.unwrap();
    sqlx::query("DELETE FROM tags").execute(&fx.pool).await.unwrap();
    let dry = fx.run(|_| {}).await;
    assert_eq!((dry.tags_restored, dry.tag_links_restored), (3, 4));
    assert_eq!(fx.count("tags").await, 0, "a dry run writes nothing");
    let r = fx.run(|o| o.apply = true).await;
    assert_eq!(
        (r.orphans_recreated, r.tags_restored, r.tag_links_restored, r.errors),
        (4, 3, 4, 0)
    );
    let links: Vec<(String, String, bool)> = sqlx::query_as(
        "SELECT t.name, a.original_name, at.imported FROM asset_tags at JOIN tags t ON t.id = at.tag_id
         JOIN assets a ON a.id = at.asset_id ORDER BY t.name, a.original_name",
    )
    .fetch_all(&fx.pool)
    .await
    .unwrap();
    assert_eq!(
        links,
        [
            ("From file".to_string(), "p1.jpg".to_string(), true),
            ("Holiday".to_string(), "p1.jpg".to_string(), false),
            ("Holiday".to_string(), "p2.jpg".to_string(), false),
            ("Holiday".to_string(), "p3.jpg".to_string(), true),
        ]
    );
    // The empty tag and the tags' ids survive, and the photos' keywords count as read.
    let same_id: Uuid = sqlx::query_scalar("SELECT id FROM tags WHERE name = 'Holiday'")
        .fetch_one(&fx.pool)
        .await
        .unwrap();
    assert_eq!(same_id, holiday);
    assert_eq!(fx.count("tags").await, 3);
    let unread: i64 = sqlx::query_scalar("SELECT count(*) FROM assets WHERE tags_imported_at IS NULL")
        .fetch_one(&fx.pool)
        .await
        .unwrap();
    assert_eq!(
        unread, 0,
        "the manifest knows their tags, so the metadata job does not import keywords again"
    );
    // A second run has nothing to do.
    let again = fx.run(|o| o.apply = true).await;
    assert_eq!(
        (again.orphans, again.tags_restored, again.tag_links_restored),
        (0, 0, 0)
    );
    fx.teardown().await;
}

/// A tag the owner already has is not changed by a run that recreates no photo of it, and one that recreates a photo only tags that
/// photo (the owner may have removed the tag from the others since the manifest was written).
#[tokio::test]
async fn an_existing_tag_is_only_given_to_photos_that_were_recreated() {
    let Some(fx) = Fx::new().await else { return };
    let mut ids = Vec::new();
    for (name, body) in [("a.jpg", &b"aa"[..]), ("b.jpg", b"bb")] {
        let (k, s) = fx.put_original(fx.user, name, body, None).await;
        ids.push(fx.insert_row(fx.user, &k, &s, name).await);
    }
    let holiday: Uuid = sqlx::query_scalar("INSERT INTO tags (owner_id, name) VALUES ($1, 'Holiday') RETURNING id")
        .bind(fx.user)
        .fetch_one(&fx.pool)
        .await
        .unwrap();
    for id in &ids {
        sqlx::query("INSERT INTO asset_tags (asset_id, tag_id) VALUES ($1, $2)")
            .bind(id)
            .bind(holiday)
            .execute(&fx.pool)
            .await
            .unwrap();
    }
    manifest::write_user(&fx.pool, &fx.storage, fx.user, 14).await.unwrap();
    // Since then the owner took Holiday off photo a; then photo b is lost from the database.
    sqlx::query("DELETE FROM asset_tags WHERE asset_id = $1")
        .bind(ids[0])
        .execute(&fx.pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM assets WHERE id = $1")
        .bind(ids[1])
        .execute(&fx.pool)
        .await
        .unwrap();
    let r = fx.run(|o| o.apply = true).await;
    assert_eq!(
        (r.orphans_recreated, r.tags_restored, r.tag_links_restored, r.errors),
        (1, 0, 1, 0)
    );
    let names: Vec<String> = sqlx::query_scalar(
        "SELECT a.original_name FROM asset_tags at JOIN assets a ON a.id = at.asset_id ORDER BY a.original_name",
    )
    .fetch_all(&fx.pool)
    .await
    .unwrap();
    assert_eq!(names, ["b.jpg"], "photo a stays without the tag the owner took off");
    fx.teardown().await;
}

/// A smart album's definition (its saved rules) survives a bucket-only rebuild; it has no members to restore, and its link keeps working.
#[tokio::test]
async fn smart_albums_come_back_with_their_rules() {
    let Some(fx) = Fx::new().await else { return };
    let (k, s) = fx.put_original(fx.user, "a.jpg", b"aaa", None).await;
    fx.insert_row(fx.user, &k, &s, "a.jpg").await;
    let rules = serde_json::json!({"match": "any", "archived": "exclude",
        "rules": [{"field": "place", "value": "Lisbon"}, {"field": "rating", "min": 4}]});
    let smart: Uuid = sqlx::query_scalar(
        "INSERT INTO albums (owner_id, title, rules) VALUES ($1, 'Lisbon or great', $2) RETURNING id",
    )
    .bind(fx.user)
    .bind(&rules)
    .fetch_one(&fx.pool)
    .await
    .unwrap();
    let plain: Uuid = sqlx::query_scalar("INSERT INTO albums (owner_id, title) VALUES ($1, 'Plain') RETURNING id")
        .bind(fx.user)
        .fetch_one(&fx.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO shares (owner_id, token, kind, album_id) VALUES ($1, 'tok-smart-0123456789abcdef', 'album', $2)",
    )
    .bind(fx.user)
    .bind(smart)
    .execute(&fx.pool)
    .await
    .unwrap();
    manifest::write_user(&fx.pool, &fx.storage, fx.user, 14).await.unwrap();
    let key = manifest::list_all(&fx.storage).await.unwrap()[&fx.user][0].clone();
    let text = String::from_utf8(fx.storage.get_bytes_limited(&key, 1 << 20).await.unwrap().to_vec()).unwrap();
    assert_eq!(
        text.matches("\"items\":[],\"rules\":").count(),
        1,
        "only the smart album carries rules: {text}"
    );

    sqlx::query("DELETE FROM assets").execute(&fx.pool).await.unwrap();
    sqlx::query("DELETE FROM albums").execute(&fx.pool).await.unwrap();
    let r = fx.run(|o| o.apply = true).await;
    assert_eq!((r.albums_restored, r.smart_albums_restored, r.errors), (2, 1, 0));
    let got: Vec<(Uuid, Option<serde_json::Value>)> = sqlx::query_as("SELECT id, rules FROM albums ORDER BY title")
        .fetch_all(&fx.pool)
        .await
        .unwrap();
    assert_eq!(got, [(smart, Some(rules)), (plain, None)]);
    let members: i64 = sqlx::query_scalar("SELECT count(*) FROM album_assets")
        .fetch_one(&fx.pool)
        .await
        .unwrap();
    assert_eq!(members, 0, "nothing is materialised for a smart album");
    let link: i64 =
        sqlx::query_scalar("SELECT count(*) FROM shares WHERE token = 'tok-smart-0123456789abcdef' AND album_id = $1")
            .bind(smart)
            .fetch_one(&fx.pool)
            .await
            .unwrap();
    assert_eq!(link, 1, "the link to the smart album is back");
    fx.teardown().await;
}
