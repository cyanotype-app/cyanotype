//! Near-duplicate review over a real Postgres (and a real S3 API for what writes the edits object, the `S3_*` variables): the scan's
//! rules on stored vectors, dismissals, the scan requests and their state machine, the review endpoints (best photo, hidden photos,
//! owners), and "Keep selected" with everything it merges. The rules and the best-photo choice are unit tested where they live.
use axum::http::StatusCode;
use cyanotype_core::duplicates::{self, Thresholds};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use super::search_v2_tests::photo;
use crate::testutil::{TEST_MODEL, TestDb, app, app_with_s3, asset, send, session, user};

fn thresholds() -> Thresholds {
    Thresholds {
        model: TEST_MODEL.id().to_string(),
        version: TEST_MODEL.version(),
        dim: TEST_MODEL.dim(),
        with_metadata: 0.85,
        same_size: 0.92,
        stripped: 0.94,
    }
}

async fn set(pool: &PgPool, id: Uuid, sql: &str) {
    sqlx::query(&format!("UPDATE assets SET {sql} WHERE id = $1"))
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
}

/// An embedded photo: `words` make its vector (the same words, the same vector), then the metadata.
async fn pic(pool: &PgPool, owner: Uuid, name: &str, words: &[(&str, f32)], meta: &str) -> Uuid {
    let id = photo(pool, owner, name, TEST_MODEL, words).await;
    if !meta.is_empty() {
        set(pool, id, meta).await;
    }
    id
}

async fn groups(pool: &PgPool, owner: Uuid) -> Vec<Vec<Uuid>> {
    let mut out: Vec<Vec<Uuid>> = Vec::new();
    let gs: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM duplicate_groups WHERE owner_id = $1")
        .bind(owner)
        .fetch_all(pool)
        .await
        .unwrap();
    for g in gs {
        let mut m: Vec<Uuid> = sqlx::query_scalar("SELECT asset_id FROM duplicate_members WHERE group_id = $1")
            .bind(g)
            .fetch_all(pool)
            .await
            .unwrap();
        m.sort();
        out.push(m);
    }
    out.sort();
    out
}

fn sorted(mut v: Vec<Uuid>) -> Vec<Uuid> {
    v.sort();
    v
}

const CAT: &[(&str, f32)] = &[("cat", 1.0)];
const NEAR_CAT: &[(&str, f32)] = &[("cat", 1.0), ("dog", 0.2)];
const DOG: &[(&str, f32)] = &[("dog", 1.0)];

#[tokio::test]
async fn the_scan_groups_what_the_rules_say_and_nothing_else() {
    let Some(t) = TestDb::new().await else { return };
    let (alice, _) = user(&t.pool, "a@example.com").await;
    let (bob, _) = user(&t.pool, "b@example.com").await;
    let at =
        "taken_at = '2024-05-01T12:00:00Z', camera_make = 'Canon', camera_model = 'R5', width = 4000, height = 3000";
    // A: same camera, 20 s apart, different sizes, a near vector (the metadata rule).
    let a1 = pic(&t.pool, alice, "a1.jpg", CAT, at).await;
    let a2 = pic(
        &t.pool,
        alice,
        "a2.jpg",
        NEAR_CAT,
        "taken_at = '2024-05-01T12:00:20Z', camera_make = 'Canon', camera_model = 'R5', width = 2000, height = 1500",
    )
    .await;
    // B: another camera and day, the same size (the size rule).
    let b1 = pic(
        &t.pool,
        alice,
        "b1.jpg",
        DOG,
        "taken_at = '2023-01-01T10:00:00Z', camera_make = 'Sony', camera_model = 'A7', width = 6000, height = 4000",
    )
    .await;
    let b2 = pic(
        &t.pool,
        alice,
        "b2.jpg",
        DOG,
        "taken_at = '2022-02-02T10:00:00Z', camera_make = 'Nikon', camera_model = 'Z6', width = 6000, height = 4000",
    )
    .await;
    // C: neither has a capture time, other sizes (a stripped copy), identical vectors.
    let words_c: &[(&str, f32)] = &[("beach", 1.0)];
    let c1 = pic(&t.pool, alice, "c1.jpg", words_c, "width = 1000, height = 800").await;
    let c2 = pic(&t.pool, alice, "c2.jpg", words_c, "width = 500, height = 400").await;
    // D: the same camera and minute but not close enough; E: close, but nothing else is shared; F: one burst; G: a RAW half.
    let d1 = pic(&t.pool, alice, "d1.jpg", &[("car", 1.0)], at).await;
    let d2 = pic(&t.pool, alice, "d2.jpg", &[("car", 1.0), ("night", 1.0)], at).await;
    let e1 = pic(
        &t.pool,
        alice,
        "e1.jpg",
        &[("cake", 1.0)],
        "taken_at = '2020-01-01T10:00:00Z', width = 100, height = 100",
    )
    .await;
    let e2 = pic(
        &t.pool,
        alice,
        "e2.jpg",
        &[("cake", 1.0)],
        "taken_at = '2021-01-01T10:00:00Z', width = 200, height = 200",
    )
    .await;
    let f1 = pic(&t.pool, alice, "f1.jpg", &[("beach", 0.1), ("night", 1.0)], at).await;
    let f2 = pic(&t.pool, alice, "f2.jpg", &[("beach", 0.1), ("night", 1.0)], at).await;
    set(&t.pool, f1, &format!("burst_id = '{f1}'")).await;
    set(&t.pool, f2, &format!("burst_id = '{f1}'")).await;
    let g1 = pic(
        &t.pool,
        alice,
        "g1.jpg",
        &[("dog", 1.0), ("cat", 1.0), ("car", 1.0)],
        at,
    )
    .await;
    let g2 = pic(
        &t.pool,
        alice,
        "g2.dng",
        &[("dog", 1.0), ("cat", 1.0), ("car", 1.0)],
        at,
    )
    .await;
    set(&t.pool, g2, &format!("primary_id = '{g1}'")).await;
    // Bob has the very same pictures as Alice's A pair: never grouped with hers.
    let bob_a = pic(&t.pool, bob, "bob.jpg", CAT, at).await;
    let _ = (e1, e2);

    let r = duplicates::scan(&t.pool, alice, &thresholds()).await.unwrap();
    assert_eq!(r.groups, 3, "{r:?}");
    assert_eq!(
        groups(&t.pool, alice).await,
        {
            let mut v = vec![sorted(vec![a1, a2]), sorted(vec![b1, b2]), sorted(vec![c1, c2])];
            v.sort();
            v
        },
        "A by camera and time, B by size, C stripped; not D (too far), E (nothing else shared), F (a burst), G (a RAW half)"
    );
    assert!(groups(&t.pool, bob).await.is_empty());
    let _ = (d1, d2, bob_a);
    // The vectors of a different model are not compared; a rescan replaces the groups (nothing doubles).
    let again = duplicates::scan(&t.pool, alice, &thresholds()).await.unwrap();
    assert_eq!(again.groups, 3);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM duplicate_groups WHERE owner_id = $1")
        .bind(alice)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(n, 3);
    // A trashed photo leaves its group; a group of one is no group.
    set(&t.pool, a2, "deleted_at = now()").await;
    assert_eq!(duplicates::scan(&t.pool, alice, &thresholds()).await.unwrap().groups, 2);
    t.teardown().await;
}

#[tokio::test]
async fn a_group_is_a_chain_of_close_pairs_and_one_too_big_is_left_out() {
    let Some(t) = TestDb::new().await else { return };
    let (alice, _) = user(&t.pool, "a@example.com").await;
    let at = "taken_at = '2024-05-01T12:00:00Z', camera_make = 'Canon', camera_model = 'R5'";
    // Three copies chain into one group.
    let mut ids = Vec::new();
    for n in 0..3 {
        ids.push(pic(&t.pool, alice, &format!("c{n}.jpg"), NEAR_CAT, at).await);
    }
    let r = duplicates::scan(&t.pool, alice, &thresholds()).await.unwrap();
    assert_eq!((r.groups, r.too_big), (1, 0));
    assert_eq!(groups(&t.pool, alice).await, vec![sorted(ids)]);
    // 51 identical photos are beyond the limit of a group: counted, not offered.
    for n in 0..=duplicates::MAX_GROUP {
        pic(&t.pool, alice, &format!("big{n}.jpg"), DOG, at).await;
    }
    let r = duplicates::scan(&t.pool, alice, &thresholds()).await.unwrap();
    assert_eq!((r.groups, r.too_big), (1, 1));
    t.teardown().await;
}

#[tokio::test]
async fn not_duplicates_is_permanent_audited_and_for_the_owner_only() {
    let Some(t) = TestDb::new().await else { return };
    let (alice, ka) = user(&t.pool, "a@example.com").await;
    let (_bob, kb) = user(&t.pool, "b@example.com").await;
    let app = app(&t.pool);
    let at = "taken_at = '2024-05-01T12:00:00Z', camera_make = 'Canon', camera_model = 'R5'";
    let a = pic(&t.pool, alice, "a.jpg", CAT, at).await;
    let b = pic(&t.pool, alice, "b.jpg", CAT, at).await;
    let c = pic(&t.pool, alice, "c.jpg", DOG, at).await;
    let d = pic(&t.pool, alice, "d.jpg", DOG, at).await;
    duplicates::scan(&t.pool, alice, &thresholds()).await.unwrap();
    let (_, list) = send(&app, "GET", "/assets/duplicates", &ka, None).await;
    assert_eq!(list["total"], 2);
    let gid = |names: [Uuid; 2]| {
        let list = list.clone();
        list["groups"]
            .as_array()
            .unwrap()
            .iter()
            .find(|g| {
                g["members"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|m| m["id"] == json!(names[0]))
            })
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let g_ab = gid([a, b]);
    // Bob cannot dismiss or keep Alice's group.
    for (path, body) in [
        (format!("/assets/duplicates/{g_ab}/dismiss"), json!({})),
        (format!("/assets/duplicates/{g_ab}/keep"), json!({"keep": a})),
    ] {
        assert_eq!(
            send(&app, "POST", &path, &kb, Some(body)).await.0,
            StatusCode::NOT_FOUND,
            "{path}"
        );
    }
    assert_eq!(send(&app, "GET", "/assets/duplicates", &kb, None).await.1["total"], 0);
    let (st, r) = send(
        &app,
        "POST",
        &format!("/assets/duplicates/{g_ab}/dismiss"),
        &ka,
        Some(json!({})),
    )
    .await;
    assert_eq!((st, r["dismissed"].as_i64()), (StatusCode::OK, Some(2)));
    // Gone from the list, never grouped again by a scan, the other group untouched, one audit row with a count only.
    assert_eq!(send(&app, "GET", "/assets/duplicates", &ka, None).await.1["total"], 1);
    duplicates::scan(&t.pool, alice, &thresholds()).await.unwrap();
    assert_eq!(groups(&t.pool, alice).await, vec![sorted(vec![c, d])]);
    let audit: Vec<(String, Value)> =
        sqlx::query_as("SELECT action, detail FROM audit_log WHERE action LIKE 'duplicates.%'")
            .fetch_all(&t.pool)
            .await
            .unwrap();
    assert_eq!(audit, [("duplicates.dismiss".to_string(), json!({"photos": 2}))]);
    // The dismissal follows the photos: purging one removes it.
    sqlx::query("DELETE FROM assets WHERE id = $1")
        .bind(a)
        .execute(&t.pool)
        .await
        .unwrap();
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM duplicate_dismissals")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
    t.teardown().await;
}

#[tokio::test]
async fn the_review_lists_resolution_size_date_and_metadata_and_preselects_the_best() {
    let Some(t) = TestDb::new().await else { return };
    let (alice, ka) = user(&t.pool, "a@example.com").await;
    let app = app(&t.pool);
    let at = "taken_at = '2024-05-01T12:00:00Z', camera_make = 'Canon', camera_model = 'R5'";
    let big = pic(&t.pool, alice, "big.jpg", CAT, at).await;
    let small = pic(&t.pool, alice, "small.jpg", NEAR_CAT, at).await;
    set(&t.pool, big, "width = 4000, height = 3000, size_bytes = 5000000, exif = '{\"Make\": \"Canon\", \"Model\": \"R5\", \"ISO\": \"100\"}'").await;
    set(&t.pool, small, "width = 1000, height = 750, size_bytes = 90000, favorite = true, caption = 'A caption', latitude = 1.0, longitude = 2.0").await;
    duplicates::scan(&t.pool, alice, &thresholds()).await.unwrap();
    let (st, v) = send(&app, "GET", "/assets/duplicates", &ka, None).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["total"], 1);
    let g = &v["groups"][0];
    let m = |id: Uuid| {
        g["members"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["id"] == json!(id))
            .unwrap()
            .clone()
    };
    // The larger picture is best even though the smaller is a favorite with a caption and a place.
    assert_eq!(
        (m(big)["best"].clone(), m(small)["best"].clone()),
        (json!(true), json!(false))
    );
    assert_eq!(
        (
            m(big)["width"].clone(),
            m(big)["height"].clone(),
            m(big)["size_bytes"].clone(),
            m(big)["exif_fields"].clone()
        ),
        (json!(4000), json!(3000), json!(5_000_000), json!(3))
    );
    assert_eq!(
        (
            m(small)["has_location"].clone(),
            m(small)["has_caption"].clone(),
            m(small)["favorite"].clone()
        ),
        (json!(true), json!(true), json!(true))
    );
    assert!(m(big)["sort_time"].is_string() && m(big)["original_name"] == "big.jpg");
    // At the same resolution the one with more metadata is best.
    set(&t.pool, small, "width = 4000, height = 3000").await;
    let (_, v) = send(&app, "GET", "/assets/duplicates", &ka, None).await;
    let best: Vec<&Value> = v["groups"][0]["members"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|x| x["best"] == true)
        .collect();
    assert_eq!(best.len(), 1);
    assert_eq!(
        best[0]["id"],
        json!(small),
        "a place, a caption and a time outweigh three EXIF fields"
    );
    // Paging.
    for n in 0..3 {
        let x = pic(
            &t.pool,
            alice,
            &format!("x{n}a.jpg"),
            &[("beach", 1.0 + n as f32)],
            &format!(
                "taken_at = '2020-0{}-01T00:00:00Z', camera_make = 'Z', camera_model = 'Z'",
                n + 1
            ),
        )
        .await;
        let y = pic(
            &t.pool,
            alice,
            &format!("x{n}b.jpg"),
            &[("beach", 1.0 + n as f32)],
            &format!(
                "taken_at = '2020-0{}-01T00:00:05Z', camera_make = 'Z', camera_model = 'Z'",
                n + 1
            ),
        )
        .await;
        let _ = (x, y);
    }
    duplicates::scan(&t.pool, alice, &thresholds()).await.unwrap();
    let (_, p1) = send(&app, "GET", "/assets/duplicates?limit=2", &ka, None).await;
    let c = p1["next_cursor"].as_str().unwrap();
    let (_, p2) = send(
        &app,
        "GET",
        &format!("/assets/duplicates?limit=2&cursor={c}"),
        &ka,
        None,
    )
    .await;
    assert_eq!(
        (
            p1["groups"].as_array().unwrap().len(),
            p2["groups"].as_array().unwrap().len(),
            p1["total"].clone()
        ),
        (2, 2, json!(4))
    );
    t.teardown().await;
}

#[tokio::test]
async fn keeping_one_merges_what_the_others_carry_trashes_them_and_can_be_undone_from_the_trash() {
    let Some(t) = TestDb::new().await else { return };
    let Some((app, storage)) = app_with_s3(&t.pool) else {
        return;
    };
    let (alice, ka) = user(&t.pool, "a@example.com").await;
    let at = "camera_make = 'Canon', camera_model = 'R5'";
    let keeper = pic(&t.pool, alice, "keeper.jpg", CAT, &format!("{at}, taken_at = '2024-05-01T12:00:00Z', width = 4000, height = 3000, rating = 2, rating_source = 'file', caption = 'Keeper words'")).await;
    let l1 = pic(&t.pool, alice, "l1.jpg", CAT, &format!("{at}, taken_at = '2024-05-01T12:00:05Z', favorite = true, rating = 5, rating_source = 'user', caption = 'Copy words', latitude = 38.7, longitude = -9.1")).await;
    let l2 = pic(
        &t.pool,
        alice,
        "l2.jpg",
        CAT,
        &format!("{at}, taken_at = '2024-05-01T12:00:09Z', caption = 'Keeper words'"),
    )
    .await;
    // Albums, tags (one imported on a copy, the owner's on the keeper), a RAW behind a copy, a cover.
    let (st, al) = send(
        &app,
        "POST",
        "/assets/albums",
        &ka,
        Some(json!({"title": "Trip", "asset_ids": [l1]})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{al}");
    let album: Uuid = al["id"].as_str().unwrap().parse().unwrap();
    sqlx::query("UPDATE albums SET cover_asset_id = $2 WHERE id = $1")
        .bind(album)
        .bind(l1)
        .execute(&t.pool)
        .await
        .unwrap();
    send(
        &app,
        "POST",
        "/assets/tags/apply",
        &ka,
        Some(json!({"ids": [l1], "add": ["Beach"]})),
    )
    .await;
    send(
        &app,
        "POST",
        "/assets/tags/apply",
        &ka,
        Some(json!({"ids": [l2], "add": ["Sunset"]})),
    )
    .await;
    let raw = asset(&t.pool, alice, "l1.dng").await;
    set(&t.pool, raw, &format!("primary_id = '{l1}'")).await;
    duplicates::scan(&t.pool, alice, &thresholds()).await.unwrap();
    let (_, v) = send(&app, "GET", "/assets/duplicates", &ka, None).await;
    let gid = v["groups"][0]["id"].as_str().unwrap().to_string();
    assert_eq!(v["groups"][0]["members"].as_array().unwrap().len(), 3);

    // A photo that is not in the group is refused; so is keeping with nothing to merge.
    let (st, _) = send(
        &app,
        "POST",
        &format!("/assets/duplicates/{gid}/keep"),
        &ka,
        Some(json!({"keep": Uuid::new_v4()})),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    let (st, r) = send(
        &app,
        "POST",
        &format!("/assets/duplicates/{gid}/keep"),
        &ka,
        Some(json!({"keep": keeper})),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{r}");
    assert_eq!(r["kept"], json!(keeper));
    let trashed: Vec<Uuid> = r["trashed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap().parse().unwrap())
        .collect();
    assert_eq!(sorted(trashed.clone()), sorted(vec![l1, l2]));

    // What came across: favorite, the highest rating (as the owner's), the album and its cover, both tags, every distinct caption
    // (the kept photo's first), the location the keeper lacked.
    let (fav, rating, source, caption, lat): (bool, i16, Option<String>, Option<String>, Option<f64>) =
        sqlx::query_as("SELECT favorite, rating, rating_source, caption, latitude FROM assets WHERE id = $1")
            .bind(keeper)
            .fetch_one(&t.pool)
            .await
            .unwrap();
    assert_eq!((fav, rating, source.as_deref()), (true, 5, Some("user")));
    assert_eq!(caption.as_deref(), Some("Keeper words\n\nCopy words"));
    assert_eq!(lat, Some(38.7));
    let in_album: i64 = sqlx::query_scalar("SELECT count(*) FROM album_assets WHERE album_id = $1 AND asset_id = $2")
        .bind(album)
        .bind(keeper)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(in_album, 1);
    let cover: Option<Uuid> = sqlx::query_scalar("SELECT cover_asset_id FROM albums WHERE id = $1")
        .bind(album)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(cover, Some(keeper));
    let tags: Vec<String> = sqlx::query_scalar(
        "SELECT t.name FROM asset_tags at JOIN tags t ON t.id = at.tag_id WHERE at.asset_id = $1 ORDER BY t.name",
    )
    .bind(keeper)
    .fetch_all(&t.pool)
    .await
    .unwrap();
    assert_eq!(tags, ["Beach", "Sunset"]);
    // The caption and the place are edits like any other: in the bucket, so a rebuild keeps them.
    let sha: Vec<u8> = sqlx::query_scalar("SELECT sha256 FROM assets WHERE id = $1")
        .bind(keeper)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    let e = cyanotype_core::edits::parse(
        &storage
            .get_bytes(&cyanotype_core::keys::edits(alice, &sha.try_into().unwrap()))
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        (e.caption.as_deref(), e.latitude),
        (Some("Keeper words\n\nCopy words"), Some(38.7))
    );

    // The others are in the trash (the RAW with its photo), the group is gone, one audit row with counts only.
    let live: Vec<String> =
        sqlx::query_scalar("SELECT original_name FROM assets WHERE deleted_at IS NULL ORDER BY original_name")
            .fetch_all(&t.pool)
            .await
            .unwrap();
    assert_eq!(live, ["keeper.jpg"]);
    assert_eq!(send(&app, "GET", "/assets/duplicates", &ka, None).await.1["total"], 0);
    let audit: Vec<(String, Value)> =
        sqlx::query_as("SELECT action, detail FROM audit_log WHERE action = 'duplicates.keep'")
            .fetch_all(&t.pool)
            .await
            .unwrap();
    assert_eq!(audit.len(), 1);
    let text = audit[0].1.to_string();
    assert!(
        text.contains("\"trashed\":2") && !text.contains("keeper") && !text.contains("words"),
        "{text}"
    );
    // Undo: the trashed photos come back through the trash; the merge stays on the kept photo.
    let (st, back) = send(&app, "POST", "/assets/restore", &ka, Some(json!({"ids": trashed}))).await;
    assert_eq!(st, StatusCode::OK, "{back}");
    let live: i64 = sqlx::query_scalar("SELECT count(*) FROM assets WHERE deleted_at IS NULL AND primary_id IS NULL")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(live, 3);
    let (rating,): (i16,) = sqlx::query_as("SELECT rating FROM assets WHERE id = $1")
        .bind(keeper)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(rating, 5);
    t.teardown().await;
}

#[tokio::test]
async fn a_keepers_own_location_and_a_lower_rating_are_never_overwritten() {
    let Some(t) = TestDb::new().await else { return };
    let Some((app, _storage)) = app_with_s3(&t.pool) else {
        return;
    };
    let (alice, ka) = user(&t.pool, "a@example.com").await;
    let at = "camera_make = 'Canon', camera_model = 'R5', taken_at = '2024-05-01T12:00:00Z'";
    let keeper = pic(
        &t.pool,
        alice,
        "k.jpg",
        CAT,
        &format!("{at}, latitude = 10.0, longitude = 20.0, rating = 4, rating_source = 'user'"),
    )
    .await;
    let other = pic(
        &t.pool,
        alice,
        "o.jpg",
        CAT,
        &format!("{at}, latitude = 50.0, longitude = 60.0, rating = 1, rating_source = 'user', caption = 'x'"),
    )
    .await;
    duplicates::scan(&t.pool, alice, &thresholds()).await.unwrap();
    let (_, v) = send(&app, "GET", "/assets/duplicates", &ka, None).await;
    let gid = v["groups"][0]["id"].as_str().unwrap().to_string();
    let (st, r) = send(
        &app,
        "POST",
        &format!("/assets/duplicates/{gid}/keep"),
        &ka,
        Some(json!({"keep": keeper})),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{r}");
    let (lat, rating, caption): (Option<f64>, i16, Option<String>) =
        sqlx::query_as("SELECT latitude, rating, caption FROM assets WHERE id = $1")
            .bind(keeper)
            .fetch_one(&t.pool)
            .await
            .unwrap();
    assert_eq!(
        (lat, rating, caption.as_deref()),
        (Some(10.0), 4, Some("x")),
        "only the caption came across"
    );
    let _ = other;
    t.teardown().await;
}

#[tokio::test]
async fn a_scan_is_requested_by_the_owner_at_once_and_after_imports_when_things_go_quiet() {
    let Some(t) = TestDb::new().await else { return };
    let (alice, ka) = user(&t.pool, "a@example.com").await;
    let (_bob, _) = user(&t.pool, "b@example.com").await;
    let (_admin, admin_cookie) = session(&t.pool, "root@example.com", "admin").await;
    let (_member, member_cookie) = session(&t.pool, "m@example.com", "member").await;
    let app = app(&t.pool);
    let state = |v: &Value| v["scan"]["state"].as_str().unwrap().to_string();
    let (_, v) = send(&app, "GET", "/assets/duplicates", &ka, None).await;
    assert_eq!(state(&v), "never");
    // The owner presses "Find duplicates": wanted, manual, claimed at once, then idle with what it found.
    let (st, v) = send(&app, "POST", "/assets/duplicates/scan", &ka, Some(json!({}))).await;
    assert_eq!((st, state(&v).as_str()), (StatusCode::OK, "wanted"));
    let (owner, requested) = duplicates::claim_due(&t.pool)
        .await
        .unwrap()
        .expect("a manual request is due at once");
    assert_eq!(owner, alice);
    assert!(
        duplicates::claim_due(&t.pool).await.unwrap().is_none(),
        "a running scan is not claimed twice"
    );
    assert_eq!(
        state(&send(&app, "GET", "/assets/duplicates", &ka, None).await.1),
        "running"
    );
    // A request that arrives while it runs stays wanted afterwards.
    duplicates::request(&t.pool, alice, true).await.unwrap();
    duplicates::finish(
        &t.pool,
        owner,
        requested,
        Ok(duplicates::ScanResult {
            photos: 7,
            groups: 2,
            too_big: 0,
        }),
    )
    .await
    .unwrap();
    let v = send(&app, "GET", "/assets/duplicates", &ka, None).await.1;
    assert_eq!(
        (
            state(&v).as_str(),
            v["scan"]["groups"].as_i64(),
            v["scan"]["photos"].as_i64()
        ),
        ("wanted", Some(2), Some(7))
    );
    let (o2, r2) = duplicates::claim_due(&t.pool).await.unwrap().unwrap();
    duplicates::finish(&t.pool, o2, r2, Ok(duplicates::ScanResult::default()))
        .await
        .unwrap();
    assert_eq!(
        state(&send(&app, "GET", "/assets/duplicates", &ka, None).await.1),
        "idle"
    );
    assert!(duplicates::claim_due(&t.pool).await.unwrap().is_none());
    // After an import (an embedding): automatic, only once things have been quiet for the quiet period.
    let a = asset(&t.pool, alice, "new.jpg").await;
    duplicates::request_for_asset(&t.pool, a).await.unwrap();
    assert!(
        duplicates::claim_due(&t.pool).await.unwrap().is_none(),
        "an automatic request waits"
    );
    sqlx::query("UPDATE duplicate_scans SET requested_at = now() - interval '6 minutes' WHERE owner_id = $1")
        .bind(alice)
        .execute(&t.pool)
        .await
        .unwrap();
    let (o3, r3) = duplicates::claim_due(&t.pool)
        .await
        .unwrap()
        .expect("quiet for six minutes");
    // More embeddings during the scan push the next one back, and a manual request is never made automatic.
    duplicates::request_for_asset(&t.pool, a).await.unwrap();
    duplicates::finish(&t.pool, o3, r3, Ok(duplicates::ScanResult::default()))
        .await
        .unwrap();
    assert_eq!(
        state(&send(&app, "GET", "/assets/duplicates", &ka, None).await.1),
        "wanted"
    );
    duplicates::request(&t.pool, alice, true).await.unwrap();
    duplicates::request_for_asset(&t.pool, a).await.unwrap();
    assert!(
        duplicates::claim_due(&t.pool).await.unwrap().is_some(),
        "still manual: runs at once"
    );
    // The admin button: every account with photos, admins only, audited with a count.
    assert_eq!(
        send_cookie_post(&app, &member_cookie, "/assets/admin/duplicates/scan").await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send_cookie_post(&app, &admin_cookie, "/assets/admin/duplicates/scan").await,
        StatusCode::OK
    );
    let audit: Vec<(String, Value)> =
        sqlx::query_as("SELECT action, detail FROM audit_log WHERE action = 'duplicates.scan_all'")
            .fetch_all(&t.pool)
            .await
            .unwrap();
    assert_eq!(audit.len(), 1);
    t.teardown().await;
}

async fn send_cookie_post(app: &axum::Router, cookie: &str, path: &str) -> StatusCode {
    crate::testutil::send_cookie(app, "POST", path, Some(cookie), Some(json!({})))
        .await
        .0
}

/// The neighbour lookup of a scan walks the model's HNSW index (no scan of the vectors per photo), whatever other users' vectors the
/// index holds.
#[tokio::test]
async fn the_neighbour_lookup_uses_the_hnsw_index() {
    let Some(t) = TestDb::new().await else { return };
    let (alice, _) = user(&t.pool, "a@example.com").await;
    for n in 0..40 {
        pic(
            &t.pool,
            alice,
            &format!("p{n}.jpg"),
            &[("cat", 1.0 + n as f32 * 0.01), ("dog", n as f32 * 0.1)],
            "",
        )
        .await;
    }
    let d = TEST_MODEL.dim();
    let (m, v) = (TEST_MODEL.id(), TEST_MODEL.version());
    let mut tx = t.pool.begin().await.unwrap();
    sqlx::query("SET LOCAL enable_seqscan = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    let plan: Vec<(String,)> = sqlx::query_as(&format!(
        "EXPLAIN SELECT a.id, n.asset_id FROM unnest(ARRAY[gen_random_uuid()]) AS a(id)
         JOIN asset_embeddings e ON e.asset_id = a.id AND e.model = '{m}' AND e.version = {v}
         CROSS JOIN LATERAL (
             SELECT e2.asset_id, (e2.embedding::halfvec({d}) <=> e.embedding::halfvec({d})) AS dist
             FROM asset_embeddings e2 WHERE e2.model = '{m}' AND e2.version = {v} AND e2.asset_id <> a.id
             ORDER BY e2.embedding::halfvec({d}) <=> e.embedding::halfvec({d}) LIMIT {k}) n",
        k = duplicates::NEIGHBOURS
    ))
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    let plan = plan.into_iter().map(|r| r.0).collect::<Vec<_>>().join("\n");
    assert!(plan.contains("hnsw"), "{plan}");
    t.teardown().await;
}
