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

Worker: each job runs in a `job` span carrying `job.id`, `job.kind`, `job.attempts` and `asset.id` (dotted
attribute names, which Datadog treats as nested attributes), so every line logged while the job runs has them.
It finishes with one summary line with `outcome` (`done` | `skipped` | `retry` | `failed`) and
`duration_ms` (plus `error` on failures, and `reason` when skipped).

## What is deliberately not logged

- Query strings and raw paths (presigned URLs, `?before=`, ids). Only the route template is logged.
- Request/response headers and bodies: `Authorization`, `Cookie`/`Set-Cookie`, session tokens, API keys.
- Emails and passwords (the register/login handlers log nothing from the body).
- Presigned URLs and S3 credentials. `S3Config`/`DatabaseConfig` implement `Debug` by hand and
  redact keys and the DB password; startup logs only the password-redacted DB URL and the bucket name.
- The gateway (Caddy) access log drops the query string and all request/response headers too, and blanks the secret of
  a share link (`/s/<token>`, `/assets/public/s/<token>/...`). The gateway's own error log (an upstream that is down
  answers 502 and Caddy logs the request) gets the same redaction.
- Share tokens and passwords. Share events carry `share_id`; the assets service's `path` is the route template
  (`/assets/public/s/{token}`), never the token.

Covered by unit tests (`cyanotype-service::http::tests`, `cyanotype-core::telemetry::tests`,
`cyanotype-core::config::tests`).

## Metrics (DogStatsD)

The worker and the assets service send metrics over UDP to the Agent's DogStatsD port with a tiny built-in sender
(`crates/cyanotype-core/src/metrics.rs`, no extra dependency). Sending never blocks or fails a job or request: lines go
through a bounded in-memory queue to a background thread, and anything that cannot be sent (no Agent, DNS failure, full
queue) is dropped. **With neither `DOGSTATSD_URL` nor `DD_AGENT_HOST` set, nothing is sent and no thread starts.**

| Variable | Meaning |
|---|---|
| `DOGSTATSD_URL` (or `DD_DOGSTATSD_URL`) | `udp://host:port` of the Agent (compose sets `udp://datadog-agent:8125`; set it empty to turn metrics off). Unix sockets are not supported. |
| `DD_AGENT_HOST`, `DD_DOGSTATSD_PORT` | Alternative: host and port (default 8125). |

Every metric carries `env`, `service` and `version` (the same values as the logs) plus the tags below; the Agent adds
`host`. Counters are sent as DogStatsD counters (query them with `.as_count()`). Durations and sizes are
**histograms**, so the Agent publishes `.avg`, `.median`, `.95percentile`, `.max` and `.count` series without any
configuration in Datadog (distributions would need percentile aggregation enabled per metric).

| Metric | Type | Tags | Meaning |
|---|---|---|---|
| `cyanotype.queue.queued` | gauge | `kind` | Queued jobs (includes delayed retries). |
| `cyanotype.queue.running` | gauge | `kind` | Jobs a worker holds. |
| `cyanotype.queue.failed` | gauge | `kind` | Jobs in state `failed` (cumulative until retried or deleted). |
| `cyanotype.queue.oldest_queued_age_seconds` | gauge | `kind` | Wait of the oldest runnable queued job; 0 when none. |
| `cyanotype.queue.stalled_seconds` | gauge | | Runnable work exists and nothing finished: the lesser of the oldest wait and the time since any success; 0 when idle. |
| `cyanotype.integrity.findings_open` | gauge | `kind` (`missing_original`, `untracked_object`, `checksum_mismatch`) | Open findings of the storage integrity check (docs/admin.md), sampled once a minute by the worker; acknowledged ones are not counted. |
| `cyanotype.integrity.last_run_age_seconds` | gauge | | Seconds since the last integrity check finished. |
| `cyanotype.integrity.runs` | count | `result` (`done`, `failed`) | Integrity checks run. |
| `cyanotype.integrity.bytes_hashed` / `.derived_requeued` | count | | Originals re-read for the checksum sample, and thumbnails queued again, per run. |
| `cyanotype.integrity.run.duration_seconds` | histogram | | Wall time of a check. |
| `cyanotype.jobs.completed` | count | `kind`, `family`, `outcome` (`done`, `skipped`) | Jobs finished. |
| `cyanotype.jobs.failed` | count | `kind`, `family`, `outcome:failed`, `permanent` | Jobs that failed for good (use `.as_count()` sums for rates). |
| `cyanotype.jobs.retried` | count | `kind`, `family`, `outcome:retry` | Failures that will be retried. |
| `cyanotype.jobs.duration` | histogram (ms) | `kind`, `family`, `outcome` | Wall time per job. |
| `cyanotype.jobs.decoded_megapixels` | histogram | `kind`, `family` | Size of the image a thumbnail job decoded. |
| `cyanotype.jobs.decode_path` | count | `path` (`jpeg_scaled`, `png_streamed`, `heif_tiled`, `heif`, `raw`, `full`), `family` | Which decoder a thumbnail job used. `full` is the plain full-size decode; a rising share for big JPEGs/HEICs means the reduced paths are being skipped. |
| `cyanotype.jobs.decode_fallbacks` | count | `family` | A reduced-size decoder (scaled JPEG, streamed PNG) failed on a file and the job retried with a full decode. Should be near zero. |
| `cyanotype.worker.memory_budget.waits` / `.wait_ms` | count / histogram | | A reservation had to wait for memory held by other jobs, and for how long. |
| `cyanotype.worker.memory_budget.refusals` | count | `reason` (`over_size_limit`, `over_budget`, `decode_refused`), `family` | Jobs turned away as too big. |
| `cyanotype.worker.memory_budget.used_bytes` / `.total_bytes` | gauge | | Budget in use and its size (`WORKER_MEMORY_BUDGET_MB`). |
| `cyanotype.embed.model_loaded` | gauge | | 1 while the image model is in memory (loaded by the first embed job, dropped after five quiet minutes). |
| `cyanotype.worker.rss_bytes` | gauge | | Resident set of the worker process, to read against its container limit (`container.memory.usage` is the cgroup's view). |
| `cyanotype.embed.duration` | histogram (ms) | `model` | Wall time per photo embedded for search (download of the preview, decode, preprocessing, inference, store), averaged over the batch. |
| `cyanotype.embed.batch_size` | histogram | `model` | Photos taken together (at most 4; their previews download concurrently, the model runs over them one at a time). Near 4 while a backlog drains, 1 for a trickle of uploads. |
| `cyanotype.faces.model_loaded` | gauge | | 1 while the face models are in memory (only with `FACES_ENABLED`; loaded by the first `faces` job, dropped after five quiet minutes). |
| `cyanotype.faces.duration` | histogram (ms) | `model` | Wall time per photo scanned for faces (download of the preview, decode, detection, fingerprints, store). |
| `cyanotype.faces.per_photo` | histogram | `model` | Faces found per photo scanned. |
| `cyanotype.faces.model_load_ms` | histogram (ms) | `model` | Time to load the face models (once per load). |
| `cyanotype.people.run_ms` | histogram (ms) | | Wall time of one grouping of an owner's faces into people (`docs/faces.md`, "People"). |
| `cyanotype.people.assigned` / `.created` | count | | Faces that joined a known person, and new unnamed people, per run. |
| `cyanotype.people.ungrouped_faces` | gauge | | Faces the last run left without a person (too few alike, or not alike enough). |
| `cyanotype.pets.model_loaded` | gauge | | 1 while the pet detector is in memory (unless `PETS_ENABLED=0`; loaded by the first `pets` job that needs it, dropped after five quiet minutes). Most photos never need it: see `pets.checked`. |
| `cyanotype.pets.checked` | count | `verdict` (`pet`, `none`) | Photos the `pets` job read the search embedding of (`docs/pets.md`, "The check"): `pet` for a photo that holds a dog or a cat, which goes on to the detector, `none` for the rest, which are done there. |
| `cyanotype.pets.duration` | histogram (ms) | `model` | Wall time per photo of the whole `pets` job (the check; and for a photo with a pet the download of the preview, decode, detection and store). |
| `cyanotype.pets.detector_ms` | histogram (ms) | `model` | Wall time of the detector alone (decode of the preview and inference), per photo that needed it. |
| `cyanotype.pets.fingerprint_ms` | histogram (ms) | `model` (the fingerprint model) | Wall time to fingerprint the animals of one photo (crop, preprocessing and the image tower), in the job that finds them and in the one that fills in animals found before named pets (`docs/pets.md`, "Telling individuals apart"). |
| `cyanotype.pets.per_photo` | histogram | `model` | Animals kept per photo scanned (0 for most). |
| `cyanotype.pets.found` | count | `kind` (`dog`, `cat`), `model` | Animals found, by kind. |
| `cyanotype.pets.unlocated` | count | `model` | Photos the check said hold a dog or a cat and the detector found no box in (stored as having no pets; 2 % of the flagged photos on the evaluation set). A rising share means the two have drifted apart. |
| `cyanotype.pets.model_load_ms` | histogram (ms) | `model` | Time to load the detector (once per load). |
| `cyanotype.search.queries` | count | `mode` (`text`, `text_filters`, `filters`, `similar`), `plan` (`exact`, `hnsw`, `timeline`, `none`), `outcome` (`ok`, `bad_request`, `unavailable`, `error`; `similar` also `not_found`, `not_indexed`) | Searches served by the assets service. `unavailable` is a deployment without the model files. `mode:similar` is "More like this" (`GET /assets/{id}/similar`); its `not_indexed` outcome is the photo having no vector yet for the serving model (HTTP 409). |
| `cyanotype.search.latency_ms` | histogram (ms) | same as `search.queries` | Time to answer a search: query embedding, ranking and building the page (for `mode:similar` there is no query embedding). |
| `cyanotype.search.model` | count | `model` (`siglip2-b16`, `clip-b32`) | Ranked searches by the model that answered them: during a model change the old model's count falls as the new one's rises, and the day it reaches zero the previous model's text tower is no longer needed. |
| `cyanotype.search.no_strong_matches` | count | `weaker` (`yes`: weaker matches exist, `no`) | Ranked searches whose first page had no strong match: the "No close matches" / "Nothing matched" screens. A rising share after a change of the cut-off means it is too strict. |
| `cyanotype.bursts.regroup_ms` | histogram (ms) | | Wall time of one `burst` job (the regroup of a photo's neighbourhood). |
| `cyanotype.bursts.window_photos` | histogram | | Photos of the camera in the window a regroup looked at: usually a handful, large for a library with thousands of photos a minute. |
| `cyanotype.bursts.photos_changed` | count | | Photos whose burst or computed mark a regroup wrote (a backfill shows as a long run of this; a steady state is near the upload rate). |
| `cyanotype.usage.reconcile.drifted_users` | gauge | | Accounts whose maintained storage counters differed from the files at the last nightly reconcile (0 is healthy; any other value means a code path changed `assets` without the trigger seeing it, which is a bug to find). |
| `cyanotype.usage.reconcile.drift_bytes` | gauge | | Bytes those counters were off by, summed over accounts. |
| `cyanotype.usage.bytes` | gauge | `kind` (`originals`, `trash`, `derived`) | Server totals of what accounts store, sampled at each reconcile. |
| `cyanotype.usage.photos` | gauge | | Photos in all libraries, at the same time. |
| `cyanotype.uploads.refused` | count | `reason` (`quota_exceeded`, `daily_cap_exceeded`, `file_too_large`, `file_exceeds_daily_cap`, `larger_than_declared`) | Uploads an account's limits refused (docs/admin.md, "Limits"). A steady trickle is people at their limits; a burst from one client is worth a look. |
| `cyanotype.uploads.completed` / `.bytes` | count | `family` | Uploads completed (browser and CLI) and their bytes. |
| `cyanotype.uploads.parts` | count | `outcome` (`ok`, `timeout` = HTTP 408, `error`), `reason` on failures (`client_aborted` = the body broke mid-part, `body_too_short`, `timeout`, `storage_error`, `other`) | Proxied (browser) part uploads. A failed part is also logged at warn (`part upload failed`: `upload_id`, `part`, `reason`, `bytes_received`, `bytes_expected`, `elapsed_ms`; never the file name, owner or a URL). |
| `cyanotype.uploads.part_latency_ms` | histogram | | Time to receive a part from the browser and relay it to the bucket (successful parts). |
| `cyanotype.uploads.part_bytes` | count | | Bytes relayed. |
| `cyanotype.shares.views` | count | `kind` (`album`, `selection`) | Share link opened (the public page's info request, once a password link is unlocked). Logs carry `share_id`, never the token. |
| `cyanotype.shares.downloads` | count | `kind` | Download redirects issued by a share link, and "Download all" archives. |
| `cyanotype.shares.zip_downloads` | count | `kind` | "Download all" archives started by a share link. |
| `cyanotype.assets.zip_downloads` | count | `source` (`album`, `scope`, `ids`) | ZIP archives of an album or a selection started by their owner. |

Embedding photos for search is background work and reads as such: the `embed` kind has queue gauges and job metrics like any
other (`kind:embed`), its backlog is hours long after a first deploy or a model swap, and the "oldest queued job" monitor
leaves `kind:embed` out for that reason. Grouping photos into bursts (`kind:burst`, database only, after a photo is embedded and when one is
trashed, restored or paired) is the same kind of background work and is left out of that monitor too; the dashboard shows its queue and
regroup time. The dashboard's Search section shows the backlog, photos embedded per minute, time
per photo, query rate and latency by plan, and the assets and worker container memory.

`family` is `jpeg`, `heic`, `raw`, `png` or `other` (`unknown` when the asset row is gone), from the content type and
file extension (`metrics::family`). The job gauges are sampled every 15 s by the worker, one reading per kind every time
(zeros included, so a drained queue reads 0 rather than going stale); run one worker process, or accept duplicate
(identical) readings.

Container metrics (`container.memory.usage`, `container.memory.oom_events`, `container.uptime`, `container.cpu.usage`,
`docker.*`) come from the Agent itself (Docker socket and cgroup mounts in compose) and are scoped, like the logs, to
`cyanotype-*` containers (`DD_CONTAINER_INCLUDE_METRICS`).

### Is the Agent reachable from the services?

`docker-compose.yml` gives the Agent `DD_DOGSTATSD_NON_LOCAL_TRAFFIC=true` (listen on all interfaces; the port is not
published, so only containers on the compose network can send) and points the worker and assets service at
`udp://datadog-agent:8125`. The Agent is still opt-in (`--profile datadog`, which `just deploy` and the Deploy workflow
add when `DD_API_KEY` is set); without it the name does not resolve, the sender logs one warning and drops its metrics.
After deploying this version, recreate the stack with the profile on (`dc --profile datadog up -d`) so the Agent
restarts with the new setting.

## Dashboard and monitors as code

`infra/datadog/dashboard.json` and `infra/datadog/monitors/*.json` are plain Datadog API payloads. Nothing is created
automatically; import them yourself:

- **Dashboard, UI:** Dashboards > New Dashboard > (gear) Import dashboard JSON, paste the file. Set the `env` variable
  at the top to `dev`.
- **Dashboard, API:** `curl -X POST "https://api.us5.datadoghq.com/api/v1/dashboard" -H "DD-API-KEY: $DD_API_KEY" -H
  "DD-APPLICATION-KEY: $DD_APP_KEY" -H "Content-Type: application/json" -d @infra/datadog/dashboard.json`
  (an application key with dashboards_write; the site is us5).
- **Monitors, UI:** Monitors > New Monitor > Import from JSON, one file at a time.
- **Monitors, API:** `for f in infra/datadog/monitors/*.json; do curl -X POST "https://api.us5.datadoghq.com/api/v1/monitor" -H "DD-API-KEY: $DD_API_KEY" -H "DD-APPLICATION-KEY: $DD_APP_KEY" -H "Content-Type: application/json" -d @$f; done`
  (application key with monitors_write). Re-running creates duplicates; update existing ones with `PUT /api/v1/monitor/{id}`.

Monitors: job failures spike (more than 25 in 15 min), oldest queued job older than 30 min, worker container
crash-looping (uptime under 2 min for 15 min), worker OOM kill, no job completing for 30 min while work is queued, and
upload parts failing (warning above 5% of parts over 30 min, critical above 25%; the total is floored at 20 parts so a few
parts cannot trip it).
They have no notification handle: add `@you@example.com` (or a channel) to each `message` before importing. The queries
use `{*}`; add `env:dev` inside the braces to scope them. The query syntax of all six was checked with Datadog's
monitor validator; the metric names do not exist in Datadog until the first deploy that emits them, so a new monitor
may show "no data" until then (they are set not to alert on no data).

## Watching an import

1. Open the dashboard (env `dev`). **Queue depth by kind** should climb as uploads finish and fall as the worker drains
   it; **Jobs completed per minute** is the throughput (an estimate of time left is queued / that rate).
2. **Oldest queued job age** staying under a few minutes means the worker keeps up; a long queue with steady
   throughput during a 50 GB import is normal. **Stalled seconds** rising means nothing is finishing: check the worker.
3. **Memory budget waits** and **worker container memory** show whether the 640 MB decode budget (not CPU) is the limit;
   **refusals** are RAW/HEIC/JPEG files too large for the budget (they fail permanently and show in the web app's
   failures panel).
4. **Failed jobs by kind** and the **Worker errors** log stream show why; in Logs, `service:cyanotype-worker
   outcome:failed` with `@error`, `@asset.id`, `@job.kind`.
5. The web app shows the same progress to the user (the banner under the Library tabs, [`library.md`](library.md)),
   with a Retry button for failed jobs once the cause is fixed.

## Instant job pickup (LISTEN/NOTIFY)

A trigger on `jobs` (migration 0007) sends `pg_notify('jobs_queued', kind)` whenever a job becomes queued: an insert
(upload complete, the pairing backfill, `reindex --rethumb`, ad-hoc SQL) or a transition back to `queued` (a retry
after a failure, the retry endpoint). The worker holds one dedicated `LISTEN` connection (a `PgListener`, not a
connection from the pool, so it cannot starve it) and wakes immediately. It still polls every 30 s as a safety net and
for stale-job reclaim, sleeps exactly until the earliest delayed retry is due, and re-checks after every listener
reconnect (notifications sent while disconnected are lost). Notifications are delivered only after the enqueuing
transaction commits.

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
unit ms), `@job.id`, `@job.kind`, `@asset.id`, `@outcome`, `@job.attempts` (measure), `@duration_ms` (measure).

Pipeline (Logs > Configuration > Pipelines, filter `source:rust`):

- Status remapper: `level` (usually automatic). Message remapper: `message`.
- Date remapper: `timestamp`.
- Trace/request correlation: `request_id` is a plain attribute, not a Datadog trace id; keep it as a facet.
- Optional attribute remappers to Datadog's standard HTTP attributes: `method` -> `http.method`,
  `status` -> `http.status_code`, `path` -> `http.url_details.path`, `latency_ms` -> `duration` (needs
  a ms -> ns arithmetic processor).

Useful monitors beyond the committed ones: error-level log rate per `service`; p95 `latency_ms` by `path`.

## Follow-up: tracing (OTLP) not implemented

Optional OTLP trace export to the Agent (behind `OTEL_EXPORTER_OTLP_ENDPOINT`) was skipped: it needs
`opentelemetry`, `opentelemetry-otlp` and `tracing-opentelemetry` (plus a gRPC/HTTP stack) in every
binary, and trace-id injection into logs for correlation. Plan when wanted: add an optional layer in
`telemetry::init` enabled only when the env var is set, emit `dd.trace_id`/`dd.span_id` on log lines,
and enable `DD_OTLP_CONFIG_RECEIVER_PROTOCOLS_GRPC_ENDPOINT` on the Agent.

## Datadog Software Catalog

`service.datadog.yaml` at the repo root defines the catalog entities (schema v2.2, one YAML document
per long-running service: `cyanotype-auth`, `cyanotype-assets`, `cyanotype-worker`, `cyanotype-gateway`, `cyanotype-web`;
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
so only this project's containers (`cyanotype-<service>-<n>`: postgres, auth, assets, worker, web, gateway, plus
migrate/backup/the Agent itself) are collected and other projects' containers are never read. The compose
project name is pinned to `cyanotype` (`name:` at the top of `docker-compose.yml`) so the pattern stays
valid whatever the checkout directory is called; if you pass `-p <other>`, change the include pattern.
This scoping was exercised in a live run against Datadog us5 on a host with other containers (using
different container names); it is not exercised in CI.
