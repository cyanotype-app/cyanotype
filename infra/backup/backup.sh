#!/bin/sh
# Postgres backups to S3-compatible storage.
#
#   backup.sh daemon            run forever: one backup per day at BACKUP_AT (UTC, default 03:17)
#   backup.sh now               one backup, then prune to the newest BACKUP_KEEP
#   backup.sh list              list dumps in the bucket
#   backup.sh restore [name]    restore a dump (default: newest) into DATABASE_URL; needs CONFIRM=yes
#
# Env (the same names the services use): DATABASE_URL, S3_ENDPOINT, S3_REGION, S3_BUCKET,
# S3_ACCESS_KEY, S3_SECRET_KEY, S3_FORCE_PATH_STYLE. Optional: BACKUP_PREFIX (backups/postgres),
# BACKUP_KEEP (14), BACKUP_AT (03:17), BACKUP_ON_START (false).
# Secrets are only passed through the environment; nothing here prints them.
set -eu

: "${DATABASE_URL:?DATABASE_URL is required}"
: "${S3_BUCKET:?S3_BUCKET is required}"
: "${S3_ACCESS_KEY:?S3_ACCESS_KEY is required}"
: "${S3_SECRET_KEY:?S3_SECRET_KEY is required}"

PREFIX="${BACKUP_PREFIX:-backups/postgres}"
PREFIX="${PREFIX#/}"; PREFIX="${PREFIX%/}"
KEEP="${BACKUP_KEEP:-14}"
BACKUP_AT="${BACKUP_AT:-03:17}"
case "$KEEP" in ''|*[!0-9]*|0) echo "BACKUP_KEEP must be a positive integer" >&2; exit 2;; esac

# rclone remote "s3:" configured purely from env: no config file, no secrets on disk.
export RCLONE_CONFIG_S3_TYPE=s3
export RCLONE_CONFIG_S3_PROVIDER=Other
export RCLONE_CONFIG_S3_ACCESS_KEY_ID="$S3_ACCESS_KEY"
export RCLONE_CONFIG_S3_SECRET_ACCESS_KEY="$S3_SECRET_KEY"
export RCLONE_CONFIG_S3_REGION="${S3_REGION:-us-east-1}"
[ -n "${S3_ENDPOINT:-}" ] && export RCLONE_CONFIG_S3_ENDPOINT="$S3_ENDPOINT"
case "${S3_FORCE_PATH_STYLE:-}" in true|1) export RCLONE_CONFIG_S3_FORCE_PATH_STYLE=true;; esac
# The bucket already exists and the key may not be allowed to create buckets.
export RCLONE_CONFIG_S3_NO_CHECK_BUCKET=true

export RCLONE_LOG_LEVEL=ERROR   # errors only (also hides the harmless "no config file" notice)
REMOTE="s3:$S3_BUCKET/$PREFIX"
log() { printf '%s backup: %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$*"; }

list_dumps() { rclone lsf --files-only "$REMOTE" | grep -E '^cyanotype-[0-9]{8}T[0-9]{6}Z\.dump$' | sort; }

do_backup() {
  name="cyanotype-$(date -u +%Y%m%dT%H%M%SZ).dump"
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' EXIT
  log "dumping to $name"
  pg_dump -Fc --no-password "$DATABASE_URL" -f "$tmp/$name"
  # Refuse to upload something pg_restore cannot read.
  pg_restore --list "$tmp/$name" >/dev/null
  size="$(wc -c < "$tmp/$name" | tr -d ' ')"
  rclone copyto "$tmp/$name" "$REMOTE/$name"
  log "uploaded $PREFIX/$name ($size bytes)"
  prune
  rm -rf "$tmp"; trap - EXIT
}

# Keep the newest $KEEP dumps. On a versioned bucket this only adds delete markers: the old
# versions stay (and bill) until a lifecycle rule or a manual purge removes them.
prune() {
  total="$(list_dumps | wc -l | tr -d ' ')"
  [ "$total" -gt "$KEEP" ] || { log "$total dump(s), keeping up to $KEEP: nothing to prune"; return 0; }
  list_dumps | head -n "$((total - KEEP))" | while read -r old; do
    rclone deletefile "$REMOTE/$old"
    log "pruned $old"
  done
}

do_restore() {
  [ "${CONFIRM:-}" = yes ] || { echo "restore overwrites the database in DATABASE_URL: re-run with CONFIRM=yes" >&2; exit 2; }
  name="${1:-}"
  [ -n "$name" ] && [ "$name" != latest ] || name="$(list_dumps | tail -n 1)"
  [ -n "$name" ] || { echo "no dumps found under $PREFIX/" >&2; exit 1; }
  case "$name" in */*) echo "pass a file name, not a path" >&2; exit 2;; esac
  tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
  log "downloading $name"
  rclone copyto "$REMOTE/$name" "$tmp/$name"
  log "restoring $name"
  pg_restore --clean --if-exists --no-owner --no-password -d "$DATABASE_URL" "$tmp/$name"
  log "restore complete"
}

seconds_until() { # next HH:MM UTC
  h="${BACKUP_AT%%:*}"; m="${BACKUP_AT##*:}"
  now="$(date -u +%s)"
  # Drop one leading zero so "08" is not read as octal ("00" -> "0", "0" -> "" -> 0).
  h="${h#0}"; m="${m#0}"; h="${h:-0}"; m="${m:-0}"
  target=$(( now - now % 86400 + h*3600 + m*60 ))
  [ "$target" -gt "$now" ] || target=$((target + 86400))
  echo $((target - now))
}

case "${1:-daemon}" in
  now) do_backup ;;
  list) list_dumps ;;
  restore) shift; do_restore "${1:-}" ;;
  daemon)
    log "scheduler up: daily at $BACKUP_AT UTC, keeping $KEEP, target $S3_BUCKET/$PREFIX"
    [ "${BACKUP_ON_START:-false}" = true ] && { do_backup || log "backup FAILED"; }
    while :; do
      wait_s="$(seconds_until)"
      log "next backup in ${wait_s}s"
      sleep "$wait_s"
      # A failed night must not kill the loop; the failure is logged and the next night retries.
      ( do_backup ) || log "backup FAILED"
    done ;;
  *) echo "usage: backup.sh daemon|now|list|restore [name]" >&2; exit 2 ;;
esac
