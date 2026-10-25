//! The storage integrity check against a real Postgres and a real S3 API (versioning on, like production).
//! Needs `DATABASE_URL` (role may CREATE DATABASE) and `S3_ENDPOINT`, `S3_ACCESS_KEY`, `S3_SECRET_KEY`
//! (+ `S3_FORCE_PATH_STYLE=true` for SeaweedFS). Each test makes its own bucket and database; without both variables the
//! tests return early. Names are code words, never people.
use std::time::Duration;

use aws_sdk_s3::{
    Client,
    config::{Credentials, Region},
    types::{BucketVersioningStatus, VersioningConfiguration},
};
use cyanotype_core::{
    admin_tasks,
    config::S3Config,
    db,
    integrity::{self, ActError, Action, Options, Summary},
    jobs, keys, objmeta,
    storage::Storage,
};
use sha2::{Digest, Sha256};
use sqlx::{Connection, Executor, PgConnection, PgPool};
use uuid::Uuid;

struct Fx {
    pool: PgPool,
    admin_url: String,
    db_name: String,
    storage: Storage,
    client: Client,
    bucket: String,
    user: Uuid,
    admin: Uuid,
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
        let user = make_user(&pool, "alpha@example.com").await;
        let admin = make_user(&pool, "bravo@example.com").await;
        Some(Self {
            pool,
            admin_url,
            db_name,
            storage: Storage::new(&cfg),
            client,
            bucket: cfg.bucket,
            user,
            admin,
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

    /// Store `body` as an original (a plain put) at its content-addressed key; returns (key, sha256).
    async fn put_original(&self, owner: Uuid, filename: &str, body: &[u8]) -> (String, [u8; 32]) {
        let sha: [u8; 32] = Sha256::digest(body).into();
        let key = keys::original(owner, &sha, &keys::sanitize_ext(filename));
        self.storage.put_bytes(&key, body.to_vec(), "image/jpeg").await.unwrap();
        (key, sha)
    }

    /// A row for `owner`; the thumbnail job is `done` unless `job` says otherwise (`None`: no job at all).
    async fn row(&self, owner: Uuid, key: &str, sha: &[u8; 32], size: i64, job: Option<&str>) -> Uuid {
        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO assets (owner_id, sha256, original_key, original_name, content_type, size_bytes)
             VALUES ($1,$2,$3,'photo.jpg','image/jpeg',$4) RETURNING id",
        )
        .bind(owner)
        .bind(&sha[..])
        .bind(key)
        .bind(size)
        .fetch_one(&self.pool)
        .await
        .unwrap();
        if let Some(state) = job {
            sqlx::query(
                "INSERT INTO jobs (kind, asset_id, state, finished_at) VALUES ('thumbnail', $1, $2, CASE WHEN $2 = 'done' THEN now() END)",
            )
            .bind(id)
            .bind(state)
            .execute(&self.pool)
            .await
            .unwrap();
        }
        id
    }

    async fn derived(&self, owner: Uuid, asset: Uuid) {
        for k in [keys::thumb(owner, asset), keys::preview(owner, asset)] {
            self.storage
                .put_bytes(&k, b"webp".to_vec(), "image/webp")
                .await
                .unwrap();
        }
    }

    /// A photo that is exactly as it should be: right bytes, right row, derived images present.
    async fn good(&self, owner: Uuid, name: &str, body: &[u8]) -> Uuid {
        let (key, sha) = self.put_original(owner, name, body).await;
        let id = self.row(owner, &key, &sha, body.len() as i64, Some("done")).await;
        self.derived(owner, id).await;
        id
    }

    fn opts(&self) -> Options {
        Options {
            min_age: Duration::ZERO,
            yield_to_jobs: false,
            ..Options::default()
        }
    }

    async fn check(&self, opts: &Options) -> Summary {
        let run = integrity::begin(&self.pool, "manual", Some(self.admin)).await.unwrap();
        integrity::run(&self.pool, &self.storage, run, opts).await.unwrap()
    }

    async fn findings(&self, state: &str) -> Vec<integrity::Finding> {
        integrity::list_findings(&self.pool, None, state, None, 200)
            .await
            .unwrap()
    }

    async fn of_kind(&self, kind: &str) -> Vec<integrity::Finding> {
        self.findings("open")
            .await
            .into_iter()
            .filter(|f| f.kind == kind)
            .collect()
    }

    async fn verified(&self, id: Uuid) -> bool {
        sqlx::query_scalar::<_, bool>("SELECT verified_at IS NOT NULL FROM assets WHERE id = $1")
            .bind(id)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    async fn act(&self, finding: i64, a: Action) -> Result<integrity::Acted, ActError> {
        integrity::act(&self.pool, &self.storage, finding, a, self.admin, false).await
    }

    async fn delete_object(&self, key: &str) {
        // A plain delete: on a versioned bucket a delete marker over the old versions.
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .unwrap();
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
async fn every_kind_of_finding_is_detected_and_a_clean_library_reports_nothing() {
    let Some(fx) = Fx::new().await else { return };
    let ok = fx.good(fx.user, "ok.jpg", b"alpha one").await;

    let clean = fx.check(&fx.opts()).await;
    assert_eq!(clean.open.values().sum::<i64>(), 0, "{clean:?}");
    assert_eq!(
        (clean.objects_listed, clean.assets_checked, clean.assets_verified),
        (1, 1, 1)
    );
    assert_eq!(clean.derived_requeued, 0);
    assert!(fx.verified(ok).await);

    // (a) a row whose original is gone, with an earlier version to restore
    let (gone_key, gone_sha) = fx.put_original(fx.user, "gone.jpg", b"bravo two").await;
    let gone = fx.row(fx.user, &gone_key, &gone_sha, 9, Some("done")).await;
    fx.derived(fx.user, gone).await;
    fx.delete_object(&gone_key).await;
    // (b) an object nobody points at, a stray key, and a copy of content that is indexed under another key
    let (orphan_key, _) = fx.put_original(fx.user, "orphan.jpg", b"charlie three").await;
    fx.storage
        .put_bytes("originals/readme.txt", b"hi".to_vec(), "text/plain")
        .await
        .unwrap();
    let (ok_key, ok_sha): (String, Vec<u8>) = sqlx::query_as("SELECT original_key, sha256 FROM assets WHERE id = $1")
        .bind(ok)
        .fetch_one(&fx.pool)
        .await
        .unwrap();
    let alias_key = keys::original(fx.user, &ok_sha.clone().try_into().unwrap(), "png");
    fx.storage
        .put_bytes(&alias_key, b"alpha one".to_vec(), "image/png")
        .await
        .unwrap();
    // (c) a file of the wrong size, and one of the right size with other bytes
    let (short_key, short_sha) = fx.put_original(fx.user, "short.jpg", b"delta").await;
    let short = fx.row(fx.user, &short_key, &short_sha, 99, Some("done")).await;
    fx.derived(fx.user, short).await;
    let (rot_key, _) = fx.put_original(fx.user, "rot.jpg", b"echo four").await;
    let rot_sha: [u8; 32] = Sha256::digest(b"echo FOUR").into();
    let rot = fx.row(fx.user, &rot_key, &rot_sha, 9, Some("done")).await;
    fx.derived(fx.user, rot).await;

    let s = fx.check(&fx.opts()).await;
    assert_eq!(s.open[integrity::KIND_MISSING_ORIGINAL], 1, "{s:?}");
    // The fourth: the corrupted file sits at the key of the bytes it should have (the row says other content), so no row points at it by content.
    assert_eq!(s.open[integrity::KIND_UNTRACKED], 4, "{s:?}");
    assert_eq!(s.open[integrity::KIND_CHECKSUM], 2, "{s:?}");

    let missing = &fx.of_kind(integrity::KIND_MISSING_ORIGINAL).await[0];
    assert_eq!(
        (missing.key.as_str(), missing.asset_id, missing.owner_id),
        (gone_key.as_str(), Some(gone), Some(fx.user))
    );
    assert_eq!(missing.owner_email.as_deref(), Some("alpha@example.com"));
    assert_eq!(missing.detail["previous_versions"], 1, "the old version is offered");

    let untracked = fx.of_kind(integrity::KIND_UNTRACKED).await;
    let reason = |key: &str| {
        untracked
            .iter()
            .find(|f| f.key == key)
            .unwrap_or_else(|| panic!("{key} not flagged"))
            .detail["reason"]
            .clone()
    };
    assert_eq!(reason(&orphan_key), "no_row");
    assert_eq!(reason("originals/readme.txt"), "unrecognised_key");
    assert_eq!(reason(&alias_key), "duplicate_content");
    assert!(
        !untracked.iter().any(|f| f.key == ok_key),
        "the indexed original is fine"
    );

    let sums = fx.of_kind(integrity::KIND_CHECKSUM).await;
    let by = |key: &str| {
        sums.iter()
            .find(|f| f.key == key)
            .unwrap_or_else(|| panic!("{key}"))
            .detail
            .clone()
    };
    let size = by(&short_key);
    assert_eq!(
        (
            size["reason"].as_str(),
            size["actual_bytes"].as_i64(),
            size["size_bytes"].as_i64()
        ),
        (Some("size"), Some(5), Some(99))
    );
    let hash = by(&rot_key);
    assert_eq!(hash["reason"], "hash");
    assert_eq!(hash["expected_sha256"], keys::hex(&rot_sha));
    assert_eq!(hash["actual_sha256"], keys::hex(&Sha256::digest(b"echo four")));

    // The run record carries the counts.
    let run = integrity::last_run(&fx.pool).await.unwrap().unwrap();
    assert_eq!(
        (
            run.state.as_str(),
            run.missing_originals,
            run.untracked_objects,
            run.checksum_mismatches
        ),
        ("done", 1, 4, 2)
    );
    assert!(run.finished_at.is_some() && run.bytes_hashed > 0);

    // A second run raises nothing again: same findings, updated, not duplicated.
    let again = fx.check(&fx.opts()).await;
    assert_eq!(again.open, s.open);
    assert_eq!(fx.findings("open").await.len(), 7);
    fx.teardown().await;
}

#[tokio::test]
async fn what_the_app_keeps_or_is_removing_is_not_flagged() {
    let Some(fx) = Fx::new().await else { return };
    // Manifests, edits and derived images are the app's own.
    fx.storage
        .put_bytes(
            &keys::manifest(fx.user, "20260101"),
            b"{}".to_vec(),
            "application/x-ndjson",
        )
        .await
        .unwrap();
    fx.storage
        .put_bytes(
            &keys::accounts_manifest("20260101"),
            b"{}".to_vec(),
            "application/x-ndjson",
        )
        .await
        .unwrap();
    fx.storage
        .put_bytes(&keys::edits(fx.user, &[7; 32]), b"{}".to_vec(), "application/json")
        .await
        .unwrap();
    fx.storage
        .put_bytes(&keys::thumb(fx.user, Uuid::new_v4()), b"w".to_vec(), "image/webp")
        .await
        .unwrap();
    // A row in the trash keeps its original; one queued for purging may have lost it, or be losing it right now.
    let (trash_key, trash_sha) = fx.put_original(fx.user, "trash.jpg", b"in the bin").await;
    let trashed = fx.row(fx.user, &trash_key, &trash_sha, 10, None).await;
    sqlx::query("UPDATE assets SET deleted_at = now() WHERE id = $1")
        .bind(trashed)
        .execute(&fx.pool)
        .await
        .unwrap();
    let (purge_key, purge_sha) = fx.put_original(fx.user, "purge.jpg", b"being purged").await;
    let purging = fx.row(fx.user, &purge_key, &purge_sha, 12, None).await;
    sqlx::query("UPDATE assets SET deleted_at = now(), purging_at = now() WHERE id = $1")
        .bind(purging)
        .execute(&fx.pool)
        .await
        .unwrap();
    let (half_key, half_sha) = fx.put_original(fx.user, "half.jpg", b"half purged").await;
    let half = fx.row(fx.user, &half_key, &half_sha, 11, None).await;
    sqlx::query("UPDATE assets SET deleted_at = now(), purging_at = now() WHERE id = $1")
        .bind(half)
        .execute(&fx.pool)
        .await
        .unwrap();
    fx.delete_object(&half_key).await;
    // A trashed row whose original is gone is on its way out: not a missing original.
    let (lost_key, lost_sha) = fx.put_original(fx.user, "lost.jpg", b"lost in the bin").await;
    let lost = fx.row(fx.user, &lost_key, &lost_sha, 15, None).await;
    sqlx::query("UPDATE assets SET deleted_at = now() WHERE id = $1")
        .bind(lost)
        .execute(&fx.pool)
        .await
        .unwrap();
    fx.delete_object(&lost_key).await;
    // An upload that has finished in the bucket and is registering; an account being deleted; and a fresh object.
    let (up_key, _) = fx.put_original(fx.user, "uploading.jpg", b"mid upload").await;
    sqlx::query("INSERT INTO uploads (upload_id, owner_id, sha256, key, size_bytes, part_size, original_name, content_type) VALUES ('u1', $1, sha256('x'), $2, 10, 10, 'a.jpg', 'image/jpeg')")
        .bind(fx.user)
        .bind(&up_key)
        .execute(&fx.pool)
        .await
        .unwrap();
    let leaving = Uuid::new_v4();
    let _ = fx.put_original(leaving, "leaving.jpg", b"owner is leaving").await;
    admin_tasks::enqueue(
        &fx.pool,
        admin_tasks::KIND_DELETE_ACCOUNT,
        Some(fx.admin),
        serde_json::json!({"user_id": leaving, "email": "x@example.com"}),
    )
    .await
    .unwrap();

    let s = fx.check(&fx.opts()).await;
    assert!(
        fx.findings("open").await.is_empty(),
        "nothing to report: {:?}",
        fx.findings("open")
            .await
            .iter()
            .map(|f| (&f.kind, &f.key))
            .collect::<Vec<_>>()
    );
    assert_eq!(s.open.values().sum::<i64>(), 0);

    // Younger than min_age is left alone; once old enough it is a stray.
    fx.put_original(fx.user, "fresh.jpg", b"just landed").await;
    let patient = Options {
        min_age: Duration::from_secs(3600),
        ..fx.opts()
    };
    fx.check(&patient).await;
    assert_eq!(fx.findings("open").await.len(), 0, "too recent");
    fx.check(&fx.opts()).await;
    // The fresh object and the finished-upload-in-flight: only the first is judged (the upload row protects the second).
    let open = fx.findings("open").await;
    assert_eq!(open.len(), 1, "{:?}", open.iter().map(|f| &f.key).collect::<Vec<_>>());
    fx.teardown().await;
}

#[tokio::test]
async fn the_checksum_sample_honours_the_byte_budget_and_advances_verified_at() {
    let Some(fx) = Fx::new().await else { return };
    let mut ids = Vec::new();
    for n in 0..5u8 {
        let body = vec![n + 1; 1000];
        ids.push(fx.good(fx.user, &format!("p{n}.jpg"), &body).await);
    }
    let budget = |bytes: u64| Options {
        verify_bytes: bytes,
        ..fx.opts()
    };
    let verified = |ids: &[Uuid]| {
        let pool = fx.pool.clone();
        let ids = ids.to_vec();
        async move {
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM assets WHERE id = ANY($1) AND verified_at IS NOT NULL")
                .bind(ids)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };

    // Off: nothing is read.
    let off = fx.check(&budget(0)).await;
    assert_eq!((off.assets_verified, off.bytes_hashed), (0, 0));
    assert_eq!(verified(&ids).await, 0);

    // 2,500 bytes: two files of 1,000, not three.
    let r1 = fx.check(&budget(2500)).await;
    assert_eq!((r1.assets_verified, r1.bytes_hashed), (2, 2000));
    assert_eq!(verified(&ids).await, 2);
    // The next run continues with the ones not yet verified, then wraps around to the oldest.
    let r2 = fx.check(&budget(2500)).await;
    assert_eq!((r2.assets_verified, r2.bytes_hashed), (2, 2000));
    assert_eq!(verified(&ids).await, 4);
    let r3 = fx.check(&budget(2500)).await;
    assert_eq!(r3.assets_verified, 2);
    assert_eq!(verified(&ids).await, 5, "every file has been read after three nights");
    // The oldest-verified went first: the one verified in run 1 is read again in run 3 only after the never-verified one.
    let covered = integrity::coverage(&fx.pool).await.unwrap();
    assert_eq!((covered.assets, covered.verified), (5, 5));
    assert!(covered.oldest_verified_at.is_some());

    // A budget smaller than one file still reads one (the first of a run), never more.
    let tiny = fx.check(&budget(10)).await;
    assert_eq!((tiny.assets_verified, tiny.bytes_hashed), (1, 1000));
    assert!(fx.findings("open").await.is_empty());
    fx.teardown().await;
}

#[tokio::test]
async fn a_hash_mismatch_stays_until_the_file_is_right_and_a_run_then_closes_it() {
    let Some(fx) = Fx::new().await else { return };
    let (key, sha) = fx.put_original(fx.user, "damaged.jpg", b"foxtrot six").await;
    let id = fx.row(fx.user, &key, &sha, 11, Some("done")).await;
    fx.derived(fx.user, id).await;
    // Same size, other bytes: a new version over the right one.
    fx.storage
        .put_bytes(&key, b"foxtrot SIX".to_vec(), "image/jpeg")
        .await
        .unwrap();

    fx.check(&fx.opts()).await;
    let f = fx.of_kind(integrity::KIND_CHECKSUM).await;
    assert_eq!((f.len(), f[0].detail["reason"].as_str()), (1, Some("hash")));
    assert_eq!(f[0].detail["previous_versions"], 1);
    // The next night it is still wrong and still one finding; it is not read first every night but is re-read as a finding.
    fx.check(&fx.opts()).await;
    assert_eq!(fx.of_kind(integrity::KIND_CHECKSUM).await.len(), 1);

    // Put the right bytes back by hand: the next run reads it again, finds it right and closes the finding.
    fx.storage
        .put_bytes(&key, b"foxtrot six".to_vec(), "image/jpeg")
        .await
        .unwrap();
    let s = fx.check(&fx.opts()).await;
    assert_eq!(s.open.values().sum::<i64>(), 0);
    let resolved = fx.findings("resolved").await;
    assert_eq!(
        (resolved.len(), resolved[0].resolution.as_deref()),
        (1, Some("verified"))
    );
    fx.teardown().await;
}

#[tokio::test]
async fn a_second_run_resolves_what_was_fixed_meanwhile() {
    let Some(fx) = Fx::new().await else { return };
    let (key, sha) = fx.put_original(fx.user, "back.jpg", b"golf seven").await;
    let id = fx.row(fx.user, &key, &sha, 10, Some("done")).await;
    fx.derived(fx.user, id).await;
    fx.delete_object(&key).await;
    let (stray, _) = fx.put_original(fx.user, "stray.jpg", b"hotel eight").await;

    let first = fx.check(&fx.opts()).await;
    assert_eq!(
        (
            first.open[integrity::KIND_MISSING_ORIGINAL],
            first.open[integrity::KIND_UNTRACKED]
        ),
        (1, 1)
    );

    // Someone puts the original back and removes the stray by hand.
    fx.storage
        .put_bytes(&key, b"golf seven".to_vec(), "image/jpeg")
        .await
        .unwrap();
    fx.storage.delete_all_versions(&[stray]).await.unwrap();
    let second = fx.check(&fx.opts()).await;
    assert_eq!(second.open.values().sum::<i64>(), 0);
    assert_eq!(second.findings_resolved, 2);
    let resolved = fx.findings("resolved").await;
    assert!(
        resolved
            .iter()
            .all(|f| f.resolution.as_deref() == Some("no longer found") && f.resolved_at.is_some())
    );
    fx.teardown().await;
}

#[tokio::test]
async fn missing_thumbnails_are_queued_again_but_failed_or_waiting_jobs_are_left_alone() {
    let Some(fx) = Fx::new().await else { return };
    let mk = |name: &'static str, job: Option<&'static str>| {
        let fx = &fx;
        async move {
            let (key, sha) = fx.put_original(fx.user, name, name.as_bytes()).await;
            fx.row(fx.user, &key, &sha, name.len() as i64, job).await
        }
    };
    let done = mk("done.jpg", Some("done")).await; // job finished, no thumbnail: made again
    let no_job = mk("nojob.jpg", None).await; // never had a job row: queued
    let failed = mk("failed.jpg", Some("failed")).await; // for the failed-jobs list
    let waiting = mk("waiting.jpg", Some("queued")).await; // on its way
    let half = mk("half.jpg", Some("done")).await; // thumbnail but no preview
    fx.storage
        .put_bytes(&keys::thumb(fx.user, half), b"w".to_vec(), "image/webp")
        .await
        .unwrap();
    let fine = fx.good(fx.user, "fine.jpg", b"fine one").await;

    let s = fx.check(&fx.opts()).await;
    assert_eq!(s.derived_requeued, 3, "{s:?}");
    assert_eq!(s.open.values().sum::<i64>(), 0, "a missing thumbnail is not a finding");
    let state = |id: Uuid| {
        let pool = fx.pool.clone();
        async move {
            sqlx::query_scalar::<_, String>("SELECT state FROM jobs WHERE asset_id = $1 AND kind = 'thumbnail'")
                .bind(id)
                .fetch_optional(&pool)
                .await
                .unwrap()
                .unwrap_or_default()
        }
    };
    for id in [done, no_job, half] {
        assert_eq!(state(id).await, "queued");
    }
    assert_eq!(
        (state(failed).await.as_str(), state(waiting).await.as_str()),
        ("failed", "queued")
    );
    assert_eq!(state(fine).await, "done");
    // The queued jobs are the existing derivative job, version 1.
    let (kind, version): (String, i32) = sqlx::query_as("SELECT kind, version FROM jobs WHERE asset_id = $1")
        .bind(no_job)
        .fetch_one(&fx.pool)
        .await
        .unwrap();
    assert_eq!((kind.as_str(), version), (jobs::KIND_THUMBNAIL, 1));
    // Once queued they are not queued twice.
    assert_eq!(fx.check(&fx.opts()).await.derived_requeued, 0);
    fx.teardown().await;
}

#[tokio::test]
async fn restoring_a_previous_version_fixes_a_missing_original_and_a_corrupted_one() {
    let Some(fx) = Fx::new().await else { return };
    // Missing: a delete marker over the good version.
    let (key, sha) = fx.put_original(fx.user, "marker.jpg", b"india nine").await;
    let id = fx.row(fx.user, &key, &sha, 10, Some("done")).await;
    fx.derived(fx.user, id).await;
    fx.delete_object(&key).await;
    // Corrupted: the right bytes, then other bytes of the same size.
    let (key2, sha2) = fx.put_original(fx.user, "bitrot.jpg", b"juliet ten").await;
    let id2 = fx.row(fx.user, &key2, &sha2, 10, Some("done")).await;
    fx.derived(fx.user, id2).await;
    fx.storage
        .put_bytes(&key2, b"juliet TEN".to_vec(), "image/jpeg")
        .await
        .unwrap();
    // Missing and with nothing to restore.
    let (key3, sha3) = fx.put_original(fx.user, "nothing.jpg", b"kilo eleven").await;
    let id3 = fx.row(fx.user, &key3, &sha3, 11, Some("done")).await;
    fx.derived(fx.user, id3).await;
    fx.storage
        .delete_all_versions(std::slice::from_ref(&key3))
        .await
        .unwrap();

    fx.check(&fx.opts()).await;
    let missing = fx.of_kind(integrity::KIND_MISSING_ORIGINAL).await;
    let find = |key: &str| missing.iter().find(|f| f.key == key).unwrap().id;
    let (m1, m3) = (find(&key), find(&key3));
    let c2 = fx.of_kind(integrity::KIND_CHECKSUM).await[0].id;
    assert_eq!(
        missing.iter().find(|f| f.id == m3).unwrap().detail["previous_versions"],
        0
    );

    let r = fx.act(m1, Action::RestoreVersion).await.unwrap();
    assert!(r.changed);
    assert_eq!(
        (r.finding.state.as_str(), r.finding.resolution.as_deref()),
        ("resolved", Some("restored a previous version"))
    );
    let back = fx.storage.hash_object(&key, None, 10).await.unwrap().unwrap();
    assert_eq!(back.sha256, sha);
    assert!(fx.verified(id).await);
    let r2 = fx.act(c2, Action::RestoreVersion).await.unwrap();
    assert!(r2.changed);
    assert_eq!(
        fx.storage.hash_object(&key2, None, 10).await.unwrap().unwrap().sha256,
        sha2
    );

    // Idempotent: again changes nothing; a different action on a resolved finding is refused.
    assert!(!fx.act(m1, Action::RestoreVersion).await.unwrap().changed);
    assert!(matches!(fx.act(m1, Action::Trash).await, Err(ActError::Conflict(_))));
    // No earlier version: refused, finding still open.
    assert!(
        matches!(fx.act(m3, Action::RestoreVersion).await, Err(ActError::Conflict(m)) if m.contains("no earlier version"))
    );
    assert_eq!(fx.of_kind(integrity::KIND_MISSING_ORIGINAL).await.len(), 1);
    // A kind it does not apply to.
    let (stray, _) = fx.put_original(fx.user, "stray.jpg", b"lima twelve").await;
    fx.check(&fx.opts()).await;
    let s = fx
        .of_kind(integrity::KIND_UNTRACKED)
        .await
        .into_iter()
        .find(|f| f.key == stray)
        .unwrap();
    assert!(matches!(
        fx.act(s.id, Action::RestoreVersion).await,
        Err(ActError::Conflict(_))
    ));
    assert!(matches!(
        fx.act(999_999, Action::Acknowledge).await,
        Err(ActError::NotFound)
    ));

    // The next run finds nothing about the two repaired photos.
    let after = fx.check(&fx.opts()).await;
    assert_eq!(after.open[integrity::KIND_CHECKSUM], 0);
    fx.teardown().await;
}

#[tokio::test]
async fn a_missing_original_can_be_moved_to_the_trash_or_marked_lost_and_stays_quiet() {
    let Some(fx) = Fx::new().await else { return };
    let (k1, s1) = fx.put_original(fx.user, "one.jpg", b"bytes thirteen").await;
    let a1 = fx.row(fx.user, &k1, &s1, 13, Some("done")).await;
    let (k2, s2) = fx.put_original(fx.user, "two.jpg", b"november fourteen").await;
    let a2 = fx.row(fx.user, &k2, &s2, 17, Some("done")).await;
    for a in [a1, a2] {
        fx.derived(fx.user, a).await;
    }
    fx.storage.delete_all_versions(&[k1.clone(), k2.clone()]).await.unwrap();
    fx.check(&fx.opts()).await;
    let open = fx.of_kind(integrity::KIND_MISSING_ORIGINAL).await;
    let (f1, f2) = (
        open.iter().find(|f| f.key == k1).unwrap().id,
        open.iter().find(|f| f.key == k2).unwrap().id,
    );

    assert!(fx.act(f1, Action::Trash).await.unwrap().changed);
    assert!(!fx.act(f1, Action::Trash).await.unwrap().changed, "idempotent");
    let trashed: bool = sqlx::query_scalar("SELECT deleted_at IS NOT NULL FROM assets WHERE id = $1")
        .bind(a1)
        .fetch_one(&fx.pool)
        .await
        .unwrap();
    assert!(trashed);

    assert!(fx.act(f2, Action::Acknowledge).await.unwrap().changed);
    assert!(!fx.act(f2, Action::Acknowledge).await.unwrap().changed);
    // A later run does not raise either again, and the acknowledged one is not counted as open.
    let s = fx.check(&fx.opts()).await;
    assert_eq!(s.open.values().sum::<i64>(), 0, "{s:?}");
    let acknowledged = fx.findings("acknowledged").await;
    assert_eq!((acknowledged.len(), acknowledged[0].key.as_str()), (1, k2.as_str()));
    assert_eq!(fx.findings("open").await.len(), 0);
    fx.teardown().await;
}

#[tokio::test]
async fn an_orphan_can_be_restored_as_a_photo_or_deleted_after_confirmation() {
    let Some(fx) = Fx::new().await else { return };
    let name = "Hotel \"Zürich\" 1.JPG";
    let body = b"oscar fifteen";
    let sha: [u8; 32] = Sha256::digest(body).into();
    let key = keys::original(fx.user, &sha, "jpg");
    let meta = objmeta::encode(&objmeta::OriginalMeta {
        name: Some(name.into()),
        owner: Some(fx.user),
        created_at: None,
        file_modified_at: None,
    });
    let up = fx.storage.create_multipart(&key, "image/jpeg", meta).await.unwrap();
    let etag = fx
        .storage
        .upload_part(&key, &up, 1, body.to_vec().into())
        .await
        .unwrap();
    fx.storage.complete_multipart(&key, &up, &[(1, etag)]).await.unwrap();
    let (junk, _) = fx.put_original(fx.user, "junk.jpg", b"papa sixteen").await;
    let ghost = Uuid::new_v4();
    let (ghost_key, _) = fx.put_original(ghost, "ghost.jpg", b"quebec seventeen").await;

    fx.check(&fx.opts()).await;
    let open = fx.of_kind(integrity::KIND_UNTRACKED).await;
    let id = |k: &str| open.iter().find(|f| f.key == k).unwrap().id;
    assert_eq!(
        open.iter().find(|f| f.key == ghost_key).unwrap().detail["owner_known"],
        false
    );

    let r = fx.act(id(&key), Action::RestorePhoto).await.unwrap();
    assert!(r.changed);
    let asset: Uuid = r.detail["asset_id"].as_str().unwrap().parse().unwrap();
    let (n, o, sz): (String, Uuid, i64) =
        sqlx::query_as("SELECT original_name, owner_id, size_bytes FROM assets WHERE id = $1")
            .bind(asset)
            .fetch_one(&fx.pool)
            .await
            .unwrap();
    assert_eq!((n.as_str(), o, sz), (name, fx.user, 13));
    let jobs_n: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE asset_id = $1")
        .bind(asset)
        .fetch_one(&fx.pool)
        .await
        .unwrap();
    assert_eq!(jobs_n, 2, "metadata + thumbnail, like a reindex");
    assert!(
        !fx.act(id(&key), Action::RestorePhoto).await.unwrap().changed,
        "idempotent"
    );
    // An owner without an account cannot be given a photo.
    assert!(
        matches!(fx.act(id(&ghost_key), Action::RestorePhoto).await, Err(ActError::Conflict(m)) if m.contains("no account"))
    );

    // Deleting needs the confirmation, then removes every version of exactly that object.
    let refused = integrity::act(&fx.pool, &fx.storage, id(&junk), Action::DeleteObject, fx.admin, false).await;
    assert!(matches!(refused, Err(ActError::Conflict(_))));
    assert!(fx.storage.head_size(&junk).await.unwrap().is_some());
    let del = integrity::act(&fx.pool, &fx.storage, id(&junk), Action::DeleteObject, fx.admin, true)
        .await
        .unwrap();
    assert!(del.changed);
    assert!(fx.storage.head_size(&junk).await.unwrap().is_none());
    assert!(
        fx.storage.list_versions(&junk).await.unwrap().is_empty(),
        "every version, no delete marker left"
    );
    let again = integrity::act(&fx.pool, &fx.storage, id(&junk), Action::DeleteObject, fx.admin, true)
        .await
        .unwrap();
    assert!(!again.changed);
    // The restored photo's own object is untouched, and the next run is quiet about it.
    assert!(fx.storage.head_size(&key).await.unwrap().is_some());
    let s = fx.check(&fx.opts()).await;
    assert_eq!(s.open[integrity::KIND_UNTRACKED], 1, "only the ghost's object remains");
    // Never delete something a photo now uses (a finding that went stale between a run and the click).
    let (k, sh) = fx.put_original(fx.user, "late.jpg", b"romeo eighteen").await;
    fx.check(&fx.opts()).await;
    let late = fx
        .of_kind(integrity::KIND_UNTRACKED)
        .await
        .into_iter()
        .find(|f| f.key == k)
        .unwrap()
        .id;
    fx.row(fx.user, &k, &sh, 14, Some("done")).await;
    let r = integrity::act(&fx.pool, &fx.storage, late, Action::DeleteObject, fx.admin, true).await;
    assert!(matches!(r, Err(ActError::Conflict(m)) if m.contains("now uses")));
    assert!(fx.storage.head_size(&k).await.unwrap().is_some());
    fx.teardown().await;
}

#[tokio::test]
async fn the_nightly_run_is_queued_once_per_night_and_a_stopped_run_is_closed() {
    let Some(fx) = Fx::new().await else { return };
    assert!(integrity::schedule_if_due(&fx.pool, 3).await.unwrap(), "never ran: due");
    assert!(
        !integrity::schedule_if_due(&fx.pool, 3).await.unwrap(),
        "already queued"
    );
    let task = admin_tasks::claim(&fx.pool, &[admin_tasks::KIND_INTEGRITY_CHECK])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(task.requested_by, None);
    assert_eq!(task.params["trigger"], "schedule");
    let run = integrity::begin(&fx.pool, "schedule", None).await.unwrap();
    // The worker dies; the task is claimed again later and starts over: the old record is closed as failed.
    let run2 = integrity::begin(&fx.pool, "schedule", None).await.unwrap();
    let (state, error): (String, Option<String>) =
        sqlx::query_as("SELECT state, error FROM integrity_runs WHERE id = $1")
            .bind(run)
            .fetch_one(&fx.pool)
            .await
            .unwrap();
    assert_eq!(state, "failed");
    assert!(error.unwrap().contains("stopped"));
    integrity::run(&fx.pool, &fx.storage, run2, &fx.opts()).await.unwrap();
    admin_tasks::finish(&fx.pool, task.id, Ok(serde_json::json!({})))
        .await
        .unwrap();
    assert!(
        !integrity::schedule_if_due(&fx.pool, 3).await.unwrap(),
        "tonight's run has happened"
    );
    // Last night's run no longer counts once the hour has come round again.
    sqlx::query("UPDATE integrity_runs SET started_at = now() - interval '30 hours'")
        .execute(&fx.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE admin_tasks SET created_at = now() - interval '30 hours'")
        .execute(&fx.pool)
        .await
        .unwrap();
    assert!(integrity::schedule_if_due(&fx.pool, 3).await.unwrap());
    fx.teardown().await;
}
