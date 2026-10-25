// The flows against a stand-in server, with a real listener on the loopback address and a "browser" that plays the person.
use std::sync::{Arc, Mutex};

use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
use serde_json::{Value, json};

use super::*;

#[derive(Default)]
struct Fake {
    /// The challenge the "browser" saw in the address, and what the token endpoint was asked.
    challenge: String,
    exchanged: Vec<Value>,
    polls: usize,
    /// Scripted answers of the device poll, one per poll; the last repeats.
    script: Vec<(StatusCode, Value)>,
}

type Shared = Arc<Mutex<Fake>>;

fn err(code: &str) -> (StatusCode, Value) {
    (StatusCode::BAD_REQUEST, json!({"error": "refused", "code": code}))
}

fn key() -> (StatusCode, Value) {
    (StatusCode::OK, json!({"key": "cyk_test", "email": "alpha@example.com"}))
}

async fn serve(fake: Shared) -> String {
    async fn token(State(f): State<Shared>, Json(b): Json<Value>) -> (StatusCode, Json<Value>) {
        let mut f = f.lock().unwrap();
        let verifier = b["code_verifier"].as_str().unwrap_or_default();
        let ok = b["code"] == "code-1" && Pkce::from_verifier(verifier.into()).challenge == f.challenge;
        f.exchanged.push(b);
        let (s, v) = if ok { key() } else { err("invalid_grant") };
        (s, Json(v))
    }
    async fn device(Json(b): Json<Value>) -> Json<Value> {
        assert_eq!(
            (b["device"].as_str(), b["scope"].as_str()),
            (Some("test-laptop"), Some("import"))
        );
        Json(json!({
            "device_code": "dev-1", "user_code": "WDJB-MJHT", "verification_path": "/activate",
            "expires_in": 600, "interval": 5,
        }))
    }
    async fn poll(State(f): State<Shared>, Json(b): Json<Value>) -> (StatusCode, Json<Value>) {
        let mut f = f.lock().unwrap();
        assert_eq!(b["device_code"], "dev-1");
        let i = f.polls.min(f.script.len() - 1);
        f.polls += 1;
        let (s, v) = f.script[i].clone();
        (s, Json(v))
    }
    let app = Router::new()
        .route("/auth/cli/token", post(token))
        .route("/auth/cli/device", post(device))
        .route("/auth/cli/device/token", post(poll))
        .with_state(fake);
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    base
}

/// The query parameters of the address the CLI asked the browser to open.
fn params(url: &str) -> std::collections::HashMap<String, String> {
    reqwest::Url::parse(url)
        .unwrap()
        .query_pairs()
        .map(|(k, v)| (k.into(), v.into()))
        .collect()
}

/// A browser that, once opened, requests the callback with `callback_query` (STATE replaced by the real state).
fn browser(fake: Shared, callback_query: &'static str) -> impl FnOnce(&str) -> bool {
    move |url: &str| {
        let p = params(url);
        assert!(url.contains("/cli/authorize?"), "{url}");
        assert!(p["redirect_uri"].starts_with("http://127.0.0.1:") && p["redirect_uri"].ends_with("/callback"));
        assert_eq!(
            (
                p["code_challenge_method"].as_str(),
                p["device"].as_str(),
                p["scope"].as_str()
            ),
            ("S256", "test-laptop", "import")
        );
        fake.lock().unwrap().challenge = p["code_challenge"].clone();
        let (base, state) = (p["redirect_uri"].clone(), p["state"].clone());
        tokio::spawn(async move {
            let http = reqwest::Client::new();
            // Noise first: a favicon request and a callback with somebody else's state change nothing.
            let r = http
                .get(base.replace("/callback", "/favicon.ico"))
                .send()
                .await
                .unwrap();
            assert_eq!(r.status(), 404);
            let r = http.get(format!("{base}?code=evil&state=wrong")).send().await.unwrap();
            assert_eq!(r.status(), 404);
            let page = http
                .get(format!("{base}?{}", callback_query.replace("STATE", &state)))
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap();
            assert!(
                page.contains("close this tab") || page.contains("Login cancelled"),
                "{page}"
            );
        });
        true
    }
}

#[tokio::test]
async fn the_loopback_flow_trades_the_code_with_the_verifier_for_a_key() {
    let fake = Shared::default();
    let base = serve(fake.clone()).await;
    let api = Api::new(&base, None).unwrap();
    let open = browser(fake.clone(), "code=code-1&state=STATE");
    let grant = login_loopback(&api, &base, "import", "test-laptop", open, Duration::from_secs(10))
        .await
        .unwrap();
    assert_eq!(
        (grant.key.as_str(), grant.email.as_str()),
        ("cyk_test", "alpha@example.com")
    );
    let f = fake.lock().unwrap();
    assert_eq!(
        f.exchanged.len(),
        1,
        "the wrong-state callback must not reach the server"
    );
    assert_eq!(f.exchanged[0]["code"], "code-1");
    assert!(f.exchanged[0]["redirect_uri"].as_str().unwrap().ends_with("/callback"));
}

#[tokio::test]
async fn a_denied_login_and_a_server_refusal_are_errors() {
    let fake = Shared::default();
    let base = serve(fake.clone()).await;
    let api = Api::new(&base, None).unwrap();
    let open = browser(fake.clone(), "error=access_denied&state=STATE");
    let e = login_loopback(&api, &base, "import", "test-laptop", open, Duration::from_secs(10))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("denied"), "{e}");
    assert!(fake.lock().unwrap().exchanged.is_empty());
    // The server turning the code down (replayed, expired) is reported, not hidden.
    let open = browser(fake.clone(), "code=stale&state=STATE");
    let e = login_loopback(&api, &base, "import", "test-laptop", open, Duration::from_secs(10))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("refused"), "{e}");
}

#[tokio::test]
async fn waiting_for_the_browser_gives_up_and_an_unopenable_browser_is_not_fatal() {
    let fake = Shared::default();
    let base = serve(fake).await;
    let api = Api::new(&base, None).unwrap();
    // The browser could not be opened (the address is printed instead); nobody comes, so the wait ends.
    let e = login_loopback(
        &api,
        &base,
        "import",
        "test-laptop",
        |_| false,
        Duration::from_millis(200),
    )
    .await
    .unwrap_err();
    assert!(e.to_string().contains("timed out"), "{e}");
}

#[tokio::test]
async fn the_device_flow_polls_through_pending_and_slow_down_to_the_key() {
    let fake = Shared::default();
    fake.lock().unwrap().script = vec![
        err("authorization_pending"),
        (
            StatusCode::BAD_REQUEST,
            json!({"error": "x", "code": "slow_down", "interval": 10}),
        ),
        err("authorization_pending"),
        key(),
    ];
    let base = serve(fake.clone()).await;
    let api = Api::new(&base, None).unwrap();
    // One "second" is a millisecond here.
    let grant = login_device(&api, &base, "import", "test-laptop", Duration::from_millis(1))
        .await
        .unwrap();
    assert_eq!(grant.key, "cyk_test");
    assert_eq!(fake.lock().unwrap().polls, 4);
}

#[tokio::test]
async fn the_device_flow_reports_denial_and_expiry() {
    for (answer, word) in [("access_denied", "denied"), ("expired_token", "expired")] {
        let fake = Shared::default();
        fake.lock().unwrap().script = vec![err("authorization_pending"), err(answer)];
        let base = serve(fake).await;
        let api = Api::new(&base, None).unwrap();
        let e = login_device(&api, &base, "import", "test-laptop", Duration::from_millis(1))
            .await
            .unwrap_err();
        assert!(e.to_string().contains(word), "{e}");
    }
}
