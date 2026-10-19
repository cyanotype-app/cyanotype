//! Request-id propagation, request logging and process lifecycle for axum services.
use std::time::Duration;

use anyhow::Result;
use axum::{
    Json, Router,
    body::Body,
    extract::MatchedPath,
    http::{Request, Response},
};
use serde_json::{Value, json};
use tower_http::{
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    trace::TraceLayer,
};
use tracing::Span;

pub const REQUEST_ID_HEADER: &str = "x-request-id";

pub async fn healthz() -> Json<Value> {
    Json(json!({"ok": true}))
}

/// Wrap `router` with request-id handling and structured request logging.
///
/// An incoming `x-request-id` is kept, otherwise one is generated; it is echoed on the
/// response and attached to every log line emitted while handling the request.
///
/// Logged per request: method, route *template* (`/assets/{id}`, never the raw path or query
/// string), status, latency. Headers (Authorization, Cookie), query strings, bodies and
/// presigned URLs are never logged.
pub fn with_http_layers<S: Clone + Send + Sync + 'static>(router: Router<S>) -> Router<S> {
    let trace = TraceLayer::new_for_http()
        .make_span_with(|req: &Request<Body>| {
            let path = req
                .extensions()
                .get::<MatchedPath>()
                .map(MatchedPath::as_str)
                .unwrap_or("<unmatched>");
            let request_id = req
                .headers()
                .get(REQUEST_ID_HEADER)
                .and_then(|v| v.to_str().ok())
                .map(|v| v.chars().take(64).collect::<String>())
                .unwrap_or_default();
            tracing::info_span!("request", request_id = %request_id, method = %req.method(), path = %path)
        })
        .on_request(())
        .on_failure(())
        .on_response(|res: &Response<Body>, latency: Duration, _span: &Span| {
            let status = res.status().as_u16();
            let latency_ms = latency.as_millis() as u64;
            if status >= 500 {
                tracing::warn!(status, latency_ms, "request completed");
            } else {
                tracing::info!(status, latency_ms, "request completed");
            }
        });
    // Last `.layer` is outermost: the id is set before the trace span reads it.
    router
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(trace)
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
}

/// Bind `addr` and serve until SIGTERM / Ctrl-C.
pub async fn serve(app: Router, addr: &str) -> Result<()> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(addr, "listening");
    // Connect info lets handlers see the peer address (used by the auth rate limiter).
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;
    tracing::info!("shut down");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! { _ = ctrl_c => {}, _ = term => {} }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::{http::StatusCode, routing::get};
    use cyanotype_core::telemetry::{Meta, json_layer};
    use tower::ServiceExt;
    use tracing_subscriber::layer::SubscriberExt;

    use super::*;

    #[derive(Clone, Default)]
    struct Buf(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for Buf {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn app() -> Router {
        with_http_layers(Router::new().route("/assets/{id}", get(|| async { "ok" })))
    }

    fn capture() -> (Buf, tracing::subscriber::DefaultGuard) {
        let buf = Buf::default();
        let b2 = buf.clone();
        let meta = Meta {
            service: "t".into(),
            env: None,
            version: "0".into(),
        };
        let sub = tracing_subscriber::registry().with(json_layer(meta, move || b2.clone()));
        (buf, tracing::subscriber::set_default(sub))
    }

    #[tokio::test]
    async fn logs_template_path_and_never_sensitive_data() {
        let (buf, _g) = capture();
        let req = Request::builder()
            .uri("/assets/0b6e?token=QUERYSECRET&X-Amz-Signature=SIGSECRET&email=a@b.co")
            .header("authorization", "Bearer BEARERSECRET")
            .header("cookie", "cyanotype_session=COOKIESECRET")
            .header("x-request-id", "req-abc")
            .body(Body::empty())
            .unwrap();
        let res = app().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers()["x-request-id"], "req-abc");

        let out = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
        for (i, secret) in [
            "QUERYSECRET",
            "SIGSECRET",
            "BEARERSECRET",
            "COOKIESECRET",
            "a@b.co",
            "0b6e",
        ]
        .into_iter()
        .enumerate()
        {
            // Report which planted value leaked by position, not the value itself.
            assert!(!out.contains(secret), "log leaked planted value #{i}");
        }
        let line: Value = serde_json::from_str(out.lines().last().unwrap()).unwrap();
        assert_eq!(line["path"], "/assets/{id}");
        assert_eq!(line["method"], "GET");
        assert_eq!(line["status"], 200);
        assert_eq!(line["request_id"], "req-abc");
        assert!(line["latency_ms"].is_u64());
    }

    #[tokio::test]
    async fn generates_request_id_when_absent() {
        let (buf, _g) = capture();
        let req = Request::builder().uri("/assets/1").body(Body::empty()).unwrap();
        let res = app().oneshot(req).await.unwrap();
        let id = res.headers()["x-request-id"].to_str().unwrap().to_string();
        assert!(!id.is_empty());
        let out = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
        let line: Value = serde_json::from_str(out.lines().last().unwrap()).unwrap();
        assert_eq!(line["request_id"], id);
    }

    #[tokio::test]
    async fn unmatched_routes_do_not_log_the_raw_path() {
        let (buf, _g) = capture();
        let req = Request::builder().uri("/nope/SECRETPATH").body(Body::empty()).unwrap();
        let res = app().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
        let out = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
        assert!(!out.contains("SECRETPATH"), "{out}");
    }
}
