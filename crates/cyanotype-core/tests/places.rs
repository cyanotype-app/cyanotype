//! Reverse geocoding and the geocode backfill against a real Postgres (skipped without `DATABASE_URL`). The real
//! GeoNames snapshot is exercised in the CLI crate, which bundles it; here a handful of hand-placed cities make the
//! distances, the borders and the empty ocean exact.
use cyanotype_core::{db, jobs, places};
use sqlx::{Connection, Executor, PgConnection, PgPool};
use uuid::Uuid;

struct TestDb {
    pool: PgPool,
    admin_url: String,
    name: String,
}

impl TestDb {
    async fn new() -> Option<Self> {
        let admin_url = std::env::var("DATABASE_URL").ok().filter(|v| !v.is_empty())?;
        let name = format!("cyanotype_test_{}", Uuid::new_v4().simple());
        let mut admin = PgConnection::connect(&admin_url).await.unwrap();
        admin.execute(format!("CREATE DATABASE {name}").as_str()).await.unwrap();
        let (base, _) = admin_url.rsplit_once('/').unwrap();
        let pool = db::connect(&format!("{base}/{name}")).await.unwrap();
        db::migrate(&pool).await.unwrap();
        Some(Self { pool, admin_url, name })
    }

    async fn teardown(self) {
        self.pool.close().await;
        let mut admin = PgConnection::connect(&self.admin_url).await.unwrap();
        admin
            .execute(format!("DROP DATABASE IF EXISTS {} WITH (FORCE)", self.name).as_str())
            .await
            .unwrap();
    }
}

const DATASET: &str = "#version\tfixture-1
C\tCH\tSwitzerland
C\tFR\tFrance
C\tUS\tUnited States
C\tFJ\tFiji
A\tCH\tBS\tBasel-City
A\tFR\t44\tGrand Est
A\tFJ\t03\tNorthern
A\tFJ\t01\tEastern
P\tBasel\t47.558\t7.573\tCH\tBS\t164488
P\tSaint-Louis\t47.590\t7.560\tFR\t44\t20000
P\tSmall Hamlet\t47.575\t7.580\tFR\t44\t1100
P\tTaveuni Town\t-16.800\t-179.900\tFJ\t03\t800
P\tLevuka\t-17.680\t179.840\tFJ\t01\t1500
P\tUtqiagvik\t71.290\t-156.789\tUS\tAK\t4900
P\tTromso\t69.649\t18.956\tNO\t\t77000
";

async fn load(pool: &PgPool) -> places::Loaded {
    places::load_dataset(pool, DATASET.as_bytes()).await.unwrap()
}

async fn nearest(pool: &PgPool, lat: f64, lon: f64) -> Option<(String, Option<String>, String)> {
    places::nearest(pool, lat, lon)
        .await
        .unwrap()
        .map(|p| (p.city, p.region, p.country))
}

async fn geotagged(pool: &PgPool, owner: Uuid, name: &str, lat: Option<f64>, lon: Option<f64>) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO assets (owner_id, sha256, original_key, original_name, content_type, size_bytes, latitude, longitude)
         VALUES ($1, sha256($2::bytea), $2, $2, 'image/jpeg', 1, $3, $4) RETURNING id",
    )
    .bind(owner)
    .bind(name)
    .bind(lat)
    .bind(lon)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn owner(pool: &PgPool) -> Uuid {
    sqlx::query_scalar("INSERT INTO users (email, password_hash) VALUES ($1, 'x') RETURNING id")
        .bind(format!("{}@example.com", Uuid::new_v4()))
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn nearest_place_within_range_borders_ocean_and_the_date_line() {
    let Some(t) = TestDb::new().await else { return };
    assert!(matches!(load(&t.pool).await, places::Loaded::Loaded { places: 7, .. }));
    // Right next to Basel; and across the border the nearer French place wins, in its own country.
    assert_eq!(nearest(&t.pool, 47.559, 7.574).await.unwrap().2, "Switzerland");
    let (city, region, country) = nearest(&t.pool, 47.589, 7.561).await.unwrap();
    assert_eq!(
        (city.as_str(), region.as_deref(), country.as_str()),
        ("Saint-Louis", Some("Grand Est"), "France")
    );
    // The nearest wins even when a bigger city is a little farther (a hamlet 1 km away beats Basel 2.5 km away).
    assert_eq!(nearest(&t.pool, 47.576, 7.581).await.unwrap().0, "Small Hamlet");
    // A region the dataset does not know stays NULL; the country code stands in for an unknown country name.
    let (_, region, country) = nearest(&t.pool, 69.65, 18.96).await.unwrap();
    assert_eq!((region, country.as_str()), (None, "NO"));
    // Within range: 40 km south still finds Basel; 60 km does not.
    assert_eq!(
        nearest(&t.pool, 47.558 - 40.0 / 111.19, 7.573).await.unwrap().0,
        "Basel"
    );
    assert!(nearest(&t.pool, 47.558 - 60.0 / 111.19, 7.573).await.is_none());
    // The ocean, and out-of-range input: no place.
    assert!(nearest(&t.pool, 0.0, -30.0).await.is_none());
    assert!(nearest(&t.pool, 91.0, 0.0).await.is_none());
    assert!(nearest(&t.pool, f64::NAN, 0.0).await.is_none());
    // Across the antimeridian: a photo at +179.95 is 5 km from Taveuni Town (-179.9) on the other side of the line.
    assert_eq!(nearest(&t.pool, -16.8, 179.95).await.unwrap().0, "Taveuni Town");
    assert_eq!(nearest(&t.pool, -17.7, -179.99).await.unwrap().0, "Levuka");
    // Near the pole every longitude is searched: 71.3 N at the other side of the world is not in range.
    assert_eq!(nearest(&t.pool, 71.3, -156.8).await.unwrap().0, "Utqiagvik");
    assert!(nearest(&t.pool, 89.99, 20.0).await.is_none());
    t.teardown().await;
}

#[tokio::test]
async fn geocode_job_body_stores_clears_and_refuses_without_a_dataset() {
    let Some(t) = TestDb::new().await else { return };
    let o = owner(&t.pool).await;
    let basel = geotagged(&t.pool, o, "basel.jpg", Some(47.559), Some(7.574)).await;
    let sea = geotagged(&t.pool, o, "sea.jpg", Some(0.0), Some(-30.0)).await;
    let none = geotagged(&t.pool, o, "none.jpg", None, None).await;
    // No dataset yet: the job must fail (and so retry) instead of recording "no place".
    assert!(places::geocode_asset(&t.pool, basel).await.is_err());
    assert_eq!(
        places::geocode_asset(&t.pool, none).await.unwrap(),
        places::Geocoded::NoCoordinates
    );
    load(&t.pool).await;

    assert_eq!(
        places::geocode_asset(&t.pool, basel).await.unwrap(),
        places::Geocoded::Placed
    );
    assert_eq!(
        places::geocode_asset(&t.pool, sea).await.unwrap(),
        places::Geocoded::NoPlace
    );
    assert_eq!(
        places::geocode_asset(&t.pool, Uuid::new_v4()).await.unwrap(),
        places::Geocoded::Gone
    );
    let place = |id| {
        let pool = t.pool.clone();
        async move {
            sqlx::query_as::<_, (Option<String>, Option<String>, Option<String>, Option<String>)>(
                "SELECT place_city, place_region, place_country, place_country_code FROM assets WHERE id = $1",
            )
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    assert_eq!(
        place(basel).await,
        (
            Some("Basel".into()),
            Some("Basel-City".into()),
            Some("Switzerland".into()),
            Some("CH".into())
        )
    );
    assert_eq!(place(sea).await, (None, None, None, None));
    // Idempotent, and a photo that lost its coordinates loses its place.
    assert_eq!(
        places::geocode_asset(&t.pool, basel).await.unwrap(),
        places::Geocoded::Placed
    );
    sqlx::query("UPDATE assets SET latitude = NULL, longitude = NULL WHERE id = $1")
        .bind(basel)
        .execute(&t.pool)
        .await
        .unwrap();
    assert_eq!(
        places::geocode_asset(&t.pool, basel).await.unwrap(),
        places::Geocoded::NoCoordinates
    );
    assert_eq!(place(basel).await, (None, None, None, None));
    t.teardown().await;
}

#[tokio::test]
async fn backfill_is_idempotent_and_a_new_dataset_version_requeues() {
    let Some(t) = TestDb::new().await else { return };
    let o = owner(&t.pool).await;
    // Geotagged before the dataset existed: the migration's backfill is simulated by `requeue_all`.
    let a = geotagged(&t.pool, o, "a.jpg", Some(47.559), Some(7.574)).await;
    let b = geotagged(&t.pool, o, "b.jpg", Some(-16.8), Some(179.95)).await;
    geotagged(&t.pool, o, "plain.jpg", None, None).await;
    let count = || async {
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM jobs WHERE kind = 'geocode'")
            .fetch_one(&t.pool)
            .await
            .unwrap()
    };
    assert_eq!(places::requeue_all(&t.pool).await.unwrap(), 2);
    assert_eq!(
        places::requeue_all(&t.pool).await.unwrap(),
        0,
        "queued jobs are left alone"
    );
    assert_eq!(count().await, 2);

    // First load: same two jobs, nothing duplicated. Run them like the worker does.
    assert!(matches!(
        load(&t.pool).await,
        places::Loaded::Loaded { requeued: 0, .. }
    ));
    assert_eq!(count().await, 2);
    while let Some(job) = jobs::claim(&t.pool, jobs::KIND_GEOCODE).await.unwrap() {
        places::geocode_asset(&t.pool, job.asset_id).await.unwrap();
        jobs::complete(&t.pool, job.id).await.unwrap();
    }
    let city: Option<String> = sqlx::query_scalar("SELECT place_city FROM assets WHERE id = $1")
        .bind(b)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(city.as_deref(), Some("Taveuni Town"));

    // Same version: nothing happens, finished jobs stay finished.
    assert!(matches!(load(&t.pool).await, places::Loaded::UpToDate { .. }));
    places::requeue(&t.pool, a).await.unwrap();
    let open: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind = 'geocode' AND state <> 'done'")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(open, 1, "only the explicit requeue of `a`");
    // A new version replaces the data and queues every geotagged photo again.
    let newer = DATASET
        .replace("fixture-1", "fixture-2")
        .replace("Taveuni Town", "Taveuni");
    assert!(matches!(
        places::load_dataset(&t.pool, newer.as_bytes()).await.unwrap(),
        places::Loaded::Loaded {
            places: 7,
            requeued: 1,
            ..
        }
    ));
    assert_eq!(count().await, 2);
    let queued: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind = 'geocode' AND state = 'queued'")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(queued, 2);
    t.teardown().await;
}
