//! Named pets over a real Postgres with pgvector: what a new scan of a photo keeps of the owner's decisions, the fingerprints the sweep fills
//! in, and the proposals ("Is this Biscuit?") with everything they leave out. Set `DATABASE_URL` (see `tests/jobs.rs`); without it the
//! tests return early. The routes are tested in `apps/assets/src/routes/named_pets_tests.rs`, the job in `apps/worker/src/pets.rs`.
use cyanotype_core::{
    db, jobs,
    named_pets::{self, LIKELY, MAYBE, Tier},
    pets::{self, Basis, EMBED_DIM, MODEL, NewDetection, SAME_CROP, Scan, VERSION},
};
use sqlx::{Connection, Executor, PgConnection, PgPool};
use uuid::Uuid;

const SEARCH: Basis<'static> = Basis {
    model: "siglip2-base-patch16-224-q4",
    version: 2,
};

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

async fn user(pool: &PgPool) -> Uuid {
    sqlx::query_scalar("INSERT INTO users (email, password_hash) VALUES ($1, 'x') RETURNING id")
        .bind(format!("{}@example.com", Uuid::new_v4()))
        .fetch_one(pool)
        .await
        .unwrap()
}

/// A live photo with a preview.
async fn asset(pool: &PgPool, owner: Uuid) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO assets (owner_id, sha256, original_key, original_name, content_type, size_bytes, thumbhash)
         VALUES ($1, sha256($2::bytea), 'k', 'p.jpg', 'image/jpeg', 1, '\\x01') RETURNING id",
    )
    .bind(owner)
    .bind(Uuid::new_v4().as_bytes().to_vec())
    .fetch_one(pool)
    .await
    .unwrap()
}

/// A unit vector that has cosine `sim` with `axis(0)` (and is `axis(k)` away otherwise): `near(1.0, _)` is `axis(0)` itself.
fn axis(i: usize) -> Vec<f32> {
    let mut v = vec![0.0; EMBED_DIM];
    v[i] = 1.0;
    v
}

fn near(sim: f32, k: usize) -> Vec<f32> {
    let mut v = axis(k);
    v.iter_mut().for_each(|x| *x *= (1.0 - sim * sim).max(0.0).sqrt());
    v[0] = sim;
    v
}

fn detection(kind: &str, bbox: [f32; 4], embedding: Option<Vec<f32>>) -> NewDetection {
    NewDetection {
        kind: kind.into(),
        bbox,
        score: 0.9,
        embedding,
    }
}

fn scan(found: Vec<NewDetection>) -> Scan {
    Scan {
        dog_share: 0.9,
        cat_share: 0.9,
        fingerprinted: true,
        found,
    }
}

async fn store(pool: &PgPool, asset: Uuid, found: Vec<NewDetection>) {
    assert!(pets::store(pool, asset, MODEL, VERSION, &scan(found)).await.unwrap());
}

/// A photo with one animal found in it, fingerprinted with `embedding`; returns the photo and the animal.
async fn seen(pool: &PgPool, owner: Uuid, kind: &str, embedding: Vec<f32>) -> (Uuid, Uuid) {
    let a = asset(pool, owner).await;
    store(pool, a, vec![detection(kind, [0.2, 0.2, 0.5, 0.5], Some(embedding))]).await;
    (a, animal(pool, a, 0).await)
}

async fn animal(pool: &PgPool, asset: Uuid, idx: i16) -> Uuid {
    sqlx::query_scalar("SELECT id FROM pet_detections WHERE asset_id = $1 AND idx = $2")
        .bind(asset)
        .bind(idx)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn pet(pool: &PgPool, owner: Uuid, name: &str, kind: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO pets (owner_id, name, kind) VALUES ($1, $2, $3) RETURNING id")
        .bind(owner)
        .bind(name)
        .bind(kind)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn put_in(pool: &PgPool, animal: Uuid, pet: Uuid) {
    sqlx::query("UPDATE pet_detections SET pet_id = $2 WHERE id = $1")
        .bind(animal)
        .bind(pet)
        .execute(pool)
        .await
        .unwrap();
}

async fn refuse(pool: &PgPool, animal: Uuid, pet: Uuid) {
    sqlx::query("INSERT INTO pet_rejections (detection_id, pet_id) VALUES ($1, $2)")
        .bind(animal)
        .bind(pet)
        .execute(pool)
        .await
        .unwrap();
}

async fn pet_of(pool: &PgPool, asset: Uuid, idx: i16) -> Option<Uuid> {
    sqlx::query_scalar("SELECT pet_id FROM pet_detections WHERE asset_id = $1 AND idx = $2")
        .bind(asset)
        .bind(idx)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn refused_for(pool: &PgPool, asset: Uuid, idx: i16) -> Vec<Uuid> {
    sqlx::query_scalar(
        "SELECT r.pet_id FROM pet_rejections r JOIN pet_detections d ON d.id = r.detection_id
         WHERE d.asset_id = $1 AND d.idx = $2 ORDER BY r.pet_id",
    )
    .bind(asset)
    .bind(idx)
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn count(pool: &PgPool, sql: &str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
}

#[tokio::test]
async fn a_new_scan_with_the_same_boxes_keeps_the_pet_the_refusals_and_the_cover() {
    let Some(t) = TestDb::new().await else { return };
    let owner = user(&t.pool).await;
    let (a, d) = seen(&t.pool, owner, "dog", axis(0)).await;
    let (biscuit, mochi) = (
        pet(&t.pool, owner, "Biscuit", "dog").await,
        pet(&t.pool, owner, "Mochi", "dog").await,
    );
    put_in(&t.pool, d, biscuit).await;
    refuse(&t.pool, d, mochi).await;
    sqlx::query("UPDATE pets SET cover_detection_id = $2 WHERE id = $1")
        .bind(biscuit)
        .bind(d)
        .execute(&t.pool)
        .await
        .unwrap();
    // Found again (a new version of the detector, say): no fingerprint this time, the same box.
    store(&t.pool, a, vec![detection("dog", [0.21, 0.2, 0.5, 0.5], None)]).await;
    let d2 = animal(&t.pool, a, 0).await;
    assert_ne!(d, d2, "a new scan makes new rows");
    assert_eq!(pet_of(&t.pool, a, 0).await, Some(biscuit));
    assert_eq!(refused_for(&t.pool, a, 0).await, vec![mochi]);
    let cover: Option<Uuid> = sqlx::query_scalar("SELECT cover_detection_id FROM pets WHERE id = $1")
        .bind(biscuit)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(cover, Some(d2), "the chosen cover follows the animal");
    t.teardown().await;
}

#[tokio::test]
async fn an_animal_that_moved_is_found_again_by_its_fingerprint_and_a_stranger_in_its_place_is_not() {
    let Some(t) = TestDb::new().await else { return };
    let owner = user(&t.pool).await;
    let (a, d) = seen(&t.pool, owner, "dog", axis(0)).await;
    let biscuit = pet(&t.pool, owner, "Biscuit", "dog").await;
    put_in(&t.pool, d, biscuit).await;
    // A rotate: the box is elsewhere, the crop looks (nearly) the same.
    store(
        &t.pool,
        a,
        vec![detection("dog", [0.6, 0.1, 0.3, 0.3], Some(near(SAME_CROP + 0.05, 1)))],
    )
    .await;
    assert_eq!(pet_of(&t.pool, a, 0).await, Some(biscuit));
    // Two animals now, none like the first and none in its place: nothing is guessed when there is a choice.
    store(
        &t.pool,
        a,
        vec![
            detection("dog", [0.0, 0.0, 0.2, 0.2], Some(near(SAME_CROP - 0.3, 2))),
            detection("dog", [0.7, 0.7, 0.2, 0.2], Some(near(SAME_CROP - 0.3, 3))),
        ],
    )
    .await;
    assert_eq!(pet_of(&t.pool, a, 0).await, None, "nothing carried to a stranger");
    assert_eq!(pet_of(&t.pool, a, 1).await, None);
    t.teardown().await;
}

#[tokio::test]
async fn a_photos_only_animal_of_a_kind_is_the_same_animal_after_a_rotate_whatever_its_crop_says() {
    let Some(t) = TestDb::new().await else { return };
    let owner = user(&t.pool).await;
    let (a, d) = seen(&t.pool, owner, "dog", axis(0)).await;
    let biscuit = pet(&t.pool, owner, "Biscuit", "dog").await;
    put_in(&t.pool, d, biscuit).await;
    // A quarter turn: the box is elsewhere and the model's crop of the turned dog is nothing like the old one.
    store(
        &t.pool,
        a,
        vec![detection("dog", [0.6, 0.1, 0.3, 0.3], Some(near(0.2, 4)))],
    )
    .await;
    assert_eq!(pet_of(&t.pool, a, 0).await, Some(biscuit));
    // But not across kinds.
    store(
        &t.pool,
        a,
        vec![detection("cat", [0.6, 0.1, 0.3, 0.3], Some(near(0.2, 4)))],
    )
    .await;
    assert_eq!(pet_of(&t.pool, a, 0).await, None);
    t.teardown().await;
}

#[tokio::test]
async fn animals_are_matched_one_to_one_within_their_kind() {
    let Some(t) = TestDb::new().await else { return };
    let owner = user(&t.pool).await;
    let a = asset(&t.pool, owner).await;
    store(
        &t.pool,
        a,
        vec![
            detection("dog", [0.0, 0.0, 0.4, 0.4], Some(axis(0))),
            detection("dog", [0.5, 0.5, 0.4, 0.4], Some(axis(1))),
            detection("cat", [0.0, 0.5, 0.4, 0.4], Some(axis(2))),
        ],
    )
    .await;
    let (first, second, cat) = (
        pet(&t.pool, owner, "Biscuit", "dog").await,
        pet(&t.pool, owner, "Mochi", "dog").await,
        pet(&t.pool, owner, "Pepper", "cat").await,
    );
    for (idx, p) in [(0, first), (1, second), (2, cat)] {
        put_in(&t.pool, animal(&t.pool, a, idx).await, p).await;
    }
    // The next scan lists them in another order, one has moved, and the cat's box is where a dog was: kinds never mix.
    store(
        &t.pool,
        a,
        vec![
            detection("cat", [0.5, 0.5, 0.4, 0.4], Some(axis(2))),
            detection("dog", [0.5, 0.5, 0.4, 0.4], Some(axis(1))),
            detection("dog", [0.1, 0.1, 0.3, 0.3], Some(axis(0))),
        ],
    )
    .await;
    assert_eq!(
        [
            pet_of(&t.pool, a, 0).await,
            pet_of(&t.pool, a, 1).await,
            pet_of(&t.pool, a, 2).await
        ],
        [Some(cat), Some(second), Some(first)]
    );
    t.teardown().await;
}

#[tokio::test]
async fn fingerprints_of_another_model_are_not_compared_only_boxes() {
    let Some(t) = TestDb::new().await else { return };
    let owner = user(&t.pool).await;
    let (a, d) = seen(&t.pool, owner, "dog", axis(0)).await;
    let biscuit = pet(&t.pool, owner, "Biscuit", "dog").await;
    put_in(&t.pool, d, biscuit).await;
    // The stored fingerprint was made by a model that is not the current one: numbers of two models mean nothing next to each other.
    sqlx::query("UPDATE pet_detections SET embed_model = 'older-model' WHERE id = $1")
        .bind(d)
        .execute(&t.pool)
        .await
        .unwrap();
    store(
        &t.pool,
        a,
        vec![
            detection("dog", [0.8, 0.8, 0.1, 0.1], Some(axis(0))),
            detection("dog", [0.0, 0.8, 0.1, 0.1], Some(axis(1))),
        ],
    )
    .await;
    assert_eq!(
        pet_of(&t.pool, a, 0).await,
        None,
        "same numbers, other model, other place: not the same animal"
    );
    t.teardown().await;
}

#[tokio::test]
async fn a_scan_that_finds_nothing_leaves_the_pet_and_drops_the_animal_with_its_refusals() {
    let Some(t) = TestDb::new().await else { return };
    let owner = user(&t.pool).await;
    let (a, d) = seen(&t.pool, owner, "dog", axis(0)).await;
    let (biscuit, mochi) = (
        pet(&t.pool, owner, "Biscuit", "dog").await,
        pet(&t.pool, owner, "Mochi", "dog").await,
    );
    put_in(&t.pool, d, biscuit).await;
    refuse(&t.pool, d, mochi).await;
    store(&t.pool, a, vec![]).await;
    assert_eq!(count(&t.pool, "SELECT count(*) FROM pet_detections").await, 0);
    assert_eq!(count(&t.pool, "SELECT count(*) FROM pet_rejections").await, 0);
    assert_eq!(
        count(&t.pool, "SELECT count(*) FROM pets").await,
        2,
        "the pets stay, named, without that photo"
    );
    t.teardown().await;
}

#[tokio::test]
async fn an_edit_forgets_the_animals_nobody_decided_about_and_keeps_the_rest_until_the_new_scan() {
    let Some(t) = TestDb::new().await else { return };
    let owner = user(&t.pool).await;
    let a = asset(&t.pool, owner).await;
    store(
        &t.pool,
        a,
        vec![
            detection("dog", [0.0, 0.0, 0.4, 0.4], Some(axis(0))),
            detection("dog", [0.5, 0.0, 0.4, 0.4], Some(axis(1))),
            detection("dog", [0.0, 0.5, 0.4, 0.4], Some(axis(2))),
            detection("dog", [0.5, 0.5, 0.4, 0.4], Some(axis(3))),
        ],
    )
    .await;
    let biscuit = pet(&t.pool, owner, "Biscuit", "dog").await;
    let mochi = pet(&t.pool, owner, "Mochi", "dog").await;
    put_in(&t.pool, animal(&t.pool, a, 0).await, biscuit).await;
    refuse(&t.pool, animal(&t.pool, a, 1).await, biscuit).await;
    sqlx::query("UPDATE pets SET cover_detection_id = $2 WHERE id = $1")
        .bind(mochi)
        .bind(animal(&t.pool, a, 2).await)
        .execute(&t.pool)
        .await
        .unwrap();
    assert!(pets::forget_and_requeue(&t.pool, a, VERSION, true).await.unwrap());
    let left: Vec<i16> = sqlx::query_scalar("SELECT idx FROM pet_detections WHERE asset_id = $1 ORDER BY idx")
        .bind(a)
        .fetch_all(&t.pool)
        .await
        .unwrap();
    assert_eq!(left, vec![0, 1, 2], "the one nobody decided about is gone");
    assert_eq!(
        count(&t.pool, "SELECT count(*) FROM pet_scans").await,
        0,
        "and the others wait unread: there is no scan"
    );
    // The new scan finds them rotated: the fingerprints still say who is who.
    store(
        &t.pool,
        a,
        vec![
            detection("dog", [0.1, 0.6, 0.3, 0.3], Some(near(SAME_CROP + 0.1, 5))),
            detection("dog", [0.6, 0.1, 0.3, 0.3], Some(axis(2))),
        ],
    )
    .await;
    assert_eq!(
        pet_of(&t.pool, a, 0).await,
        Some(biscuit),
        "the first looks like the animal that was in Biscuit"
    );
    assert_eq!(pet_of(&t.pool, a, 1).await, None);
    let cover: Option<Uuid> = sqlx::query_scalar("SELECT cover_detection_id FROM pets WHERE id = $1")
        .bind(mochi)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(
        cover,
        Some(animal(&t.pool, a, 1).await),
        "the second is the one that was Mochi's cover"
    );
    assert!(refused_for(&t.pool, a, 0).await.is_empty() && refused_for(&t.pool, a, 1).await.is_empty());
    t.teardown().await;
}

#[tokio::test]
async fn the_sweep_fills_in_fingerprints_without_finding_the_animals_again() {
    let Some(t) = TestDb::new().await else { return };
    let owner = user(&t.pool).await;
    // A photo scanned before named pets: animals, no fingerprint, and the search vector the job reads.
    let a = asset(&t.pool, owner).await;
    sqlx::query(
        "INSERT INTO asset_embeddings (asset_id, model, version, embedding)
         VALUES ($1, $2, $3, array_fill(0.1::real, ARRAY[768])::vector::halfvec)",
    )
    .bind(a)
    .bind(SEARCH.model)
    .bind(SEARCH.version)
    .execute(&t.pool)
    .await
    .unwrap();
    pets::store(
        &t.pool,
        a,
        MODEL,
        VERSION,
        &Scan {
            fingerprinted: false,
            ..scan(vec![
                detection("dog", [0.0, 0.0, 0.4, 0.4], None),
                detection("cat", [0.5, 0.5, 0.4, 0.4], None),
            ])
        },
    )
    .await
    .unwrap();
    let ids: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM pet_detections WHERE asset_id = $1 ORDER BY idx")
        .bind(a)
        .fetch_all(&t.pool)
        .await
        .unwrap();
    // The photo is scanned, but its animals have no fingerprint: the sweep queues it.
    assert_eq!(pets::enqueue_missing(&t.pool, MODEL, VERSION, SEARCH).await.unwrap(), 1);
    let todo = pets::unfingerprinted(&t.pool, a, MODEL).await.unwrap();
    assert_eq!(
        todo.iter().map(|u| (u.id, u.kind.as_str())).collect::<Vec<_>>(),
        vec![(ids[0], "dog"), (ids[1], "cat")]
    );
    assert_eq!(todo[0].bbox, [0.0, 0.0, 0.4, 0.4]);
    // One gets a fingerprint, the other is too small to have one: both are done, and the photo is not queued again.
    let job = jobs::claim(&t.pool, jobs::KIND_PETS).await.unwrap().unwrap();
    assert_eq!(job.asset_id, a);
    assert_eq!(
        pets::set_fingerprints(&t.pool, &[(ids[0], Some(axis(0))), (ids[1], None)])
            .await
            .unwrap(),
        2
    );
    jobs::complete(&t.pool, job.id).await.unwrap();
    assert!(pets::unfingerprinted(&t.pool, a, MODEL).await.unwrap().is_empty());
    assert_eq!(pets::enqueue_missing(&t.pool, MODEL, VERSION, SEARCH).await.unwrap(), 0);
    // The animals are the same rows: nothing was found again.
    let after: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM pet_detections WHERE asset_id = $1 ORDER BY idx")
        .bind(a)
        .fetch_all(&t.pool)
        .await
        .unwrap();
    assert_eq!(after, ids);
    let have: Vec<(bool, Option<String>)> = sqlx::query_as(
        "SELECT embedding IS NOT NULL, embed_model FROM pet_detections WHERE asset_id = $1 ORDER BY idx",
    )
    .bind(a)
    .fetch_all(&t.pool)
    .await
    .unwrap();
    assert_eq!(
        have,
        vec![
            (true, Some(pets::EMBED_MODEL.into())),
            (false, Some(pets::EMBED_MODEL.into()))
        ]
    );
    // A photo that is gone meanwhile is simply not updated.
    assert_eq!(
        pets::set_fingerprints(&t.pool, &[(Uuid::new_v4(), Some(axis(0)))])
            .await
            .unwrap(),
        0
    );
    t.teardown().await;
}

/// Alpha's pet Biscuit with `k` seeds exactly on axis 0, and candidates at the cosines given (one photo each).
async fn proposals_setup(pool: &PgPool, sims: &[f32]) -> (Uuid, Uuid, Vec<Uuid>) {
    let owner = user(pool).await;
    let biscuit = pet(pool, owner, "Biscuit", "dog").await;
    for _ in 0..3 {
        let (_, d) = seen(pool, owner, "dog", axis(0)).await;
        put_in(pool, d, biscuit).await;
    }
    let mut cands = Vec::new();
    for (i, s) in sims.iter().enumerate() {
        let (_, d) = seen(pool, owner, "dog", near(*s, 1 + i)).await;
        cands.push(d);
    }
    (owner, biscuit, cands)
}

fn ids(s: &[named_pets::Suggestion]) -> Vec<Uuid> {
    s.iter().map(|s| s.detection_id).collect()
}

#[tokio::test]
async fn proposals_are_ranked_by_the_centre_and_cut_into_tiers() {
    let Some(t) = TestDb::new().await else { return };
    let (likely, maybe, below) = (LIKELY + 0.05, (LIKELY + MAYBE) / 2.0, MAYBE - 0.05);
    let (owner, biscuit, cands) = proposals_setup(&t.pool, &[maybe, below, likely]).await;
    let got = named_pets::suggestions(&t.pool, owner, biscuit, "dog", false, 50)
        .await
        .unwrap();
    assert_eq!(
        ids(&got),
        vec![cands[2], cands[0]],
        "best first; below the lowest tier nothing is proposed"
    );
    assert_eq!((got[0].tier, got[1].tier), (Tier::Likely, Tier::Maybe));
    assert!((got[0].similarity - likely).abs() < 2e-3, "{}", got[0].similarity);
    let limited = named_pets::suggestions(&t.pool, owner, biscuit, "dog", false, 1)
        .await
        .unwrap();
    assert_eq!(ids(&limited), vec![cands[2]]);
    // A pet with nothing fingerprinted has nothing to compare with.
    let empty = pet(&t.pool, owner, "Mochi", "dog").await;
    assert!(
        named_pets::suggestions(&t.pool, owner, empty, "dog", false, 50)
            .await
            .unwrap()
            .is_empty()
    );
    t.teardown().await;
}

#[tokio::test]
async fn every_answer_changes_the_next_proposals() {
    let Some(t) = TestDb::new().await else { return };
    let (owner, biscuit, cands) = proposals_setup(&t.pool, &[LIKELY + 0.1, LIKELY + 0.05, MAYBE + 0.02]).await;
    assert_eq!(
        ids(&named_pets::suggestions(&t.pool, owner, biscuit, "dog", false, 50)
            .await
            .unwrap()),
        cands
    );
    // Confirmed: it leaves the list and joins the centre. Refused: it never comes back for this pet.
    put_in(&t.pool, cands[0], biscuit).await;
    refuse(&t.pool, cands[1], biscuit).await;
    let after = named_pets::suggestions(&t.pool, owner, biscuit, "dog", false, 50)
        .await
        .unwrap();
    assert!(!ids(&after).contains(&cands[0]) && !ids(&after).contains(&cands[1]));
    // The refusal is for this pet only.
    let mochi = pet(&t.pool, owner, "Mochi", "dog").await;
    for _ in 0..3 {
        let (_, d) = seen(&t.pool, owner, "dog", near(LIKELY + 0.05, 1 + 1)).await;
        put_in(&t.pool, d, mochi).await;
    }
    let mochis = named_pets::suggestions(&t.pool, owner, mochi, "dog", false, 50)
        .await
        .unwrap();
    assert!(ids(&mochis).contains(&cands[1]), "refused for Biscuit, not for Mochi");
    t.teardown().await;
}

#[tokio::test]
async fn proposals_leave_out_what_does_not_belong() {
    let Some(t) = TestDb::new().await else { return };
    let good = LIKELY + 0.05;
    let (owner, biscuit, cands) = proposals_setup(&t.pool, &[good, good, good, good, good, good, good]).await;
    let other = user(&t.pool).await;
    // 1: another pet's animal.
    let mochi = pet(&t.pool, owner, "Mochi", "dog").await;
    put_in(&t.pool, cands[1], mochi).await;
    // 2: in the trash.
    let asset_of = |d: Uuid| {
        let pool = t.pool.clone();
        async move {
            sqlx::query_scalar::<_, Uuid>("SELECT asset_id FROM pet_detections WHERE id = $1")
                .bind(d)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    sqlx::query("UPDATE assets SET deleted_at = now() WHERE id = $1")
        .bind(asset_of(cands[2]).await)
        .execute(&t.pool)
        .await
        .unwrap();
    // 3: in the Hidden view.
    sqlx::query("UPDATE assets SET hidden = true WHERE id = $1")
        .bind(asset_of(cands[3]).await)
        .execute(&t.pool)
        .await
        .unwrap();
    // 4: its photo is being scanned again (no scan row).
    sqlx::query("DELETE FROM pet_scans WHERE asset_id = $1")
        .bind(asset_of(cands[4]).await)
        .execute(&t.pool)
        .await
        .unwrap();
    // 5: a cat that looks the same.
    let (_, cat) = seen(&t.pool, owner, "cat", near(good, 20)).await;
    // 6: another model's fingerprint.
    sqlx::query("UPDATE pet_detections SET embed_model = 'older-model' WHERE id = $1")
        .bind(cands[5])
        .execute(&t.pool)
        .await
        .unwrap();
    // 7: somebody else's dog that looks the same.
    let (_, strangers) = seen(&t.pool, other, "dog", near(good, 21)).await;
    let got = named_pets::suggestions(&t.pool, owner, biscuit, "dog", false, 50)
        .await
        .unwrap();
    let (mut have, mut want) = (ids(&got), vec![cands[0], cands[6]]);
    have.sort();
    want.sort();
    assert_eq!(have, want, "the two that are left");
    for gone in [cands[1], cands[2], cands[3], cands[4], cands[5], cat, strangers] {
        assert!(!ids(&got).contains(&gone), "{gone}");
    }
    // Unlocked, the Hidden view's photo is in.
    let unlocked = named_pets::suggestions(&t.pool, owner, biscuit, "dog", true, 50)
        .await
        .unwrap();
    assert!(ids(&unlocked).contains(&cands[3]));
    // Nobody else sees Biscuit's proposals: asked as the other owner, nothing.
    assert!(
        named_pets::suggestions(&t.pool, other, biscuit, "dog", false, 50)
            .await
            .unwrap()
            .is_empty()
    );
    t.teardown().await;
}

#[tokio::test]
async fn pending_counts_are_one_query_for_every_shown_pet_and_agree_with_the_proposals() {
    let Some(t) = TestDb::new().await else { return };
    let good = LIKELY + 0.05;
    let (owner, biscuit, _) = proposals_setup(&t.pool, &[good, good, MAYBE + 0.02, MAYBE - 0.1]).await;
    // A second pet, a cat with its own proposals; a hidden pet and a pet with nothing fingerprinted.
    let pepper = pet(&t.pool, owner, "Pepper", "cat").await;
    for _ in 0..2 {
        let (_, d) = seen(&t.pool, owner, "cat", axis(0)).await;
        put_in(&t.pool, d, pepper).await;
    }
    seen(&t.pool, owner, "cat", near(good, 31)).await;
    let hidden = pet(&t.pool, owner, "Shadow", "dog").await;
    let (_, d) = seen(&t.pool, owner, "dog", axis(0)).await;
    put_in(&t.pool, d, hidden).await;
    sqlx::query("UPDATE pets SET hidden = true WHERE id = $1")
        .bind(hidden)
        .execute(&t.pool)
        .await
        .unwrap();
    let blank = pet(&t.pool, owner, "Blank", "dog").await;
    let counts = named_pets::pending_counts(&t.pool, owner, None, false).await.unwrap();
    let proposals = named_pets::suggestions(&t.pool, owner, biscuit, "dog", false, 300)
        .await
        .unwrap();
    assert_eq!(counts.get(&biscuit), Some(&(proposals.len() as i64)));
    assert_eq!(proposals.len(), 3);
    assert_eq!(counts.get(&pepper), Some(&1));
    assert!(!counts.contains_key(&hidden) && !counts.contains_key(&blank));
    // One pet alone, and somebody else's pets are not in it.
    let one = named_pets::pending_counts(&t.pool, owner, Some(pepper), false)
        .await
        .unwrap();
    assert_eq!(one.len(), 1);
    let other = user(&t.pool).await;
    assert!(
        named_pets::pending_counts(&t.pool, other, None, false)
            .await
            .unwrap()
            .is_empty()
    );
    t.teardown().await;
}

#[tokio::test]
async fn deleting_a_pet_frees_its_animals_and_everything_follows_the_photo_and_the_account() {
    let Some(t) = TestDb::new().await else { return };
    let owner = user(&t.pool).await;
    let (a, d) = seen(&t.pool, owner, "dog", axis(0)).await;
    let (b, e) = seen(&t.pool, owner, "dog", axis(1)).await;
    let (biscuit, mochi) = (
        pet(&t.pool, owner, "Biscuit", "dog").await,
        pet(&t.pool, owner, "Mochi", "dog").await,
    );
    put_in(&t.pool, d, biscuit).await;
    put_in(&t.pool, e, biscuit).await;
    refuse(&t.pool, d, mochi).await;
    sqlx::query("DELETE FROM pets WHERE id = $1")
        .bind(biscuit)
        .execute(&t.pool)
        .await
        .unwrap();
    assert_eq!(pet_of(&t.pool, a, 0).await, None, "freed, not deleted");
    assert_eq!(count(&t.pool, "SELECT count(*) FROM pet_detections").await, 2);
    // Purging a photo takes its animal and its refusals; the pet it was refused for stays.
    refuse(&t.pool, e, mochi).await;
    sqlx::query("DELETE FROM assets WHERE id = $1")
        .bind(b)
        .execute(&t.pool)
        .await
        .unwrap();
    assert_eq!(
        count(&t.pool, "SELECT count(*) FROM pet_rejections").await,
        1,
        "only the first photo's refusal is left"
    );
    // Deleting an account takes its pets.
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(owner)
        .execute(&t.pool)
        .await
        .unwrap();
    assert_eq!(count(&t.pool, "SELECT count(*) FROM pets").await, 0);
    assert_eq!(count(&t.pool, "SELECT count(*) FROM pet_rejections").await, 0);
    t.teardown().await;
}

#[tokio::test]
async fn delete_all_counts_the_named_pets_and_decisions_and_removes_them_for_one_owner() {
    let Some(t) = TestDb::new().await else { return };
    let (alpha, bravo) = (user(&t.pool).await, user(&t.pool).await);
    let (_, d) = seen(&t.pool, alpha, "dog", axis(0)).await;
    let (_, e) = seen(&t.pool, bravo, "dog", axis(0)).await;
    let (pa, pb) = (
        pet(&t.pool, alpha, "Biscuit", "dog").await,
        pet(&t.pool, bravo, "Mochi", "dog").await,
    );
    put_in(&t.pool, d, pa).await;
    put_in(&t.pool, e, pb).await;
    refuse(&t.pool, d, pa).await;
    let dry = pets::delete_all(&t.pool, Some(alpha), true).await.unwrap();
    assert_eq!((dry.detections, dry.pets, dry.rejections), (1, 1, 1));
    assert_eq!(
        count(&t.pool, "SELECT count(*) FROM pets").await,
        2,
        "a dry run changes nothing"
    );
    let done = pets::delete_all(&t.pool, Some(alpha), false).await.unwrap();
    assert_eq!((done.detections, done.pets, done.rejections), (1, 1, 1));
    assert_eq!(count(&t.pool, "SELECT count(*) FROM pets").await, 1);
    let theirs: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM pets WHERE owner_id = $1")
        .bind(bravo)
        .fetch_all(&t.pool)
        .await
        .unwrap();
    assert_eq!(theirs, vec![pb], "the other owner's pet is untouched");
    t.teardown().await;
}
