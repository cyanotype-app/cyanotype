//! People over a real Postgres with pgvector (set `DATABASE_URL`, see `tests/jobs.rs`; without it the tests return early): what a new scan
//! of a photo keeps of the owner's decisions, the grouping run (who joins whom, who never moves), the request queue, the "Is this X?"
//! suggestions and the cascades. The arithmetic itself is unit-tested in `people_math`.
use cyanotype_core::{
    db,
    faces::{self, DIM, MODEL, NewFace, VERSION},
    jobs, people,
};
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

async fn user(pool: &PgPool) -> Uuid {
    sqlx::query_scalar("INSERT INTO users (email, password_hash) VALUES ($1, 'x') RETURNING id")
        .bind(format!("{}@example.com", Uuid::new_v4()))
        .fetch_one(pool)
        .await
        .unwrap()
}

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

/// A deterministic stream of numbers in -1..1.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        ((self.0 >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0) as f32
    }
}

fn unit(mut v: Vec<f32>) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    v.iter_mut().for_each(|x| *x /= n);
    v
}

/// A made-up "person": a random direction. Different seeds are nearly orthogonal in 128 dimensions.
fn identity(seed: u64) -> Vec<f32> {
    let mut r = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    unit((0..DIM).map(|_| r.next()).collect())
}

/// A photo of `who`: the direction plus some noise (two such faces are about 0.85 alike, a stranger's about 0).
fn shot(who: &[f32], seed: u64) -> Vec<f32> {
    let mut r = Rng(seed.wrapping_mul(0xD1B5_4A32_D192_ED03) | 1);
    let noise = unit((0..DIM).map(|_| r.next()).collect());
    unit(who.iter().zip(&noise).map(|(a, n)| a + 0.4 * n).collect())
}

fn nf(bbox: [f32; 4], embedding: Option<Vec<f32>>) -> NewFace {
    NewFace {
        bbox,
        landmarks: [0.1, 0.2, 0.3, 0.2, 0.2, 0.3, 0.12, 0.4, 0.28, 0.4],
        score: 0.9,
        embedding,
    }
}

const B1: [f32; 4] = [0.1, 0.1, 0.2, 0.2];
const B2: [f32; 4] = [0.6, 0.1, 0.2, 0.2];

async fn store(pool: &PgPool, asset: Uuid, faces: &[NewFace]) {
    assert!(faces::store(pool, asset, MODEL, VERSION, faces).await.unwrap());
}

/// A new photo of `owner` with one fingerprinted face.
async fn photo_with(pool: &PgPool, owner: Uuid, e: Vec<f32>) -> (Uuid, Uuid) {
    let a = asset(pool, owner).await;
    store(pool, a, &[nf(B1, Some(e))]).await;
    (a, face_of(pool, a, 0).await)
}

async fn face_of(pool: &PgPool, asset: Uuid, idx: i16) -> Uuid {
    sqlx::query_scalar("SELECT id FROM faces WHERE asset_id = $1 AND model = $2 AND idx = $3")
        .bind(asset)
        .bind(MODEL)
        .bind(idx)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn person(pool: &PgPool, owner: Uuid, name: Option<&str>) -> Uuid {
    sqlx::query_scalar("INSERT INTO people (owner_id, name) VALUES ($1, $2) RETURNING id")
        .bind(owner)
        .bind(name)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn put(pool: &PgPool, face: Uuid, person: Uuid, by: &str) {
    sqlx::query("UPDATE faces SET person_id = $2, assigned_by = $3 WHERE id = $1")
        .bind(face)
        .bind(person)
        .bind(by)
        .execute(pool)
        .await
        .unwrap();
}

async fn reject(pool: &PgPool, face: Uuid, person: Uuid) {
    sqlx::query("INSERT INTO face_rejections (face_id, person_id) VALUES ($1, $2)")
        .bind(face)
        .bind(person)
        .execute(pool)
        .await
        .unwrap();
}

async fn who(pool: &PgPool, face: Uuid) -> (Option<Uuid>, String) {
    sqlx::query_as("SELECT person_id, assigned_by FROM faces WHERE id = $1")
        .bind(face)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn count(pool: &PgPool, sql: &str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
}

// ---- a new scan keeps what the owner decided ---------------------------------------------------------------------------------

#[tokio::test]
async fn a_rescan_after_a_rotate_keeps_people_rejections_and_the_cover_even_though_the_faces_moved() {
    let Some(t) = TestDb::new().await else { return };
    let owner = user(&t.pool).await;
    let (alpha, bravo) = (identity(1), identity(2));
    let a = asset(&t.pool, owner).await;
    store(
        &t.pool,
        a,
        &[nf(B1, Some(shot(&alpha, 10))), nf(B2, Some(shot(&bravo, 11)))],
    )
    .await;
    let (f0, f1) = (face_of(&t.pool, a, 0).await, face_of(&t.pool, a, 1).await);
    let (p_ann, p_bob) = (
        person(&t.pool, owner, Some("Person A")).await,
        person(&t.pool, owner, None).await,
    );
    put(&t.pool, f0, p_ann, "user").await;
    put(&t.pool, f1, p_bob, "auto").await;
    reject(&t.pool, f0, p_bob).await;
    sqlx::query("UPDATE people SET cover_face_id = $2 WHERE id = $1")
        .bind(p_ann)
        .bind(f0)
        .execute(&t.pool)
        .await
        .unwrap();

    // The photo is rotated: both faces are somewhere else and found in the other order, with a little different fingerprints.
    assert!(faces::forget_and_requeue(&t.pool, a, VERSION, true).await.unwrap());
    store(
        &t.pool,
        a,
        &[
            nf([0.3, 0.6, 0.2, 0.2], Some(shot(&bravo, 21))),
            nf([0.7, 0.7, 0.2, 0.2], Some(shot(&alpha, 20))),
        ],
    )
    .await;
    let (n0, n1) = (face_of(&t.pool, a, 0).await, face_of(&t.pool, a, 1).await);
    assert_eq!(who(&t.pool, n0).await, (Some(p_bob), "auto".to_string()));
    assert_eq!(who(&t.pool, n1).await, (Some(p_ann), "user".to_string()));
    let rej: Vec<(Uuid, Uuid)> = sqlx::query_as("SELECT face_id, person_id FROM face_rejections")
        .fetch_all(&t.pool)
        .await
        .unwrap();
    assert_eq!(rej, vec![(n1, p_bob)], "the rejection followed its face");
    let cover: Option<Uuid> = sqlx::query_scalar("SELECT cover_face_id FROM people WHERE id = $1")
        .bind(p_ann)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(cover, Some(n1));
    // A request to group was made.
    assert!(
        count(
            &t.pool,
            "SELECT count(*) FROM people_runs WHERE requested_at IS NOT NULL"
        )
        .await
            == 1
    );
    t.teardown().await;
}

#[tokio::test]
async fn a_rescan_keeps_decisions_for_faces_without_a_fingerprint_by_their_box_and_drops_those_that_vanished() {
    let Some(t) = TestDb::new().await else { return };
    let owner = user(&t.pool).await;
    let a = asset(&t.pool, owner).await;
    store(&t.pool, a, &[nf(B1, None), nf(B2, Some(shot(&identity(3), 1)))]).await;
    let (small, big) = (face_of(&t.pool, a, 0).await, face_of(&t.pool, a, 1).await);
    let p = person(&t.pool, owner, Some("Person B")).await;
    put(&t.pool, small, p, "user").await;
    put(&t.pool, big, p, "user").await;
    // A new version of the model: the small face is where it was, the other one is gone.
    faces::store(&t.pool, a, MODEL, VERSION + 1, &[nf([0.11, 0.1, 0.2, 0.2], None)])
        .await
        .unwrap();
    let n = face_of(&t.pool, a, 0).await;
    assert_eq!(who(&t.pool, n).await, (Some(p), "user".to_string()));
    assert_eq!(count(&t.pool, "SELECT count(*) FROM faces").await, 1);
    assert_eq!(
        count(&t.pool, "SELECT count(*) FROM people").await,
        1,
        "the person is not deleted with its face"
    );
    t.teardown().await;
}

#[tokio::test]
async fn a_new_model_takes_over_the_decisions_by_box_and_the_old_rows_give_them_up() {
    let Some(t) = TestDb::new().await else { return };
    let owner = user(&t.pool).await;
    let a = asset(&t.pool, owner).await;
    // The earlier model's faces; its fingerprints mean nothing next to the new model's.
    faces::store(&t.pool, a, "an-older-model", 1, &[nf(B1, Some(shot(&identity(4), 1)))])
        .await
        .unwrap();
    let old: Uuid = sqlx::query_scalar("SELECT id FROM faces WHERE model = 'an-older-model'")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    let p = person(&t.pool, owner, Some("Person C")).await;
    put(&t.pool, old, p, "user").await;
    reject(&t.pool, old, person(&t.pool, owner, None).await).await;
    store(&t.pool, a, &[nf([0.1, 0.12, 0.2, 0.2], Some(shot(&identity(5), 1)))]).await;
    let new = face_of(&t.pool, a, 0).await;
    assert_eq!(who(&t.pool, new).await, (Some(p), "user".to_string()));
    assert_eq!(who(&t.pool, old).await.0, None, "the person is counted once");
    assert_eq!(count(&t.pool, "SELECT count(*) FROM face_rejections").await, 1);
    // The sweep's pruning of the older model's rows loses nothing.
    faces::prune_superseded(&t.pool, MODEL, VERSION).await.unwrap();
    assert_eq!(who(&t.pool, new).await.0, Some(p));
    assert_eq!(count(&t.pool, "SELECT count(*) FROM face_rejections").await, 1);
    t.teardown().await;
}

#[tokio::test]
async fn forgetting_after_an_edit_keeps_only_what_the_owner_decided_and_a_scan_without_it_loses_no_person() {
    let Some(t) = TestDb::new().await else { return };
    let owner = user(&t.pool).await;
    let a = asset(&t.pool, owner).await;
    store(
        &t.pool,
        a,
        &[nf(B1, Some(shot(&identity(6), 1))), nf(B2, Some(shot(&identity(7), 1)))],
    )
    .await;
    let kept = face_of(&t.pool, a, 0).await;
    let p = person(&t.pool, owner, Some("Person D")).await;
    put(&t.pool, kept, p, "user").await;
    assert!(faces::forget_and_requeue(&t.pool, a, VERSION, false).await.unwrap());
    assert_eq!(
        count(&t.pool, "SELECT count(*) FROM faces").await,
        1,
        "the undecided face is forgotten"
    );
    assert_eq!(count(&t.pool, "SELECT count(*) FROM face_scans").await, 0);
    // The new picture has no face the old ones match: the person stays (named people are never deleted), without faces.
    store(&t.pool, a, &[]).await;
    assert_eq!(count(&t.pool, "SELECT count(*) FROM faces").await, 0);
    assert_eq!(
        count(&t.pool, "SELECT count(*) FROM people WHERE name = 'Person D'").await,
        1
    );
    t.teardown().await;
}

// ---- grouping ---------------------------------------------------------------------------------------------------------------

/// `n` photos of `who` for `owner`, returning the faces.
async fn photos_of(pool: &PgPool, owner: Uuid, who: &[f32], n: u64, seed: u64) -> Vec<Uuid> {
    let mut out = Vec::new();
    for i in 0..n {
        out.push(photo_with(pool, owner, shot(who, seed * 100 + i)).await.1);
    }
    out
}

#[tokio::test]
async fn unassigned_faces_become_people_only_when_there_are_enough_of_them() {
    let Some(t) = TestDb::new().await else { return };
    let owner = user(&t.pool).await;
    let (alpha, bravo, cy) = (identity(10), identity(11), identity(12));
    let a = photos_of(&t.pool, owner, &alpha, 4, 1).await;
    let b = photos_of(&t.pool, owner, &bravo, 3, 2).await;
    let c = photos_of(&t.pool, owner, &cy, 2, 3).await;
    // A face too small for a fingerprint is never grouped, and a lone stranger stays alone.
    let (_, tiny) = {
        let x = asset(&t.pool, owner).await;
        store(&t.pool, x, &[nf(B1, None)]).await;
        (x, face_of(&t.pool, x, 0).await)
    };
    let lone = photo_with(&t.pool, owner, identity(13)).await.1;

    let r = people::run(&t.pool, owner).await.unwrap();
    assert_eq!((r.assigned, r.created, r.grouped), (0, 2, 7));
    let group_of = |f: Uuid| {
        let pool = t.pool.clone();
        async move { who(&pool, f).await }
    };
    let pa = group_of(a[0]).await;
    assert!(pa.0.is_some() && pa.1 == "auto");
    for f in &a {
        assert_eq!(group_of(*f).await.0, pa.0);
    }
    let pb = group_of(b[0]).await.0;
    assert!(pb.is_some() && pb != pa.0);
    for f in &b {
        assert_eq!(group_of(*f).await.0, pb);
    }
    for f in c.iter().chain([&tiny, &lone]) {
        assert_eq!(group_of(*f).await.0, None, "too few, too small or alone: wait");
    }
    assert_eq!(
        count(&t.pool, "SELECT count(*) FROM people WHERE name IS NULL AND NOT hidden").await,
        2
    );

    // A third photo of the third person makes three: the next run groups them. Nothing else moves.
    let c3 = photo_with(&t.pool, owner, shot(&cy, 999)).await.1;
    let r = people::run(&t.pool, owner).await.unwrap();
    assert_eq!((r.assigned, r.created), (0, 1));
    let pc = who(&t.pool, c3).await.0;
    assert!(pc.is_some());
    for f in &c {
        assert_eq!(who(&t.pool, *f).await.0, pc);
    }
    assert_eq!(who(&t.pool, a[0]).await.0, pa.0);
    // Running again changes nothing.
    let r = people::run(&t.pool, owner).await.unwrap();
    assert_eq!((r.assigned, r.created, r.grouped), (0, 0, 0));
    t.teardown().await;
}

#[tokio::test]
async fn a_new_face_joins_the_person_it_looks_like_unless_it_was_rejected_from_them_or_two_people_look_alike() {
    let Some(t) = TestDb::new().await else { return };
    let owner = user(&t.pool).await;
    let (alpha, bravo) = (identity(20), identity(21));
    let pa = person(&t.pool, owner, Some("Person E")).await;
    let pb = person(&t.pool, owner, None).await;
    for f in photos_of(&t.pool, owner, &alpha, 3, 1).await {
        put(&t.pool, f, pa, "user").await;
    }
    for f in photos_of(&t.pool, owner, &bravo, 3, 2).await {
        put(&t.pool, f, pb, "auto").await;
    }
    let new_ann = photo_with(&t.pool, owner, shot(&alpha, 500)).await.1;
    let rejected_alpha = photo_with(&t.pool, owner, shot(&alpha, 501)).await.1;
    reject(&t.pool, rejected_alpha, pa).await;
    let stranger = photo_with(&t.pool, owner, identity(22)).await.1;
    // Halfway between Alpha and Bravo: two people alike, so neither.
    let between = photo_with(
        &t.pool,
        owner,
        unit(alpha.iter().zip(&bravo).map(|(a, b)| a + b).collect()),
    )
    .await
    .1;

    let r = people::run(&t.pool, owner).await.unwrap();
    assert_eq!(r.assigned, 1);
    assert_eq!(who(&t.pool, new_ann).await, (Some(pa), "auto".to_string()));
    assert_eq!(
        who(&t.pool, rejected_alpha).await.0,
        None,
        "never to a person it was rejected from"
    );
    assert_eq!(who(&t.pool, stranger).await.0, None);
    assert_eq!(who(&t.pool, between).await.0, None, "look-alikes are left alone");
    t.teardown().await;
}

#[tokio::test]
async fn decisions_of_the_owner_win_and_other_owners_are_invisible() {
    let Some(t) = TestDb::new().await else { return };
    let (me, other) = (user(&t.pool).await, user(&t.pool).await);
    let alpha = identity(30);
    // Mine: three faces of one look, put by me in a person named for someone else's face.
    let mine = photos_of(&t.pool, me, &alpha, 3, 1).await;
    let wrong_person = person(&t.pool, me, Some("Person F")).await;
    put(&t.pool, mine[0], wrong_person, "user").await;
    // Another owner has people of the same look: nothing crosses.
    let theirs = photos_of(&t.pool, other, &alpha, 4, 2).await;
    let their_person = person(&t.pool, other, None).await;
    for f in &theirs[..3] {
        put(&t.pool, *f, their_person, "auto").await;
    }
    let r = people::run(&t.pool, me).await.unwrap();
    // The two other faces of mine are the same look as the user's choice, so they join it, one by one; the user's face stays.
    assert_eq!(r.assigned, 2);
    assert_eq!(who(&t.pool, mine[0]).await, (Some(wrong_person), "user".to_string()));
    for f in &mine[1..] {
        assert_eq!(who(&t.pool, *f).await.0, Some(wrong_person));
    }
    assert_eq!(
        who(&t.pool, theirs[3]).await.0,
        None,
        "another owner's face was not touched by my run"
    );
    // The other owner's run assigns their last face to their person, not mine.
    people::run(&t.pool, other).await.unwrap();
    assert_eq!(who(&t.pool, theirs[3]).await.0, Some(their_person));
    // An unnamed person that lost every face goes at the next run, a named one stays.
    let ghost = person(&t.pool, me, None).await;
    let named = person(&t.pool, me, Some("Person G")).await;
    people::run(&t.pool, me).await.unwrap();
    let left: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM people WHERE owner_id = $1")
        .bind(me)
        .fetch_all(&t.pool)
        .await
        .unwrap();
    assert!(!left.contains(&ghost) && left.contains(&named) && left.contains(&wrong_person));
    t.teardown().await;
}

#[tokio::test]
async fn faces_of_one_photo_are_never_grouped_as_one_person() {
    let Some(t) = TestDb::new().await else { return };
    let owner = user(&t.pool).await;
    let alpha = identity(40);
    // Twins in every photo: the same look twice in each of three photos.
    for i in 0..3 {
        let a = asset(&t.pool, owner).await;
        store(
            &t.pool,
            a,
            &[nf(B1, Some(shot(&alpha, 600 + i))), nf(B2, Some(shot(&alpha, 700 + i)))],
        )
        .await;
    }
    people::run(&t.pool, owner).await.unwrap();
    // Each photo's two faces end up in different groups, or in none; never together.
    let together = count(
        &t.pool,
        "SELECT count(*) FROM faces a JOIN faces b ON a.asset_id = b.asset_id AND a.id < b.id
         WHERE a.person_id IS NOT NULL AND a.person_id = b.person_id",
    )
    .await;
    assert_eq!(together, 0);
    t.teardown().await;
}

#[tokio::test]
async fn a_group_next_to_one_known_person_joins_them_instead_of_making_a_duplicate() {
    let Some(t) = TestDb::new().await else { return };
    let owner = user(&t.pool).await;
    let alpha = identity(50);
    let p = person(&t.pool, owner, Some("Person H")).await;
    for f in photos_of(&t.pool, owner, &alpha, 3, 1).await {
        put(&t.pool, f, p, "user").await;
    }
    // Faces of her at an angle: each a little too far from any single known face to join alone (below the assign threshold), but
    // alike among themselves and, as a group, near enough the centre of the person. (Built as alpha plus a shared direction.)
    let mut r = Rng(77);
    let tilt = unit((0..DIM).map(|_| r.next()).collect());
    let angled = |s: u64| shot(&unit(alpha.iter().zip(&tilt).map(|(a, t)| a + 1.3 * t).collect()), s);
    let mut tilted = Vec::new();
    for s in 1..=3 {
        tilted.push(photo_with(&t.pool, owner, angled(s)).await.1);
    }
    let r = people::run(&t.pool, owner).await.unwrap();
    // However they got there, they are Person H's, and no second person was made for the same look.
    assert_eq!((r.created, r.assigned + r.grouped), (0, 3));
    for f in &tilted {
        assert_eq!(who(&t.pool, *f).await, (Some(p), "auto".to_string()));
    }
    assert_eq!(count(&t.pool, "SELECT count(*) FROM people").await, 1);
    t.teardown().await;
}

// ---- the queue of runs ------------------------------------------------------------------------------------------------------

#[tokio::test]
async fn a_run_is_due_when_the_faces_have_stopped_arriving_and_none_is_queued() {
    let Some(t) = TestDb::new().await else { return };
    let owner = user(&t.pool).await;
    let a = asset(&t.pool, owner).await;
    assert!(people::claim_due(&t.pool).await.unwrap().is_none());
    // Storing a face asks for a run, but not yet: the owner's faces may still be arriving.
    store(&t.pool, a, &[nf(B1, Some(shot(&identity(60), 1)))]).await;
    assert!(people::claim_due(&t.pool).await.unwrap().is_none());
    let age = |secs: i32| {
        let pool = t.pool.clone();
        async move {
            sqlx::query("UPDATE people_runs SET requested_at = now() - make_interval(secs => $1)")
                .bind(secs)
                .execute(&pool)
                .await
                .unwrap();
        }
    };
    age(people::QUIET_SECS as i32 + 5).await;
    // A scan still queued or running for one of the owner's photos holds it back.
    faces::enqueue_for_asset(&t.pool, a, VERSION).await.unwrap();
    assert!(people::claim_due(&t.pool).await.unwrap().is_none());
    let job = jobs::claim(&t.pool, jobs::KIND_FACES).await.unwrap().unwrap();
    assert!(people::claim_due(&t.pool).await.unwrap().is_none());
    jobs::complete(&t.pool, job.id).await.unwrap();
    let (claimed, answered) = people::claim_due(&t.pool).await.unwrap().expect("due");
    assert_eq!(claimed, owner);
    // Claimed once: nobody else gets it while it runs.
    assert!(people::claim_due(&t.pool).await.unwrap().is_none());
    // A request that comes in while it runs stays wanted after it finishes; one that does not is answered.
    people::request(&t.pool, owner).await.unwrap();
    people::finish(&t.pool, owner, answered, Ok(people::Report::default()))
        .await
        .unwrap();
    let wanted: bool = sqlx::query_scalar("SELECT requested_at IS NOT NULL FROM people_runs")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert!(wanted);
    age(people::QUIET_SECS as i32 + 5).await;
    let (_, answered) = people::claim_due(&t.pool).await.unwrap().expect("asked again");
    people::finish(&t.pool, owner, answered, Err("boom".into()))
        .await
        .unwrap();
    let (wanted, error): (bool, Option<String>) =
        sqlx::query_as("SELECT requested_at IS NOT NULL, error FROM people_runs")
            .fetch_one(&t.pool)
            .await
            .unwrap();
    assert!(!wanted);
    assert_eq!(error.as_deref(), Some("boom"));
    t.teardown().await;
}

// ---- suggestions and the index ----------------------------------------------------------------------------------------------

#[tokio::test]
async fn suggestions_are_the_unassigned_faces_near_a_person_never_the_ones_rejected_or_in_the_trash() {
    let Some(t) = TestDb::new().await else { return };
    let (owner, other) = (user(&t.pool).await, user(&t.pool).await);
    let alpha = identity(70);
    let p = person(&t.pool, owner, Some("Person I")).await;
    for f in photos_of(&t.pool, owner, &alpha, 3, 1).await {
        put(&t.pool, f, p, "user").await;
    }
    let (near_a, near) = photo_with(&t.pool, owner, shot(&alpha, 800)).await;
    let (_, rejected) = photo_with(&t.pool, owner, shot(&alpha, 801)).await;
    reject(&t.pool, rejected, p).await;
    let (trashed_a, _) = photo_with(&t.pool, owner, shot(&alpha, 802)).await;
    sqlx::query("UPDATE assets SET deleted_at = now() WHERE id = $1")
        .bind(trashed_a)
        .execute(&t.pool)
        .await
        .unwrap();
    let (hidden_a, _) = photo_with(&t.pool, owner, shot(&alpha, 803)).await;
    sqlx::query("UPDATE assets SET hidden = true WHERE id = $1")
        .bind(hidden_a)
        .execute(&t.pool)
        .await
        .unwrap();
    photo_with(&t.pool, owner, identity(71)).await;
    photo_with(&t.pool, other, shot(&alpha, 804)).await;

    let s = people::suggestions(&t.pool, owner, p, false, 10).await.unwrap();
    assert_eq!(
        s.iter().map(|s| (s.face_id, s.asset_id)).collect::<Vec<_>>(),
        vec![(near, near_a)]
    );
    assert!(s[0].similarity > 0.6 && s[0].bbox == B1);
    let unlocked = people::suggestions(&t.pool, owner, p, true, 10).await.unwrap();
    assert_eq!(
        unlocked.len(),
        2,
        "a hidden photo's face only with the Hidden view unlocked"
    );
    // A person without faces suggests nothing.
    let empty = person(&t.pool, owner, None).await;
    assert!(
        people::suggestions(&t.pool, owner, empty, true, 10)
            .await
            .unwrap()
            .is_empty()
    );
    t.teardown().await;
}

async fn not_a_face(pool: &PgPool, face: Uuid) {
    sqlx::query("UPDATE faces SET person_id = NULL, not_face = true WHERE id = $1")
        .bind(face)
        .execute(pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_detection_marked_not_a_face_is_never_grouped_joined_or_suggested() {
    let Some(t) = TestDb::new().await else { return };
    let owner = user(&t.pool).await;
    let (alpha, bravo) = (identity(90), identity(91));
    // Alpha has four photos, one of them a false detection: three real faces still make a person, and the false one stays out of it.
    let a = photos_of(&t.pool, owner, &alpha, 4, 1).await;
    not_a_face(&t.pool, a[0]).await;
    // Bravo has three, one false: two real faces are too few for a person.
    let b = photos_of(&t.pool, owner, &bravo, 3, 2).await;
    not_a_face(&t.pool, b[0]).await;

    let r = people::run(&t.pool, owner).await.unwrap();
    assert_eq!((r.looked_at, r.created, r.grouped), (5, 1, 3));
    assert_eq!(who(&t.pool, a[0]).await.0, None);
    let pa = who(&t.pool, a[1]).await.0;
    assert!(pa.is_some());
    for f in &a[1..] {
        assert_eq!(who(&t.pool, *f).await.0, pa);
    }
    for f in &b {
        assert_eq!(who(&t.pool, *f).await.0, None);
    }
    assert_eq!(
        r.left, 2,
        "Bravo's two real faces are left; the false detections are not counted"
    );

    // A new face of Alpha joins; a marked one of Alpha does not, and is not suggested either.
    let joined = photo_with(&t.pool, owner, shot(&alpha, 700)).await.1;
    let (_, marked) = photo_with(&t.pool, owner, shot(&alpha, 701)).await;
    not_a_face(&t.pool, marked).await;
    let (_, open) = photo_with(&t.pool, owner, shot(&alpha, 702)).await;
    let suggested = people::suggestions(&t.pool, owner, pa.unwrap(), false, 10)
        .await
        .unwrap();
    let mut got: Vec<Uuid> = suggested.iter().map(|s| s.face_id).collect();
    got.sort();
    let mut want = vec![joined, open];
    want.sort();
    assert_eq!(got, want, "the marked face is not suggested");
    assert!(
        suggested
            .iter()
            .all(|s| s.face_id != marked && s.face_id != a[0] && s.face_id != b[0])
    );
    let r = people::run(&t.pool, owner).await.unwrap();
    assert_eq!(r.assigned, 2);
    assert_eq!(who(&t.pool, joined).await.0, pa);
    assert_eq!(who(&t.pool, open).await.0, pa);
    assert_eq!(who(&t.pool, marked).await.0, None);
    assert_eq!(
        count(
            &t.pool,
            "SELECT count(*) FROM faces WHERE person_id IS NULL AND NOT not_face AND embedding IS NOT NULL"
        )
        .await,
        2,
        "only Bravo's two real faces are ungrouped"
    );
    // The worker's guarded write cannot give a marked face a person, and the table does not allow it.
    assert!(
        sqlx::query("UPDATE faces SET person_id = $2 WHERE id = $1")
            .bind(marked)
            .bind(pa)
            .execute(&t.pool)
            .await
            .is_err()
    );
    t.teardown().await;
}

#[tokio::test]
async fn not_a_face_is_undone_by_clearing_it_and_survives_a_rescan_and_an_edit() {
    let Some(t) = TestDb::new().await else { return };
    let owner = user(&t.pool).await;
    let (alpha, bravo) = (identity(95), identity(96));
    let a = asset(&t.pool, owner).await;
    store(
        &t.pool,
        a,
        &[nf(B1, Some(shot(&alpha, 1))), nf(B2, Some(shot(&bravo, 2)))],
    )
    .await;
    let (f0, f1) = (face_of(&t.pool, a, 0).await, face_of(&t.pool, a, 1).await);
    not_a_face(&t.pool, f1).await;

    // A rotate forgets what nobody decided about but keeps the mark (and the scan waits for the new one).
    assert!(faces::forget_and_requeue(&t.pool, a, VERSION, true).await.unwrap());
    assert_eq!(count(&t.pool, "SELECT count(*) FROM faces").await, 1);
    store(
        &t.pool,
        a,
        &[
            nf([0.3, 0.6, 0.2, 0.2], Some(shot(&bravo, 12))),
            nf([0.7, 0.7, 0.2, 0.2], Some(shot(&alpha, 11))),
        ],
    )
    .await;
    let (n0, n1) = (face_of(&t.pool, a, 0).await, face_of(&t.pool, a, 1).await);
    assert!(![f0, f1].contains(&n0));
    let marks: Vec<(Uuid, bool)> = sqlx::query_as("SELECT id, not_face FROM faces WHERE asset_id = $1 ORDER BY idx")
        .bind(a)
        .fetch_all(&t.pool)
        .await
        .unwrap();
    assert_eq!(
        marks,
        vec![(n0, true), (n1, false)],
        "Bravo's face moved to index 0 and is still marked"
    );

    // A scan without fingerprints finds it again by its box.
    store(
        &t.pool,
        a,
        &[nf([0.3, 0.6, 0.2, 0.2], None), nf([0.7, 0.7, 0.2, 0.2], None)],
    )
    .await;
    assert_eq!(
        count(
            &t.pool,
            &format!("SELECT count(*) FROM faces WHERE asset_id = '{a}' AND not_face")
        )
        .await,
        1
    );

    // Undo: the face is an ordinary unassigned one again.
    sqlx::query("UPDATE faces SET not_face = false WHERE asset_id = $1")
        .bind(a)
        .execute(&t.pool)
        .await
        .unwrap();
    assert_eq!(count(&t.pool, "SELECT count(*) FROM faces WHERE not_face").await, 0);
    t.teardown().await;
}

#[tokio::test]
async fn another_owners_faces_are_never_touched_by_a_mark_and_only_real_people_are_counted() {
    let Some(t) = TestDb::new().await else { return };
    let (owner, other) = (user(&t.pool).await, user(&t.pool).await);
    let alpha = identity(97);
    let mine = photos_of(&t.pool, owner, &alpha, 3, 1).await;
    let theirs = photos_of(&t.pool, other, &alpha, 3, 2).await;
    not_a_face(&t.pool, mine[0]).await;
    people::run(&t.pool, owner).await.unwrap();
    people::run(&t.pool, other).await.unwrap();
    // Mine: two real faces, no person. Theirs: three, one person.
    assert_eq!(count(&t.pool, "SELECT count(*) FROM people").await, 1);
    assert!(who(&t.pool, theirs[0]).await.0.is_some());
    assert_eq!(who(&t.pool, mine[1]).await.0, None);
    t.teardown().await;
}

#[tokio::test]
async fn the_nearest_neighbour_queries_use_the_models_index() {
    let Some(t) = TestDb::new().await else { return };
    let owner = user(&t.pool).await;
    photo_with(&t.pool, owner, shot(&identity(80), 1)).await;
    let mut tx = t.pool.begin().await.unwrap();
    sqlx::query("SET LOCAL enable_seqscan = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    let plan: Vec<String> = sqlx::query_scalar(&format!(
        "EXPLAIN SELECT f.id FROM faces f WHERE f.model = '{MODEL}'
         ORDER BY f.embedding::halfvec(128) <=> $1::text::halfvec(128) LIMIT 5"
    ))
    .bind(cyanotype_core::embeddings::pg_literal(&identity(80)))
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    assert!(plan.iter().any(|l| l.contains(faces::INDEX_NAME)), "{plan:#?}");
    t.teardown().await;
}

// ---- cascades and the delete command ----------------------------------------------------------------------------------------

#[tokio::test]
async fn people_and_rejections_go_with_their_account_and_a_purged_photo_takes_its_rejections() {
    let Some(t) = TestDb::new().await else { return };
    let (me, other) = (user(&t.pool).await, user(&t.pool).await);
    let (a, fa) = photo_with(&t.pool, me, identity(90)).await;
    let (_, fb) = photo_with(&t.pool, other, identity(91)).await;
    let (p, q) = (
        person(&t.pool, me, Some("Person J")).await,
        person(&t.pool, other, Some("Person K")).await,
    );
    put(&t.pool, fa, p, "user").await;
    reject(&t.pool, fb, q).await;
    reject(&t.pool, fa, person(&t.pool, me, None).await).await;
    // The photo goes: its face and the face's rejection with it; the person stays, empty.
    sqlx::query("DELETE FROM assets WHERE id = $1")
        .bind(a)
        .execute(&t.pool)
        .await
        .unwrap();
    assert_eq!(count(&t.pool, "SELECT count(*) FROM face_rejections").await, 1);
    assert_eq!(
        count(&t.pool, "SELECT count(*) FROM people").await,
        3,
        "the people stay"
    );
    // Deleting a person sets its faces free and drops the rejections that named it.
    sqlx::query("DELETE FROM people WHERE id = $1")
        .bind(q)
        .execute(&t.pool)
        .await
        .unwrap();
    assert_eq!(count(&t.pool, "SELECT count(*) FROM face_rejections").await, 0);
    assert_eq!(who(&t.pool, fb).await.0, None);
    // The account goes: its people and requests with it.
    people::request(&t.pool, me).await.unwrap();
    sqlx::query("DELETE FROM assets WHERE owner_id = $1")
        .bind(me)
        .execute(&t.pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(me)
        .execute(&t.pool)
        .await
        .unwrap();
    assert_eq!(
        count(&t.pool, &format!("SELECT count(*) FROM people WHERE owner_id = '{me}'")).await,
        0
    );
    assert_eq!(
        count(
            &t.pool,
            &format!("SELECT count(*) FROM people_runs WHERE owner_id = '{me}'")
        )
        .await,
        0
    );
    t.teardown().await;
}

#[tokio::test]
async fn delete_all_removes_people_and_rejections_and_counts_them_first() {
    let Some(t) = TestDb::new().await else { return };
    let (me, other) = (user(&t.pool).await, user(&t.pool).await);
    let (_, f1) = photo_with(&t.pool, me, identity(95)).await;
    let (_, f2) = photo_with(&t.pool, other, identity(96)).await;
    let (p, q) = (
        person(&t.pool, me, Some("Person L")).await,
        person(&t.pool, other, None).await,
    );
    put(&t.pool, f1, p, "user").await;
    reject(&t.pool, f2, p).await;
    reject(&t.pool, f2, q).await;
    let d = faces::delete_all(&t.pool, Some(me), true).await.unwrap();
    assert_eq!((d.faces, d.people, d.rejections), (1, 1, 0));
    assert_eq!(
        count(&t.pool, "SELECT count(*) FROM people").await,
        2,
        "a dry run deletes nothing"
    );
    let d = faces::delete_all(&t.pool, Some(other), true).await.unwrap();
    assert_eq!((d.faces, d.people, d.rejections), (1, 1, 2));
    faces::delete_all(&t.pool, Some(me), false).await.unwrap();
    assert_eq!(
        count(&t.pool, "SELECT count(*) FROM people").await,
        1,
        "the other owner's person stays"
    );
    assert_eq!(count(&t.pool, "SELECT count(*) FROM people_runs").await, 1);
    faces::delete_all(&t.pool, None, false).await.unwrap();
    for table in ["people", "faces", "face_rejections", "people_runs", "face_scans"] {
        assert_eq!(
            count(&t.pool, &format!("SELECT count(*) FROM {table}")).await,
            0,
            "{table}"
        );
    }
    t.teardown().await;
}

// ---- cost -------------------------------------------------------------------------------------------------------------------

fn crowd(k: usize) -> Vec<Vec<f32>> {
    (0..k).map(|i| identity(i as u64 + 1)).collect()
}

/// How long one run takes on a big library (printed; `cargo test -p cyanotype-core --release --test people big_library -- --ignored
/// --nocapture`, with `PEOPLE_BIG_FACES` faces, default 30,000): 25 photos of each of many made-up identities, nobody known at first.
#[tokio::test]
#[ignore = "a measurement, not a check"]
async fn big_library() {
    let Some(t) = TestDb::new().await else { return };
    let n: usize = std::env::var("PEOPLE_BIG_FACES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30_000);
    let owner = user(&t.pool).await;
    let ids = crowd(n / 25);
    let started = std::time::Instant::now();
    let mut assets: Vec<Uuid> = Vec::new();
    for chunk in (0..n).collect::<Vec<_>>().chunks(5_000) {
        let new: Vec<Uuid> = sqlx::query_scalar(
            "INSERT INTO assets (owner_id, sha256, original_key, original_name, content_type, size_bytes, thumbhash)
             SELECT $1, sha256(gen_random_uuid()::text::bytea), 'k', 'p.jpg', 'image/jpeg', 1, '\\x01'
             FROM generate_series(1, $2::int) RETURNING id",
        )
        .bind(owner)
        .bind(chunk.len() as i32)
        .fetch_all(&t.pool)
        .await
        .unwrap();
        assets.extend(new);
    }
    for (c, chunk) in assets.chunks(5_000).enumerate() {
        let lits: Vec<String> = (0..chunk.len())
            .map(|i| {
                let k = c * 5_000 + i;
                cyanotype_core::embeddings::pg_literal(&shot(&ids[k % ids.len()], k as u64 + 1))
            })
            .collect();
        sqlx::query(&format!(
            "INSERT INTO faces (asset_id, owner_id, model, version, idx, box_x, box_y, box_w, box_h, landmarks, score, embedding)
             SELECT a, $1, '{MODEL}', 1, 0, 0.1, 0.1, 0.2, 0.2, array_fill(0.1::real, ARRAY[10]), 0.9, e::halfvec
             FROM unnest($2::uuid[], $3::text[]) AS u(a, e)"
        ))
        .bind(owner)
        .bind(chunk)
        .bind(&lits)
        .execute(&t.pool)
        .await
        .unwrap();
    }
    sqlx::query(
        "INSERT INTO face_scans (asset_id, model, version, faces) SELECT id, $2, 1, 1 FROM assets WHERE owner_id = $1",
    )
    .bind(owner)
    .bind(MODEL)
    .execute(&t.pool)
    .await
    .unwrap();
    sqlx::query("ANALYZE faces").execute(&t.pool).await.unwrap();
    println!(
        "loaded {n} faces of {} identities in {:?}",
        ids.len(),
        started.elapsed()
    );
    let first = std::time::Instant::now();
    let r = people::run(&t.pool, owner).await.unwrap();
    println!("first run (nobody known): {:?} -> {r:?}", first.elapsed());
    let people_n = count(&t.pool, "SELECT count(*) FROM people").await;
    println!("{people_n} people for {} identities", ids.len());
    // 5 % new faces of known identities.
    for k in 0..n / 20 {
        let a = asset(&t.pool, owner).await;
        store(
            &t.pool,
            a,
            &[nf(B1, Some(shot(&ids[k % ids.len()], (n + k) as u64 + 7)))],
        )
        .await;
    }
    let second = std::time::Instant::now();
    let r = people::run(&t.pool, owner).await.unwrap();
    println!(
        "second run ({} new faces, {people_n} people): {:?} -> {r:?}",
        n / 20,
        second.elapsed()
    );
    t.teardown().await;
}
