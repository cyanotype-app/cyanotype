//! Logging for every Cyanotype binary: `telemetry::init("cyanotype-assets")`.
//!
//! Output is one JSON object per line on stdout using Datadog's reserved attributes
//! (`timestamp`, `level`, `message`, `service`, `env`, `version`, `logger.name`), so the
//! Datadog Agent can collect container logs with no parsing rules. See `docs/observability.md`.
//!
//! Environment:
//! - `LOG_FORMAT`: `json` or `pretty` (default: `json` unless stdout is a terminal)
//! - `RUST_LOG`: filter directives (default `info,sqlx=warn`)
//! - `DD_SERVICE` (default: the name passed to `init`), `DD_ENV` (omitted if unset),
//!   `DD_VERSION` (default: the workspace version)
use std::{fmt, io::IsTerminal};

use serde_json::{Map, Value};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use tracing::{
    Event, Subscriber,
    field::{Field, Visit},
};
use tracing_subscriber::{
    EnvFilter, Layer,
    fmt::{
        FmtContext, FormattedFields, MakeWriter,
        format::{FormatEvent, JsonFields, Writer},
    },
    layer::SubscriberExt,
    registry::LookupSpan,
    util::SubscriberInitExt,
};

pub const DEFAULT_FILTER: &str = "info,sqlx=warn";

/// Static attributes stamped on every log line.
#[derive(Debug, Clone)]
pub struct Meta {
    pub service: String,
    pub env: Option<String>,
    pub version: String,
}

impl Meta {
    pub fn from_env(default_service: &str) -> Self {
        let get = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        Self {
            service: get("DD_SERVICE").unwrap_or_else(|| default_service.into()),
            env: get("DD_ENV"),
            version: get("DD_VERSION").unwrap_or_else(|| env!("CARGO_PKG_VERSION").into()),
        }
    }
}

/// Install the global subscriber. Call once, first thing in `main`.
pub fn init(service_name: &str) {
    let meta = Meta::from_env(service_name);
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));
    let json = match std::env::var("LOG_FORMAT").ok().as_deref() {
        Some("json") => true,
        Some("pretty") => false,
        _ => !std::io::stdout().is_terminal(),
    };
    let registry = tracing_subscriber::registry().with(filter);
    let result = if json {
        registry.with(json_layer(meta, std::io::stdout)).try_init()
    } else {
        registry.with(tracing_subscriber::fmt::layer().compact()).try_init()
    };
    // Only fails if a subscriber is already installed (tests); keep the existing one.
    let _ = result;
}

/// JSON layer writing Datadog-shaped lines to `writer`. Public so tests can capture output.
pub fn json_layer<S, W>(meta: Meta, writer: W) -> impl Layer<S>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    W: for<'w> MakeWriter<'w> + 'static,
{
    tracing_subscriber::fmt::layer()
        .fmt_fields(JsonFields::new())
        .event_format(DatadogFormat { meta })
        .with_ansi(false)
        .with_writer(writer)
}

struct DatadogFormat {
    meta: Meta,
}

struct FieldVisitor<'a>(&'a mut Map<String, Value>);

impl Visit for FieldVisitor<'_> {
    fn record_f64(&mut self, f: &Field, v: f64) {
        self.0.insert(f.name().into(), v.into());
    }
    fn record_i64(&mut self, f: &Field, v: i64) {
        self.0.insert(f.name().into(), v.into());
    }
    fn record_u64(&mut self, f: &Field, v: u64) {
        self.0.insert(f.name().into(), v.into());
    }
    fn record_bool(&mut self, f: &Field, v: bool) {
        self.0.insert(f.name().into(), v.into());
    }
    fn record_str(&mut self, f: &Field, v: &str) {
        self.0.insert(f.name().into(), v.into());
    }
    fn record_debug(&mut self, f: &Field, v: &dyn fmt::Debug) {
        self.0.insert(f.name().into(), format!("{v:?}").into());
    }
}

impl<S> FormatEvent<S, JsonFields> for DatadogFormat
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn format_event(&self, ctx: &FmtContext<'_, S, JsonFields>, mut w: Writer<'_>, event: &Event<'_>) -> fmt::Result {
        let mut m = Map::new();
        // Span fields first (root to leaf), then event fields, then reserved attributes,
        // so nothing can spoof `service`, `level`, etc.
        if let Some(scope) = ctx.event_scope() {
            for span in scope.from_root() {
                let ext = span.extensions();
                let parsed = ext
                    .get::<FormattedFields<JsonFields>>()
                    .and_then(|f| serde_json::from_str::<Value>(f.fields.as_str()).ok());
                if let Some(Value::Object(o)) = parsed {
                    m.extend(o);
                }
            }
        }
        event.record(&mut FieldVisitor(&mut m));

        let meta = event.metadata();
        m.insert(
            "timestamp".into(),
            OffsetDateTime::now_utc().format(&Rfc3339).unwrap_or_default().into(),
        );
        m.insert("level".into(), meta.level().as_str().to_ascii_lowercase().into());
        m.entry("message").or_insert_with(|| "".into());
        m.insert("service".into(), self.meta.service.clone().into());
        if let Some(env) = &self.meta.env {
            m.insert("env".into(), env.clone().into());
        }
        m.insert("version".into(), self.meta.version.clone().into());
        m.insert("logger".into(), serde_json::json!({ "name": meta.target() }));
        writeln!(w, "{}", Value::Object(m))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    #[derive(Clone, Default)]
    pub struct Buf(pub Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for Buf {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Buf {
        fn lines(&self) -> Vec<Value> {
            let s = String::from_utf8(self.0.lock().unwrap().clone()).unwrap();
            s.lines().map(|l| serde_json::from_str(l).unwrap()).collect()
        }
    }

    #[test]
    fn json_lines_carry_datadog_reserved_fields() {
        let buf = Buf::default();
        let meta = Meta {
            service: "cyanotype-test".into(),
            env: Some("ci".into()),
            version: "1.2.3".into(),
        };
        let b2 = buf.clone();
        let sub = tracing_subscriber::registry().with(json_layer(meta, move || b2.clone()));
        tracing::subscriber::with_default(sub, || {
            let span = tracing::info_span!("request", request_id = "req-1", method = "GET");
            let _g = span.enter();
            tracing::warn!(job_id = 7, outcome = "retry", "something happened");
        });
        let lines = buf.lines();
        assert_eq!(lines.len(), 1);
        let l = &lines[0];
        assert_eq!(l["level"], "warn");
        assert_eq!(l["message"], "something happened");
        assert_eq!(l["service"], "cyanotype-test");
        assert_eq!(l["env"], "ci");
        assert_eq!(l["version"], "1.2.3");
        assert_eq!(l["logger"]["name"], "cyanotype_core::telemetry::tests");
        assert_eq!(l["request_id"], "req-1");
        assert_eq!(l["method"], "GET");
        assert_eq!(l["job_id"], 7);
        assert_eq!(l["outcome"], "retry");
        let ts = l["timestamp"].as_str().unwrap();
        OffsetDateTime::parse(ts, &Rfc3339).expect("timestamp is RFC 3339");
    }

    #[test]
    fn event_fields_cannot_spoof_reserved_attributes() {
        let buf = Buf::default();
        let meta = Meta {
            service: "real".into(),
            env: None,
            version: "1".into(),
        };
        let b2 = buf.clone();
        let sub = tracing_subscriber::registry().with(json_layer(meta, move || b2.clone()));
        tracing::subscriber::with_default(sub, || tracing::info!(service = "evil", level = "fatal", "hi"));
        let l = &buf.lines()[0];
        assert_eq!(l["service"], "real");
        assert_eq!(l["level"], "info");
        assert!(l.get("env").is_none());
    }
}
