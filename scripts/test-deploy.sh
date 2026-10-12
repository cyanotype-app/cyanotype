#!/usr/bin/env bash
# Regression test for scripts/deploy.sh without a real server.
# The fake `ssh` mimics the real one: it joins the remote command words with single spaces and hands the
# result to `bash -c` (what the remote login shell does), with stdin passed through. That re-splitting is what
# made an empty DEPLOY_PROFILES vanish ("$3: unbound variable"); a fake that kept argv intact hid the bug.
# A fake `docker` records its argv (and stdin for `login`). Nothing here contacts a network or a real docker.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
work="$(mktemp -d)"; trap 'rm -rf "$work"' EXIT
mkdir "$work/bin"

cat > "$work/bin/ssh" <<'FAKE'
#!/usr/bin/env bash
# usage: ssh [-o opt]... host command-words...
while [ "${1:-}" = "-o" ]; do shift 2; done
shift # host
cmd="$*" # the real ssh joins the words with spaces
if [ -n "${FAKE_SSH_SKIP_ROLLOUT:-}" ] && [[ "$cmd" == *DEPLOY-OK* ]]; then exit 0; fi # simulate a silently truncated run
printf '%s\n' "$cmd" >> "$FAKE_LOG_DIR/ssh-commands"
exec bash -c "$cmd"
FAKE
cat > "$work/bin/docker" <<'FAKE'
#!/usr/bin/env bash
if [ "${1:-}" = login ]; then
  printf 'docker %s\n' "$*" >> "$FAKE_LOG_DIR/docker"
  printf 'stdin=%s\n' "$(cat)" >> "$FAKE_LOG_DIR/docker"
else
  printf 'docker %s\n' "$*" >> "$FAKE_LOG_DIR/docker"
  # Like the real `docker compose run`, swallow whatever is left on stdin.
  case " $* " in *" run "*) cat > /dev/null ;; esac
fi
FAKE
chmod +x "$work/bin/ssh" "$work/bin/docker"

fail=0
check() { # check DESC CONDITION-COMMAND...
  local desc="$1"; shift
  if "$@"; then echo "ok   - $desc"; else echo "FAIL - $desc"; fail=1; fi
}

run_case() { # run_case NAME PROFILES -> sets $logs, $server
  local name="$1" profiles="$2"
  server="$work/$name/server"; logs="$work/$name/logs"
  mkdir -p "$server" "$logs"
  printf 'CYANOTYPE_VERSION=old\nDD_API_KEY=\n' > "$server/.env.prod"
  echo "== $name (DEPLOY_PROFILES='$profiles')"
  FAKE_LOG_DIR="$logs" PATH="$work/bin:$PATH" DEPLOY_SSH=fake-host DEPLOY_DIR="$server" \
    DEPLOY_REF=worktree DEPLOY_PROFILES="$profiles" GHCR_TOKEN=s3cret GHCR_USER=ci-user \
    "$here/deploy.sh" sha-abc1234 > "$work/$name/out" 2>&1 || { cat "$work/$name/out"; return 1; }
}

run_case empty ""
check "empty profiles: deploy completes" true
check "empty profiles: version written to .env.prod" grep -qx 'CYANOTYPE_VERSION=sha-abc1234' "$server/.env.prod"
check "empty profiles: compose ran without a profile" grep -q 'compose --env-file .env.prod -f docker-compose.yml -f docker-compose.prod.yml up -d' "$logs/docker"
# shellcheck disable=SC2016
check "empty profiles: no --profile passed" bash -c '! grep -q -- "--profile" "$1"' _ "$logs/docker"
check "empty profiles: remote saw an empty third arg" grep -qF "_ sha-abc1234 $server ''" "$logs/ssh-commands"
# shellcheck disable=SC2016
check "empty profiles: pull, migrate, up, ps all ran after migrate" bash -c 'grep -q " run --rm -T migrate" "$1" && grep -q " up -d" "$1" && grep -q " ps" "$1"' _ "$logs/docker"
check "empty profiles: success marker reported" grep -qx 'DEPLOY-OK sha-abc1234' "$work/empty/out"
check "login used the user and stdin token" grep -qxF 'stdin=s3cret' "$logs/docker"
check "login args intact" grep -qF 'docker login ghcr.io -u ci-user --password-stdin' "$logs/docker"
check "logout ran" grep -qF 'docker logout ghcr.io' "$logs/docker"

run_case datadog "datadog"
check "datadog profile: --profile datadog passed to compose" grep -q -- '--profile datadog .*up -d' "$logs/docker"
check "datadog profile: remote saw it as third arg" grep -qF "_ sha-abc1234 $server datadog" "$logs/ssh-commands"

run_case two "datadog other"
check "two profiles: both passed" grep -q -- '--profile datadog --profile other .*up -d' "$logs/docker"

# A rollout that silently does nothing (exit 0, no marker) must fail the deploy.
echo "== skipped rollout"
mkdir -p "$work/skip/server" "$work/skip/logs"; printf 'CYANOTYPE_VERSION=old\n' > "$work/skip/server/.env.prod"
if FAKE_SSH_SKIP_ROLLOUT=1 FAKE_LOG_DIR="$work/skip/logs" PATH="$work/bin:$PATH" DEPLOY_SSH=fake-host DEPLOY_DIR="$work/skip/server" \
  DEPLOY_REF=worktree "$here/deploy.sh" sha-abc1234 > "$work/skip/out" 2>&1; then
  echo "FAIL - deploy passed without the DEPLOY-OK marker"; fail=1
else
  check "missing marker fails the deploy" grep -q 'did not report DEPLOY-OK' "$work/skip/out"
fi

exit "$fail"
