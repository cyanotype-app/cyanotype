//! The offline reverse-geocoding dataset bundled into this binary (`data/places.tsv.gz`, built by
//! `scripts/build-places-dataset.sh` from GeoNames, CC BY 4.0) and its loading into Postgres by `cyanotype migrate`.
use std::io::BufReader;

use anyhow::{Context, Result};
use cyanotype_core::places::{self, Loaded};
use flate2::read::GzDecoder;
use sqlx::PgPool;

static DATASET: &[u8] = include_bytes!("../data/places.tsv.gz");

/// Load the bundled snapshot unless that version is already in the database. Runs on every deploy through the
/// one-shot `migrate` step, so it is a version check and nothing else after the first time.
pub async fn load_bundled(pool: &PgPool) -> Result<()> {
    match load(pool, DATASET).await? {
        Loaded::UpToDate { version } => tracing::info!(version, "places dataset is up to date"),
        Loaded::Loaded {
            version,
            places,
            requeued,
        } => tracing::info!(version, places, requeued, "loaded the places dataset"),
    }
    Ok(())
}

async fn load(pool: &PgPool, gz: &[u8]) -> Result<Loaded> {
    places::load_dataset(pool, BufReader::new(GzDecoder::new(gz)))
        .await
        .context("load the places dataset")
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyanotype_core::places::{geocode_asset, nearest};
    use sqlx::{Connection, Executor, PgConnection};
    use uuid::Uuid;

    /// A fresh migrated database, or None without `DATABASE_URL`.
    async fn test_db() -> Option<(PgPool, String, String)> {
        let admin_url = std::env::var("DATABASE_URL").ok().filter(|v| !v.is_empty())?;
        let name = format!("cyanotype_test_{}", Uuid::new_v4().simple());
        let mut admin = PgConnection::connect(&admin_url).await.unwrap();
        admin.execute(format!("CREATE DATABASE {name}").as_str()).await.unwrap();
        let (base, _) = admin_url.rsplit_once('/').unwrap();
        let pool = cyanotype_core::db::connect(&format!("{base}/{name}")).await.unwrap();
        cyanotype_core::db::migrate(&pool).await.unwrap();
        Some((pool, admin_url, name))
    }

    async fn drop_db(pool: PgPool, admin_url: String, name: String) {
        pool.close().await;
        let mut admin = PgConnection::connect(&admin_url).await.unwrap();
        admin
            .execute(format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)").as_str())
            .await
            .unwrap();
    }

    async fn city(pool: &PgPool, lat: f64, lon: f64) -> Option<(String, Option<String>, String)> {
        nearest(pool, lat, lon)
            .await
            .unwrap()
            .map(|p| (p.city, p.region, p.country))
    }

    #[tokio::test]
    async fn bundled_dataset_geocodes_real_places() {
        let Some((pool, admin, name)) = test_db().await else {
            return;
        };
        let started = std::time::Instant::now();
        let first = load(&pool, DATASET).await.unwrap();
        eprintln!("dataset load took {:?}", started.elapsed());
        let Loaded::Loaded { places, .. } = first else {
            panic!("first load must load")
        };
        assert!(places > 100_000, "{places} places");
        // The same snapshot again changes nothing and queues nothing.
        assert!(matches!(load(&pool, DATASET).await.unwrap(), Loaded::UpToDate { .. }));

        let (c, r, k) = city(&pool, 48.8566, 2.3522).await.unwrap();
        assert!(c.starts_with("Paris"), "{c}"); // GeoNames lists the arrondissements as places
        assert_eq!(k, "France");
        assert!(r.as_deref().is_some_and(|r| r.ends_with("le-de-France")), "{r:?}");
        let (c, r, k) = city(&pool, 37.7749, -122.4194).await.unwrap();
        assert_eq!(
            (c.as_str(), r.as_deref(), k.as_str()),
            ("San Francisco", Some("California"), "United States")
        );
        // Either side of the French/Swiss border at Basel: the nearest place decides, and it is a different country.
        let (_, _, swiss) = city(&pool, 47.5596, 7.5886).await.unwrap();
        let (_, _, french) = city(&pool, 47.5900, 7.5600).await.unwrap(); // Saint-Louis side
        assert_eq!(swiss, "Switzerland");
        assert_eq!(french, "France");
        // Across the antimeridian: 37 km east of the date line, the nearest place is Beringovskiy (Russia, 179.35 E).
        let (c, _, k) = city(&pool, 63.06, -179.9).await.unwrap();
        assert_eq!((c.as_str(), k.as_str()), ("Beringovskiy", "Russia"));
        // Open sea, and the geographic poles: no place.
        assert!(city(&pool, 0.0, -30.0).await.is_none());
        assert!(city(&pool, -90.0, 0.0).await.is_none());
        assert!(city(&pool, 90.0, 123.0).await.is_none());
        drop_db(pool, admin, name).await;
    }

    #[tokio::test]
    async fn loading_a_new_version_requeues_geocode_jobs() {
        let Some((pool, admin, name)) = test_db().await else {
            return;
        };
        let (uid,): (Uuid,) =
            sqlx::query_as("INSERT INTO users (email, password_hash) VALUES ('a@b.c','x') RETURNING id")
                .fetch_one(&pool)
                .await
                .unwrap();
        let asset: Uuid = sqlx::query_scalar(
            "INSERT INTO assets (owner_id, sha256, original_key, original_name, content_type, size_bytes, latitude, longitude)
             VALUES ($1, sha256('x'::bytea), 'k', 'x.jpg', 'image/jpeg', 1, 48.8566, 2.3522) RETURNING id",
        )
        .bind(uid)
        .fetch_one(&pool)
        .await
        .unwrap();
        load(&pool, DATASET).await.unwrap();
        // The migration's backfill plus the load's: still exactly one job.
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind = 'geocode' AND asset_id = $1")
            .bind(asset)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 1);
        geocode_asset(&pool, asset).await.unwrap();
        sqlx::query("UPDATE jobs SET state = 'done', finished_at = now() WHERE kind = 'geocode'")
            .execute(&pool)
            .await
            .unwrap();
        // Same version: still done.
        load(&pool, DATASET).await.unwrap();
        let state: String = sqlx::query_scalar("SELECT state FROM jobs WHERE kind = 'geocode'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(state, "done");
        // A different version replaces the data and queues the job again.
        sqlx::query("UPDATE place_dataset SET version = 'older'")
            .execute(&pool)
            .await
            .unwrap();
        assert!(matches!(
            load(&pool, DATASET).await.unwrap(),
            Loaded::Loaded { requeued: 1, .. }
        ));
        let state: String = sqlx::query_scalar("SELECT state FROM jobs WHERE kind = 'geocode'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(state, "queued");
        drop_db(pool, admin, name).await;
    }
}
