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
        "{}",
        dry.text()
    );
    assert_eq!(fx.count("assets").await, 0);

    let r = fx.run(|o| o.apply = true).await;
    assert_eq!(
        (r.orphans_recreated, r.edits_objects, r.edits_restored, r.errors),
        (2, 4, 1, 0),
        "{}",
        r.text()
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
