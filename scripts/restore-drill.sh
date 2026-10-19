#!/usr/bin/env bash
# Restore drill: prove that both recovery paths of docs/recovery.md work, and show exactly what each one loses.
#
#   scripts/restore-drill.sh [--keep] [--keep-images] [--no-build] [--report FILE] [--title TEXT]
#   scripts/restore-drill.sh --source real        # read-only drill against the real bucket (see below; never run by CI)
#
# Synthetic mode (the default, fully local, nothing real is touched):
#   1. brings up a throwaway compose project (cyano-drill: Postgres :54397, S3 :18397, gateway :3397; own volumes),
#      built from this checkout's Dockerfile and infra/backup, the images production runs;
#   2. seeds a library through the real APIs (admin + member, photos with EXIF, a burst, a RAW+JPEG pair, albums, a share
#      link, favorites, a trashed photo, edits, an invite, limits, API keys) and waits for the worker;
#   3. writes the daily manifests (as the assets service does every night), takes the canonical snapshot (per-table row
#      hashes + API responses), runs the real backup job, takes a bucket listing;
#   4. path A: drops the database volume, restores the dump (infra/backup, as docs/deploy.md says), migrates, starts,
#      snapshots;
#   5. path B: drops the database again, `cyanotype migrate` + `cyanotype reindex` from the bucket alone, the worker
#      catches up, snapshots (the documented extra steps, e.g. password reset links, are part of the timing);
#   6. diffs both snapshots against the first and writes a markdown report (stdout and --report FILE);
#   7. tears everything down (containers, volumes, the drill's own images) unless --keep.
#
# Real mode (--source real) restores the latest REAL Postgres dump into a local throwaway Postgres and compares it with what
# `cyanotype reindex` would rebuild from the REAL bucket, strictly read-only. It needs a READ-ONLY access key:
#   DRILL_S3_ENDPOINT  DRILL_S3_BUCKET  DRILL_S3_ACCESS_KEY_ID  DRILL_S3_SECRET_ACCESS_KEY
#   (optional: DRILL_S3_REGION, DRILL_S3_FORCE_PATH_STYLE=true|false, DRILL_S3_BACKUP_PREFIX=backups/postgres)
# It refuses to run if that key can write (it tries to PUT a tiny object under drill-probe/ and requires a denial), if less
# than 10 GB of disk is free, or if the variables are missing. Only the dump and listings are downloaded: reindex reads
# no original (listings and HEADs), and its container has no writable volume. It never contacts the live server and never
# starts the services or the worker. How to make the key: docs/recovery.md, "Real-data drill".
#
# Needs: docker compose, python3, curl. Credentials of the throwaway stack are generated per run and never printed.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/.." && pwd)"
helper="$here/restore-drill/drill.py"
compose_file="$repo/infra/drill/docker-compose.yml"

PROJECT=cyano-drill
export DRILL_PG_PORT=54397 DRILL_LOCAL_S3_PORT=18397 DRILL_GATEWAY_PORT=3397
IMAGES=(cyano-drill-cli:drill cyano-drill-auth:drill cyano-drill-assets:drill cyano-drill-worker:drill cyano-drill-backup:drill)

SOURCE=synthetic KEEP=0 KEEP_IMAGES=0 NO_BUILD=0 REPORT="" TITLE="" SETTLE_TIMEOUT="${DRILL_SETTLE_TIMEOUT:-1500}"
usage() { sed -n '2,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'; }
while [ $# -gt 0 ]; do
  case "$1" in
    --source) SOURCE="${2:?--source needs synthetic or real}"; shift ;;
    --keep) KEEP=1 ;;
    --keep-images) KEEP_IMAGES=1 ;;
    --no-build) NO_BUILD=1 ;;
    --report) REPORT="${2:?--report needs a file}"; shift ;;
    --title) TITLE="${2:?--title needs text}"; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown option: $1 (try --help)" >&2; exit 2 ;;
  esac
  shift
done
case "$SOURCE" in synthetic|real) ;; *) echo "--source must be synthetic or real" >&2; exit 2 ;; esac

log() { printf '\n==> %s\n' "$*" >&2; }
die() { printf 'drill: %s\n' "$*" >&2; exit 1; }

# Every docker compose call goes through this: always the drill's own project and file, never the default project.
dc() { docker compose -p "$PROJECT" -f "$compose_file" "$@"; }

free_gb() { df -Pk "$repo" | awk 'NR==2 { print int($4 / 1048576) }'; }
min_gb=5; [ "$SOURCE" = real ] && min_gb=10
[ "$(free_gb)" -ge "$min_gb" ] || die "only $(free_gb) GB free on the disk of $repo; this mode needs at least $min_gb GB. Aborting."

command -v docker >/dev/null || die "docker is required"
command -v python3 >/dev/null || die "python3 is required"
command -v curl >/dev/null || die "curl is required"

work="$(mktemp -d "${TMPDIR:-/tmp}/cyano-drill.XXXXXX")"
timings="$work/timings.tsv"
: > "$timings"
[ -n "$REPORT" ] || REPORT="$repo/.restore-drill/report.md"
mkdir -p "$(dirname "$REPORT")"

teardown() {
  local rc=$?
  trap - EXIT
  if [ "$KEEP" = 1 ]; then
    log "--keep: leaving the project '$PROJECT' running (ports $DRILL_PG_PORT/$DRILL_LOCAL_S3_PORT/$DRILL_GATEWAY_PORT) and $work"
    log "remove it with: docker compose -p $PROJECT -f $compose_file --profile tools --profile backup --profile real down -v"
  else
    log "tearing down (containers, volumes, the drill's own images)"
    dc --profile tools --profile backup --profile real down -v --remove-orphans --timeout 5 >/dev/null 2>&1 || true
    docker volume rm -f "${PROJECT}_pgdata" "${PROJECT}_seaweeddata" >/dev/null 2>&1 || true
    if [ "$KEEP_IMAGES" = 0 ]; then docker rmi "${IMAGES[@]}" >/dev/null 2>&1 || true; fi
    rm -rf "$work"
  fi
  exit "$rc"
}
trap teardown EXIT

timed() { # timed "label" cmd...: runs the command and records its wall time
  local name="$1" t0 t1
  shift
  t0="$(date +%s.%N)"
  "$@"
  t1="$(date +%s.%N)"
  awk -v n="$name" -v a="$t0" -v b="$t1" 'BEGIN { printf "%s\t%.1f\n", n, b - a }' >> "$timings"
}

# A previous aborted run of THIS project (only this project) is removed first, then the ports must be free.
# DRILL_REUSE=1 (tests of this script) keeps a stack that --keep left running instead.
if [ "${DRILL_REUSE:-0}" != 1 ]; then
  DRILL_LOCAL_S3_ACCESS_KEY=x DRILL_LOCAL_S3_SECRET_KEY=x DRILL_LOCAL_S3_RO_ACCESS_KEY=x DRILL_LOCAL_S3_RO_SECRET_KEY=x \
    dc --profile tools --profile backup --profile real down -v --remove-orphans --timeout 5 >/dev/null 2>&1 || true
  for p in "$DRILL_PG_PORT" "$DRILL_LOCAL_S3_PORT" "$DRILL_GATEWAY_PORT"; do
    if (exec 3<>"/dev/tcp/127.0.0.1/$p") 2>/dev/null; then die "port $p is in use by something that is not this drill; free it and run again"; fi
  done
fi

rand() { python3 -c 'import secrets,sys; print(sys.argv[1] + secrets.token_hex(10))' "$1"; }
export DRILL_LOCAL_S3_ACCESS_KEY="${DRILL_LOCAL_S3_ACCESS_KEY:-$(rand drill)}" DRILL_LOCAL_S3_SECRET_KEY="${DRILL_LOCAL_S3_SECRET_KEY:-$(rand secret)}"
export DRILL_LOCAL_S3_RO_ACCESS_KEY="${DRILL_LOCAL_S3_RO_ACCESS_KEY:-$(rand drillro)}" DRILL_LOCAL_S3_RO_SECRET_KEY="${DRILL_LOCAL_S3_RO_SECRET_KEY:-$(rand secretro)}"
export DRILL_LOCAL_S3_URL="http://127.0.0.1:$DRILL_LOCAL_S3_PORT"
export DRILL_BASE_URL="http://127.0.0.1:$DRILL_GATEWAY_PORT"
export DRILL_PSQL="docker compose -p $PROJECT -f $compose_file exec -T postgres psql -U cyanotype -d cyanotype"
state="$work/state.json"

build_images() {
  if [ "$NO_BUILD" = 1 ] && docker image inspect "${IMAGES[@]}" >/dev/null 2>&1; then log "images exist (--no-build)"; return; fi
  log "building the images (Dockerfile targets auth, assets, worker, cli; infra/backup)"
  timed "build images" dc --profile tools --profile backup --profile real build
}

wait_gateway() {
  local i
  for i in $(seq 1 90); do
    if curl -fsS "$DRILL_BASE_URL/healthz" >/dev/null 2>&1 && curl -fsS "$DRILL_BASE_URL/auth/registration" >/dev/null 2>&1; then return 0; fi
    sleep 1
  done
  die "the gateway did not become ready"
}
start_infra() { dc up -d --wait postgres seaweedfs; dc run --rm -T seaweedfs-init >/dev/null; }
migrate_db() { dc run --rm -T migrate >/dev/null; }
start_services() { dc up -d --wait auth assets worker gateway; wait_gateway; }
stop_services() { dc stop -t 5 gateway auth assets worker >/dev/null; }
drop_db() { dc stop -t 5 postgres >/dev/null; dc rm -f postgres >/dev/null; docker volume rm -f "${PROJECT}_pgdata" >/dev/null; dc up -d --wait postgres; }
# reindex_to FILE [flags]: the report (last line of stdout, JSON) goes to FILE; the logs are not kept.
reindex_to() { local out="$1"; shift; dc --profile tools run --rm -T cli reindex --min-age-secs 0 "$@" --json | tail -n 1 > "$out"; }
reindex_real_to() { local out="$1"; shift; dc --profile real run --rm -T cli-real reindex --min-age-secs "${DRILL_MIN_AGE_SECS:-600}" "$@" --json | tail -n 1 > "$out"; }
jget() { python3 -c 'import json,sys; d=json.loads(sys.argv[1]); print(d.get(sys.argv[2], 0))' "$1" "$2"; }
settle() { python3 "$helper" settle --state "$state" --timeout "$SETTLE_TIMEOUT" || log "WARNING: the worker had not finished; the snapshot is of what exists"; }
snap() { python3 "$helper" snapshot --state "$state" --out "$work/$1.json" --bucket "$work/$1.bucket.json"; }
bucket_snap() { python3 "$helper" bucket --env DRILL_LOCAL_S3 --out "$work/$1.bucket.json"; }

# ------------------------------------------------------------------------------------------------ real-data mode

run_real() {
  local v
  for v in DRILL_S3_ENDPOINT DRILL_S3_BUCKET DRILL_S3_ACCESS_KEY_ID DRILL_S3_SECRET_ACCESS_KEY; do
    [ -n "${!v:-}" ] || die "$v is not set (real mode needs a READ-ONLY key; see docs/recovery.md, 'Real-data drill')"
  done
  export DRILL_S3_ENDPOINT DRILL_S3_BUCKET DRILL_S3_ACCESS_KEY_ID DRILL_S3_SECRET_ACCESS_KEY
  case "$DRILL_S3_ENDPOINT" in http://*|https://*) ;; *) die "DRILL_S3_ENDPOINT must start with http:// or https://" ;; esac
  log "disk: $(free_gb) GB free (real mode aborts below 10 GB)"

  log "guard: the key must be READ-ONLY (a PUT of a tiny object under drill-probe/ must be denied)"
  python3 "$helper" probe --env DRILL_S3 || die "the read-only guard refused this key (see above). Nothing was restored or read."

  build_images
  log "starting the local scratch Postgres (nothing else: no services, no worker, no local bucket)"
  dc up -d --wait postgres

  log "restoring the newest real dump into the local scratch database (read-only download of the dump)"
  dc --profile real run --rm -T backup-real list | tail -n 3 | sed 's/^/    dump: /' >&2
  timed "real: restore the newest dump" dc --profile real run --rm -T -e CONFIRM=yes backup-real restore
  timed "real: migrate" dc run --rm -T migrate >/dev/null
  python3 "$helper" snapshot --no-api --out "$work/real_dump.json"

  log "reindex DRY RUN against the real bucket (reads only; compares the restored dump with the bucket)"
  timed "real: reindex dry run" reindex_real_to "$work/real_dry.json"

  log "path B rehearsal: rebuild a SECOND local database from the bucket alone (reindex --apply writes only that local database)"
  dc exec -T postgres dropdb -U cyanotype --if-exists cyanotype_rebuild
  dc exec -T postgres createdb -U cyanotype cyanotype_rebuild
  export DRILL_DATABASE_URL="postgres://cyanotype:cyanotype@postgres:5432/cyanotype_rebuild"
  dc run --rm -T -e DATABASE_URL="$DRILL_DATABASE_URL" migrate >/dev/null
  timed "real: reindex --apply into the scratch rebuild" reindex_real_to "$work/real_apply.json" --apply
  DRILL_PSQL="docker compose -p $PROJECT -f $compose_file exec -T postgres psql -U cyanotype -d cyanotype_rebuild" \
    python3 "$helper" snapshot --no-api --out "$work/real_rebuild.json"

  python3 "$helper" report-real --dump "$work/real_dump.json" --rebuild "$work/real_rebuild.json" --dry "$work/real_dry.json" \
    --apply "$work/real_apply.json" --timings "$timings" --out "$REPORT" --commit "$(git -C "$repo" rev-parse --short HEAD 2>/dev/null || echo unknown)"
  log "report written to $REPORT (it describes a real library: do not commit it)"
}

if [ "$SOURCE" = real ]; then
  run_real
  exit 0
fi

# ------------------------------------------------------------------------------------------------ synthetic drill

build_images
log "starting the throwaway stack (project $PROJECT)"
timed "start infrastructure (Postgres, bucket)" start_infra
migrate_db
timed "start services (auth, assets, worker, gateway)" start_services

log "seeding a realistic library through the real APIs"
raw_cache="${DRILL_RAW_CACHE:-$repo/models/eval/raw}"
timed "seed through the APIs" python3 "$helper" seed --state "$state" --cache "$raw_cache"
timed "wait for the worker (thumbnails, EXIF, embeddings, geocode, bursts)" settle

log "the nightly manifest job (what the assets service writes once a day)"
timed "write manifests (cyanotype manifest)" dc --profile tools run --rm -T cli manifest

settle
# The dump is taken first and the snapshot after it: the snapshot's own API calls (logins, a view of the share page) change
# a few rows, and they must not be in the dump the recovery restores. Nothing else changes in between (the worker is idle).
log "the real backup job (infra/backup), the same container production uses"
timed "backup now (pg_dump to the bucket)" dc --profile backup run --rm -T backup now
dc --profile backup run --rm -T backup list | sed 's/^/    dump in bucket: /' >&2

log "canonical snapshot BEFORE the disaster"
bucket_snap before
snap before

# ---- path A: restore the dump -------------------------------------------------------------------------------------
log "PATH A: drop the database, restore the newest dump (docs/deploy.md 'Restore'), migrate, start"
stop_services
timed "A: lose the database (drop volume, new empty Postgres)" drop_db
timed "A: restore the dump (backup restore)" dc --profile backup run --rm -T -e CONFIRM=yes backup restore >/dev/null
timed "A: migrate" migrate_db
timed "A: start services" start_services
timed "A: wait for the worker" settle
log "reindex dry run after the dump restore (documented verification: expect 0 orphans)"
reindex_to "$work/a_reindex.json" || true
bucket_snap path_a
snap path_a

# ---- path B: lose the database, rebuild from the bucket alone -----------------------------------------------------
log "PATH B: drop the database again, migrate + reindex from the bucket alone"
stop_services
timed "B: lose the database (drop volume, new empty Postgres)" drop_db
timed "B: migrate" migrate_db
timed "B: reindex dry run" reindex_to "$work/b_dry.json"
dry="$(cat "$work/b_dry.json")"
log "dry run: orphans $(jget "$dry" orphans), recreated $(jget "$dry" orphans_recreated), skipped (owner unknown) $(jget "$dry" orphans_unknown_user), accounts to restore $(jget "$dry" users_restored)"
assign=()
if [ "$(jget "$dry" orphans_unknown_user)" != 0 ]; then
  log "no accounts in the bucket: the documented fallback (register the owner again, then --assign-to)"
  dc up -d --wait auth gateway
  wait_gateway
  legacy_pw="$(rand legacy)"
  owner_email="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["users"]["alice"]["email"])' "$state")"
  curl -fsS -H 'content-type: application/json' -d "{\"email\":\"$owner_email\",\"password\":\"$legacy_pw\"}" "$DRILL_BASE_URL/auth/register" >/dev/null
  dc stop -t 5 gateway auth >/dev/null
  assign=(--assign-to "$owner_email")
fi
timed "B: reindex --apply" reindex_to "$work/b_apply.json" --apply ${assign[@]+"${assign[@]}"}
timed "B: start services" start_services
if [ "${#assign[@]}" = 0 ]; then
  log "accounts came back with locked passwords: owner steps = one reset link per account (cyanotype reset-link)"
  timed "B: password reset links (cyanotype reset-link + set the password)" python3 "$helper" reset-passwords --state "$state" --compose "docker compose -p $PROJECT -f $compose_file --profile tools run --rm -T cli"
fi
timed "B: wait for the worker (thumbnails, EXIF, embeddings, places, bursts)" settle
bucket_snap path_b
snap path_b

# ---- the guard of the real-data mode, against the local bucket ----------------------------------------------------
log "self-test of the real-data mode's read-only guard against the local bucket"
if DRILL_S3_ENDPOINT="$DRILL_LOCAL_S3_URL" DRILL_S3_BUCKET=cyanotype DRILL_S3_ACCESS_KEY_ID="$DRILL_LOCAL_S3_ACCESS_KEY" DRILL_S3_SECRET_ACCESS_KEY="$DRILL_LOCAL_S3_SECRET_KEY" python3 "$helper" probe --env DRILL_S3 2>"$work/guard_rw.txt"; then
  die "guard self-test FAILED: a writable key was accepted"
fi
log "writable key refused: $(head -c 120 "$work/guard_rw.txt")..."
DRILL_S3_ENDPOINT="$DRILL_LOCAL_S3_URL" DRILL_S3_BUCKET=cyanotype DRILL_S3_ACCESS_KEY_ID="$DRILL_LOCAL_S3_RO_ACCESS_KEY" DRILL_S3_SECRET_ACCESS_KEY="$DRILL_LOCAL_S3_RO_SECRET_KEY" python3 "$helper" probe --env DRILL_S3 \
  || die "guard self-test FAILED: the read-only key was refused"

log "report"
commit="$(git -C "$repo" rev-parse --short HEAD 2>/dev/null || echo unknown)"
python3 "$helper" report --before "$work/before.json" --path-a "$work/path_a.json" --path-b "$work/path_b.json" \
  --timings "$timings" --out "$REPORT" --commit "$commit" --title "$TITLE" \
  --a-reindex "$work/a_reindex.json" --b-dry "$work/b_dry.json" --b-apply "$work/b_apply.json"
log "report written to $REPORT"
# The raw material (snapshots, reindex reports) next to the report: synthetic data only, handy for diffing.
mkdir -p "${REPORT%.md}.data"
cp "$work"/before.json "$work"/path_a.json "$work"/path_b.json "$work"/a_reindex.json "$work"/b_dry.json "$work"/b_apply.json "$timings" "${REPORT%.md}.data/" 2>/dev/null || true
