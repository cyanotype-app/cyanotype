//! Text in photos through the API (`GET /assets/{id}/text`, the search merge, the `has_text` filter, the backfill and model
//! switch, processing progress), over a real Postgres. The reading itself is tested in `cyanotype-ocr` and measured by its
//! bake-off; here the rows the worker writes are made by hand.
use axum::http::StatusCode;
use cyanotype_core::{jobs, ocr};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use super::search_v2_tests::{ids, photo};
use crate::testutil::{TEST_MODEL, TestDb, app, asset, send, user};

fn found(text: &str, qr: Value) -> ocr::Found {
    ocr::Found {
        text: text.to_string(),
        lines: json!(text.lines().enumerate().map(|(i, l)| json!({
            "t": l, "s": 0.95, "q": [0.1, 0.1 + i as f64 * 0.1, 0.8, 0.1 + i as f64 * 0.1, 0.8, 0.15 + i as f64 * 0.1, 0.1, 0.15 + i as f64 * 0.1],
            "w": []
        })).collect::<Vec<_>>()),
        qr,
    }
}

async fn read(pool: &PgPool, id: Uuid, text: &str, qr: Value) {
    ocr::store(pool, id, ocr::MODEL, ocr::VERSION, &found(text, qr))
        .await
        .unwrap();
}

/// Every photo has its derived images (a thumbhash): what makes a photo readable for the sweep and countable in progress.
async fn with_previews(pool: &PgPool) {
    sqlx::query("UPDATE assets SET thumbhash = '\\x0102'")
        .execute(pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn the_text_route_gives_text_lines_and_links_only_for_web_addresses() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(&t.pool);
    let (alice, key) = user(&t.pool, "a@example.com").await;
    let (_, bkey) = user(&t.pool, "b@example.com").await;
    let id = asset(&t.pool, alice, "receipt.jpg").await;

    // Not read yet: a known state, not an error.
    let (st, v) = send(&app, "GET", &format!("/assets/{id}/text"), &key, None).await;
    assert_eq!(
        (st, v["status"].clone(), v["text"].clone()),
        (StatusCode::OK, json!("pending"), json!("")),
        "{v}"
    );

    read(
        &t.pool,
        id,
        "HOFER KG\nTotal 22,41",
        json!([
            {"t": "https://example.org/r?id=1", "q": [0.1, 0.1, 0.2, 0.1, 0.2, 0.2, 0.1, 0.2]},
            {"t": "javascript:alert(1)", "q": [0.3, 0.3, 0.4, 0.3, 0.4, 0.4, 0.3, 0.4]},
            {"t": "WIFI:S:net;T:WPA;P:pw;;", "q": [0.5, 0.5, 0.6, 0.5, 0.6, 0.6, 0.5, 0.6]}
        ]),
    )
    .await;
    let (st, v) = send(&app, "GET", &format!("/assets/{id}/text"), &key, None).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(
        (v["status"].clone(), v["text"].clone()),
        (json!("read"), json!("HOFER KG\nTotal 22,41"))
    );
    assert_eq!(v["lines"].as_array().unwrap().len(), 2);
    assert_eq!(v["lines"][1]["q"].as_array().unwrap().len(), 8);
    let qr = v["qr"].as_array().unwrap();
    assert_eq!(qr[0]["link"], "https://example.org/r?id=1");
    assert!(
        qr[1].get("link").is_none() && qr[2].get("link").is_none(),
        "only a plain web address is a link: {v}"
    );

    // Someone else's photo, an unknown one and one in the trash are not found.
    let (st, _) = send(&app, "GET", &format!("/assets/{id}/text"), &bkey, None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = send(&app, "GET", &format!("/assets/{}/text", Uuid::new_v4()), &key, None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    sqlx::query("UPDATE assets SET deleted_at = now() WHERE id = $1")
        .bind(id)
        .execute(&t.pool)
        .await
        .unwrap();
    let (st, _) = send(&app, "GET", &format!("/assets/{id}/text"), &key, None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    // A hidden photo's text is as hidden as the photo: an API key never unlocks the Hidden view, and search never lists it.
    sqlx::query("UPDATE assets SET deleted_at = NULL, hidden = true WHERE id = $1")
        .bind(id)
        .execute(&t.pool)
        .await
        .unwrap();
    let (st, _) = send(&app, "GET", &format!("/assets/{id}/text"), &key, None).await;
    assert_eq!(st, StatusCode::NOT_FOUND, "hidden and locked");
    let (st, r) = send(&app, "GET", "/assets/search?q=hofer", &key, None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(!ids(&r).contains(&id), "a hidden photo's text is in no search: {r}");
    t.teardown().await;
}

#[tokio::test]
async fn text_in_a_photo_is_found_by_search_after_captions_and_before_the_ranking() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(&t.pool);
    let (alice, key) = user(&t.pool, "a@example.com").await;
    let (bob, _) = user(&t.pool, "b@example.com").await;
    let m = TEST_MODEL;
    let sign = photo(&t.pool, alice, "sign.jpg", m, &[("car", 1.0)]).await;
    let captioned = photo(&t.pool, alice, "captioned.jpg", m, &[("car", 1.0)]).await;
    let other = photo(&t.pool, alice, "other.jpg", m, &[("cat", 1.0)]).await;
    let theirs = photo(&t.pool, bob, "theirs.jpg", m, &[("car", 1.0)]).await;
    let both = photo(&t.pool, alice, "both.jpg", m, &[("dog", 1.0)]).await;
    read(&t.pool, sign, "ABBEY ROAD NW8\nCITY OF WESTMINSTER", json!([])).await;
    read(&t.pool, theirs, "Abbey Road", json!([])).await;
    read(&t.pool, other, "", json!([])).await; // read, nothing there
    read(&t.pool, both, "Abbey Road souvenir", json!([])).await;
    for (id, c) in [(captioned, "Walking down Abbey Road"), (both, "Our abbey road photo")] {
        sqlx::query("UPDATE assets SET caption = $2 WHERE id = $1")
            .bind(id)
            .bind(c)
            .execute(&t.pool)
            .await
            .unwrap();
    }
    sqlx::query("UPDATE assets SET taken_at = '2020-01-01T00:00:00Z' WHERE id = $1")
        .bind(sign)
        .execute(&t.pool)
        .await
        .unwrap();

    let (st, r) = send(&app, "GET", "/assets/search?q=abbey%20road", &key, None).await;
    assert_eq!(st, StatusCode::OK, "{r}");
    let got = ids(&r);
    // Captions first (newest first), then text in the picture; the photo with both is listed once, as a caption match.
    assert_eq!(got.iter().take(3).copied().collect::<Vec<_>>().len(), 3);
    let pos = |id| got.iter().position(|x| *x == id).unwrap();
    assert!(
        pos(captioned) < pos(sign) && pos(both) < pos(sign),
        "captions come first: {got:?}"
    );
    assert_eq!(got.iter().filter(|i| **i == both).count(), 1);
    assert!(!got.contains(&theirs), "someone else's text is theirs");
    // Every word must be in the text, in any case, as part of a word; a number can be half of what is written.
    let (_, r) = send(&app, "GET", "/assets/search?q=abbey%20westminster", &key, None).await;
    assert!(ids(&r).contains(&sign), "{r}");
    let (_, r) = send(&app, "GET", "/assets/search?q=abbey%20nowhere", &key, None).await;
    assert!(!ids(&r).iter().take(1).any(|i| *i == sign));
    let (_, r) = send(&app, "GET", "/assets/search?q=minste", &key, None).await;
    assert_eq!(ids(&r)[0], sign, "part of a word: {r}");
    // The filters apply to text matches too.
    let (_, r) = send(&app, "GET", "/assets/search?q=abbey&from=2024-01-01", &key, None).await;
    assert!(!ids(&r).contains(&sign), "{r}");
    t.teardown().await;
}

#[tokio::test]
async fn the_has_text_filter_and_its_facet_count_only_photos_that_were_read_with_text() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(&t.pool);
    let (alice, key) = user(&t.pool, "a@example.com").await;
    let m = TEST_MODEL;
    let with = photo(&t.pool, alice, "with.jpg", m, &[("cat", 1.0)]).await;
    let blank = photo(&t.pool, alice, "blank.jpg", m, &[("cat", 1.0)]).await;
    let unread = photo(&t.pool, alice, "unread.jpg", m, &[("cat", 1.0)]).await;
    read(&t.pool, with, "STOP", json!([])).await;
    read(&t.pool, blank, "   ", json!([])).await;

    let (_, r) = send(&app, "GET", "/assets/search?has_text=true", &key, None).await;
    assert_eq!(ids(&r), vec![with], "{r}");
    let (_, r) = send(&app, "GET", "/assets/search?has_text=false", &key, None).await;
    let mut no = ids(&r);
    no.sort();
    let mut want = vec![blank, unread];
    want.sort();
    assert_eq!(no, want, "no text: read and empty, or not read yet");
    let (_, f) = send(&app, "GET", "/assets/search/facets", &key, None).await;
    assert_eq!(
        (f["total"].clone(), f["with_text"].clone()),
        (json!(3), json!(1)),
        "{f}"
    );
    t.teardown().await;
}

#[tokio::test]
async fn the_sweep_queues_what_the_current_model_has_not_read_and_a_new_version_reads_everything_again() {
    let Some(t) = TestDb::new().await else { return };
    let (alice, _) = user(&t.pool, "a@example.com").await;
    let m = TEST_MODEL;
    let read_now = photo(&t.pool, alice, "read.jpg", m, &[("cat", 1.0)]).await;
    let unread = photo(&t.pool, alice, "unread.jpg", m, &[("cat", 1.0)]).await;
    let no_preview = asset(&t.pool, alice, "nopreview.jpg").await;
    with_previews(&t.pool).await;
    sqlx::query("UPDATE assets SET thumbhash = NULL WHERE id = $1")
        .bind(no_preview)
        .execute(&t.pool)
        .await
        .unwrap();
    read(&t.pool, read_now, "hello", json!([])).await;

    assert_eq!(ocr::missing_count(&t.pool, ocr::MODEL, ocr::VERSION).await.unwrap(), 1);
    assert_eq!(
        ocr::enqueue_missing(&t.pool, ocr::MODEL, ocr::VERSION).await.unwrap(),
        1,
        "only the unread photo with a preview"
    );
    let queued: Vec<(Uuid, i16)> = sqlx::query_as("SELECT asset_id, priority FROM jobs WHERE kind = 'ocr'")
        .fetch_all(&t.pool)
        .await
        .unwrap();
    assert_eq!(queued, vec![(unread, jobs::PRIORITY_BACKFILL)], "at backfill priority");
    assert_eq!(
        ocr::enqueue_missing(&t.pool, ocr::MODEL, ocr::VERSION).await.unwrap(),
        0,
        "a queued job is left alone"
    );

    // A model switch: a newer version has read nothing, so every photo with a preview is queued again, and the text of the old
    // version keeps answering until its photo's new text replaces it.
    let next = ocr::VERSION + 1;
    assert_eq!(ocr::missing_count(&t.pool, ocr::MODEL, next).await.unwrap(), 2);
    assert_eq!(ocr::enqueue_missing(&t.pool, ocr::MODEL, next).await.unwrap(), 2);
    let before: String = sqlx::query_scalar("SELECT text FROM asset_text WHERE asset_id = $1")
        .bind(read_now)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(before, "hello");
    ocr::store(&t.pool, read_now, ocr::MODEL, next, &found("hello again", json!([])))
        .await
        .unwrap();
    let (v, text): (i32, String) = sqlx::query_as("SELECT version, text FROM asset_text WHERE asset_id = $1")
        .bind(read_now)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(
        (v, text.as_str()),
        (next, "hello again"),
        "one row per photo, replaced in place"
    );
    assert_eq!(ocr::missing_count(&t.pool, ocr::MODEL, next).await.unwrap(), 1);
    t.teardown().await;
}

#[tokio::test]
async fn reading_text_is_reported_apart_from_the_processing_banner() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(&t.pool);
    let (alice, key) = user(&t.pool, "a@example.com").await;
    let a = photo(&t.pool, alice, "a.jpg", TEST_MODEL, &[("cat", 1.0)]).await;
    let b = photo(&t.pool, alice, "b.jpg", TEST_MODEL, &[("cat", 1.0)]).await;
    with_previews(&t.pool).await;
    sqlx::query("DELETE FROM jobs").execute(&t.pool).await.unwrap();
    read(&t.pool, a, "x", json!([])).await;
    ocr::enqueue_missing(&t.pool, ocr::MODEL, ocr::VERSION).await.unwrap();
    let _ = b;
    let (st, v) = send(&app, "GET", "/assets/processing", &key, None).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["active"], false, "a text backfill is not 'processing': {v}");
    assert_eq!(v["queued"], 0);
    assert!(v["by_kind"].as_array().unwrap().is_empty(), "{v}");
    assert_eq!(
        (
            v["text"]["read"].clone(),
            v["text"]["total"].clone(),
            v["text"]["queued"].clone(),
            v["text"]["percent"].clone()
        ),
        (json!(1), json!(2), json!(1), json!(50)),
        "{v}"
    );
    assert_eq!(v["text"]["model"], ocr::MODEL);
    t.teardown().await;
}

#[tokio::test]
async fn the_simple_configuration_searches_any_script_and_the_trigram_index_serves_part_of_a_word() {
    let Some(t) = TestDb::new().await else { return };
    let (alice, _) = user(&t.pool, "a@example.com").await;
    let id = asset(&t.pool, alice, "mixed.jpg").await;
    read(
        &t.pool,
        id,
        "BIÊN NHẬN CHUYỂN TIỀN\nКВИТАНЦИЯ Москва\nSumme 142005 EUR\n大德大路",
        json!([]),
    )
    .await;
    let hits = |q: &'static str| {
        let pool = t.pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM asset_text WHERE tsv @@ plainto_tsquery('simple', $1)")
                .bind(q)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    assert_eq!(hits("москва").await, 1, "Cyrillic, lower-cased by the configuration");
    assert_eq!(
        hits("nhận").await,
        1,
        "Vietnamese diacritics are kept, not stemmed away"
    );
    assert_eq!(hits("eur summe").await, 1);
    assert_eq!(hits("nothing").await, 0);
    let part: i64 = sqlx::query_scalar("SELECT count(*) FROM asset_text WHERE text ILIKE '%4200%'")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(part, 1, "the middle of a number");
    let has: bool = sqlx::query_scalar("SELECT has_text FROM asset_text WHERE asset_id = $1")
        .bind(id)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert!(has);
    t.teardown().await;
}
