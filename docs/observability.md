# Observability

Every binary logs **one JSON object per line to stdout**, using Datadog's reserved attributes.
The Datadog Agent tails container stdout; application code never talks to Datadog and never
sees `DD_API_KEY`.

```
app (JSON on stdout) -> Docker -> Datadog Agent (logs collection) -> Datadog
```

Implementation: `crates/cyanotype-core/src/telemetry.rs` (`telemetry::init("<binary>")`, called first
in every `main`) and `crates/cyanotype-service/src/http.rs` (request id + request logging for the
axum services).

## Environment variables

| Variable | Default | Meaning |
|---|---|---|
| `LOG_FORMAT` | `json` unless stdout is a TTY | `json` or `pretty` (human-readable, for local dev) |
| `RUST_LOG` | `info,sqlx=warn` | `tracing` `EnvFilter` directives |
| `DD_SERVICE` | the binary name (`cyanotype-auth`, `cyanotype-assets`, `cyanotype-worker`, `cyanotype-cli`) | `service` attribute |
| `DD_ENV` | unset (attribute omitted) | `env` attribute; local compose sets `local`, the VPS override `dev` (set `prod` for a production instance) |
| `DD_VERSION` | workspace crate version | `version` attribute |

`DD_API_KEY` / `DD_SITE` are read only by the Agent container (compose), never by Rust code.

## Log schema

```json
{"timestamp":"2026-10-12T03:50:01.123456789Z","level":"info","message":"request completed",
 "service":"cyanotype-assets","env":"local","version":"0.0.1","logger":{"name":"cyanotype_service::http"},
 "request_id":"2f0c...","method":"GET","path":"/assets/{id}","status":200,"latency_ms":4}
```

Always present: `timestamp` (RFC 3339, UTC), `level` (lowercase: `error|warn|info|debug|trace`),
`message`, `service`, `version`, `logger.name` (the Rust module path). `env` is present when `DD_ENV`
is set. Span and event fields are flattened into the top level. Reserved attributes are written
last, so a log field can never spoof `service`/`level`/etc.

HTTP services (auth, assets), one line per request, inside a `request` span, so every line logged
while handling a request also carries `request_id`, `method` and `path`:

- `request_id`: the incoming `x-request-id` (the Caddy gateway sets one if the client did not),
  generated if missing, echoed on the response.
- `path`: the **route template** (`/assets/{id}`), `<unmatched>` for 404s on unknown routes.
- `status`, `latency_ms`.

Worker: each job runs in a `job` span carrying `job_id`, `kind`, `asset_id`, `attempts`, and
finishes with one summary line with `outcome` (`done` | `skipped` | `retry` | `failed`) and
`duration_ms` (plus `error` on failures, and `reason` when skipped).

## What is deliberately not logged

- Query strings and raw paths (presigned URLs, `?before=`, ids). Only the route template is logged.
- Request/response headers and bodies: `Authorization`, `Cookie`/`Set-Cookie`, session tokens, API keys.
- Emails and passwords (the register/login handlers log nothing from the body).
- Presigned URLs and S3 credentials. `S3Config`/`DatabaseConfig` implement `Debug` by hand and
  redact keys and the DB password; startup logs only the password-redacted DB URL and the bucket name.
- The gateway (Caddy) access log drops the query string and all request/response headers too.

Covered by unit tests (`cyanotype-service::http::tests`, `cyanotype-core::telemetry::tests`,
`cyanotype-core::config::tests`).

## Turning on the Datadog Agent

The Agent is an opt-in compose profile, so a plain `docker compose up` needs no key:

```sh
export DD_API_KEY=...            # required
export DD_SITE=datadoghq.eu      # optional, default datadoghq.com
docker compose --profile datadog up -d --build
```

The Agent runs with `DD_LOGS_ENABLED=true` and `DD_LOGS_CONFIG_CONTAINER_COLLECT_ALL=true`, and mounts
the Docker socket and `/var/lib/docker/containers`. Each app container carries an Autodiscovery label,
e.g. `com.datadoghq.ad.logs: '[{"source":"rust","service":"cyanotype-assets"}]'`, which sets
`source`/`service` for its logs. Because the lines are JSON, Datadog parses them automatically.

Note: the Datadog ingestion path itself has not been verified in this repository (no API key in
CI or in the authoring environment); what is verified is that the compose file is valid with the
profile enabled and that the log lines have the shape above.

## Recommended Datadog setup

Facets: `@request_id`, `@method`, `@path`, `@status` (measure or string), `@latency_ms` (measure,
unit ms), `@job_id`, `@kind`, `@asset_id`, `@outcome`, `@attempts` (measure), `@duration_ms` (measure).

Pipeline (Logs > Configuration > Pipelines, filter `source:rust`):

- Status remapper: `level` (usually automatic). Message remapper: `message`.
- Date remapper: `timestamp`.
- Trace/request correlation: `request_id` is a plain attribute, not a Datadog trace id; keep it as a facet.
- Optional attribute remappers to Datadog's standard HTTP attributes: `method` -> `http.method`,
  `status` -> `http.status_code`, `path` -> `http.url_details.path`, `latency_ms` -> `duration` (needs
  a ms -> ns arithmetic processor).

Useful monitors: error-level log rate per `service`; worker `outcome:failed`; p95 `latency_ms` by `path`.

## Follow-up: tracing (OTLP) not implemented

Optional OTLP trace export to the Agent (behind `OTEL_EXPORTER_OTLP_ENDPOINT`) was skipped: it needs
`opentelemetry`, `opentelemetry-otlp` and `tracing-opentelemetry` (plus a gRPC/HTTP stack) in every
binary, and trace-id injection into logs for correlation. Plan when wanted: add an optional layer in
`telemetry::init` enabled only when the env var is set, emit `dd.trace_id`/`dd.span_id` on log lines,
and enable `DD_OTLP_CONFIG_RECEIVER_PROTOCOLS_GRPC_ENDPOINT` on the Agent.

## Datadog Software Catalog

`service.datadog.yaml` at the repo root defines the catalog entities (schema v2.2, one YAML document
per long-running service: `cyanotype-auth`, `cyanotype-assets`, `cyanotype-worker`, `cyanotype-gateway`;
the one-shot migrate job is left out). Datadog's GitHub integration picks the file up once the repo
is linked to Datadog. Keep each `dd-service` identical to the `DD_SERVICE` / service name the
service actually emits (compose and `telemetry::init`; for the gateway, the Autodiscovery `service`
label), so logs attach to the catalog entry. Do not manage the same entities through both GitHub and
the Datadog API; whichever writes last wins.

## On the VPS

`docker-compose.prod.yml` keeps the Agent behind the `datadog` profile, with `DD_SITE` defaulting to
`us5.datadoghq.com` and `DD_ENV` to `dev`. See `docs/deploy.md`.

## Scoping log collection on a shared host

`DD_LOGS_CONFIG_CONTAINER_COLLECT_ALL=true` would ship every container's logs on the host. The Agent service
therefore also sets `DD_CONTAINER_EXCLUDE_LOGS="name:.*"` and `DD_CONTAINER_INCLUDE_LOGS="name:^cyanotype-.*"`,
so only this project's containers (`cyanotype-<service>-<n>`: postgres, auth, assets, worker, gateway, plus
migrate/backup/the Agent itself) are collected and other projects' containers are never read. The compose
project name is pinned to `cyanotype` (`name:` at the top of `docker-compose.yml`) so the pattern stays
valid whatever the checkout directory is called; if you pass `-p <other>`, change the include pattern.
This scoping was exercised in a live run against Datadog us5 on a host with other containers (using
different container names); it is not exercised in CI.
