#!/usr/bin/env bash
# Regression tests for scripts/deploy.sh without a real server.
# The fake `ssh` mimics the real one: it starts in the login home, joins the remote command words with single
# spaces and hands the result to a shell's -c (what the remote login shell does), stdin passed through. The shell
# is bash by default, or sh/dash via FAKE_SSH_SHELL so a non-bash login shell is covered. Earlier fakes kept argv
# intact and hid the real quoting bugs. A fake `docker` records its argv (and stdin for `login`) and, like the
# real `compose run`, swallows stdin. Nothing here touches a network or a real docker.
# shellcheck disable=SC2329,SC2088  # helpers are called via check; literal "~/" is the input under test
set -uo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
work="$(mktemp -d)"; trap 'rm -rf "$work"' EXIT
mkdir -p "$work/bin" "$work/home"

cat > "$work/bin/ssh" <<'FAKE'
#!/usr/bin/env bash
mode=""
while [ $# -gt 0 ]; do
  case "$1" in
    -o) shift 2 ;;
    -O) mode="$2"; shift 2 ;;
    *) break ;;
  esac
done
shift # host
[ -n "$mode" ] && exit 0 # ControlMaster management
cmd="$*" # the real ssh joins the words with spaces
printf '%s\n' "$cmd" >> "$FAKE_LOG_DIR/ssh-commands"
if [ -n "${FAKE_SSH_SKIP_ROLLOUT:-}" ] && [[ "$cmd" == *DEPLOY-OK* ]]; then exit 0; fi # simulate a silently truncated run
cd "$HOME" # ssh starts in the login home
exec "${FAKE_SSH_SHELL:-bash}" -c "$cmd"
FAKE
cat > "$work/bin/docker" <<'FAKE'
#!/usr/bin/env bash
printf 'docker %s\n' "$*" >> "$FAKE_LOG_DIR/docker"
if [ "${1:-}" = login ]; then printf 'stdin=%s\n' "$(cat)" >> "$FAKE_LOG_DIR/docker"; fi
case " $* " in *" run "*) cat > /dev/null ;; esac
FAKE
chmod +x "$work/bin/ssh" "$work/bin/docker"

fail=0
check() { # check DESC COMMAND...
  local desc="$1"; shift
  if "$@"; then echo "ok   - $desc"; else echo "FAIL - $desc"; fail=1; fi
}
lineno() { grep -n -m1 -- "$2" "$1" | cut -d: -f1; }
in_order() { # in_order LOG PATTERN...: first matches appear in increasing line order
  local log="$1" prev=0 n p; shift
  for p in "$@"; do n="$(lineno "$log" "$p")"; [ -n "$n" ] && [ "$n" -gt "$prev" ] || return 1; prev="$n"; done
}
no_match() { ! grep -q -e "$1" "$2"; }

# run_case NAME PROFILES DEPLOY_DIR SERVER_PATH SHELL -> sets $logs, $server, $rc. HOME is the fake login home.
run_case() {
  local name="$1" profiles="$2" dirspec="$3" shell="$4"
  server="$5"; logs="$work/$name/logs"
  mkdir -p "$server" "$logs"
  printf 'CYANOTYPE_VERSION=old\nDD_API_KEY=\n' > "$server/.env.prod"
  echo "== $name (profiles='$profiles' dir='$dirspec' shell=$shell)"
  HOME="$work/home" FAKE_SSH_SHELL="$shell" FAKE_LOG_DIR="$logs" PATH="$work/bin:$PATH" DEPLOY_SSH=fake-host \
    DEPLOY_DIR="$dirspec" DEPLOY_REF=worktree DEPLOY_PROFILES="$profiles" GHCR_TOKEN=s3cret GHCR_USER=ci-user \
    "$here/deploy.sh" sha-abc1234 > "$work/$name/out" 2>&1
  rc=$?
  [ "$rc" = 0 ] || { echo "---- deploy.sh output:"; cat "$work/$name/out"; echo "----"; }
}

common_checks() { # common_checks NAME
  check "$1: deploy exits 0" test "$rc" = 0
  check "$1: DEPLOY-OK reported" grep -qx 'DEPLOY-OK sha-abc1234' "$work/$1/out"
  check "$1: version written to .env.prod" grep -qx 'CYANOTYPE_VERSION=sha-abc1234' "$server/.env.prod"
  check "$1: bundle landed in the right dir" test -f "$server/docker-compose.prod.yml"
  check "$1: pull -> migrate -> up -> ps in order" in_order "$logs/docker" ' pull ' ' run --rm -T migrate' ' up -d' ' ps'
  check "$1: login with user and stdin token" grep -qxF 'stdin=s3cret' "$logs/docker"
  check "$1: login args intact" grep -qF 'docker login ghcr.io -u ci-user --password-stdin' "$logs/docker"
  check "$1: logout ran" grep -qF 'docker logout ghcr.io' "$logs/docker"
}

abs="$work/abs-server"
run_case empty "" "$abs" bash "$abs"
common_checks empty
check "empty: no --profile passed" no_match "--profile" "$logs/docker"
check "empty: remote saw an empty third arg" grep -qF "_ sha-abc1234 $abs ''" "$logs/ssh-commands"
check "empty: no bash-only \$'..' quoting" no_match "\\\$'" "$logs/ssh-commands"

run_case datadog "datadog" "$abs-dd" bash "$abs-dd"
common_checks datadog
check "datadog: --profile datadog passed to compose" grep -q -- '--profile datadog .*up -d' "$logs/docker"
check "datadog: remote saw it as third arg" grep -qF "_ sha-abc1234 $abs-dd datadog" "$logs/ssh-commands"

run_case two "datadog other" "$abs-two" bash "$abs-two"
check "two profiles: both passed" grep -q -- '--profile datadog --profile other .*up -d' "$logs/docker"

# Non-bash login shell (dash if present, else sh): the quoting must be plain POSIX.
sh_shell="sh"; if command -v dash >/dev/null 2>&1; then sh_shell="dash"; fi
run_case dash "datadog" "$abs-dash" "$sh_shell" "$abs-dash"
common_checks dash
check "dash: profile passed" grep -q -- '--profile datadog .*up -d' "$logs/docker"
run_case dashempty "" "$abs-dashempty" "$sh_shell" "$abs-dashempty"
common_checks dashempty

# DEPLOY_DIR forms: relative to the login home, and ~/ (stripped locally; nothing remote expands ~).
run_case relative "" "cyanotype" bash "$work/home/cyanotype"
common_checks relative
run_case tilde "" "~/tilde-dir" "$sh_shell" "$work/home/tilde-dir"
common_checks tilde
check "tilde: ~ never sent to the server" no_match '~' "$logs/ssh-commands"

# ~user forms and junk are rejected before any ssh.
for bad in '~bob/x' '-x' 'a b' 'a;b'; do
  out="$(HOME="$work/home" PATH="$work/bin:$PATH" FAKE_LOG_DIR="$work" DEPLOY_SSH=fake-host DEPLOY_DIR="$bad" DEPLOY_REF=worktree "$here/deploy.sh" sha-abc1234 2>&1)"; brc=$?
  check "DEPLOY_DIR '$bad' rejected" test "$brc" = 2
  check "DEPLOY_DIR '$bad' rejected with a message" grep -q 'bad DEPLOY_DIR' <<<"$out"
done

# A rollout that silently does nothing (exit 0, no marker) must fail the deploy.
echo "== skipped rollout"
skip="$work/skip-server"; mkdir -p "$skip" "$work/skip/logs"; printf 'CYANOTYPE_VERSION=old\n' > "$skip/.env.prod"
HOME="$work/home" FAKE_SSH_SKIP_ROLLOUT=1 FAKE_LOG_DIR="$work/skip/logs" PATH="$work/bin:$PATH" DEPLOY_SSH=fake-host \
  DEPLOY_DIR="$skip" DEPLOY_REF=worktree "$here/deploy.sh" sha-abc1234 > "$work/skip/out" 2>&1
check "missing marker fails the deploy" test "$?" != 0
check "missing marker explained" grep -q 'did not report DEPLOY-OK' "$work/skip/out"

# scripts/reindex.sh: a dry run by default, only known flags reach the server, compose run reindex with --json.
echo "== reindex.sh"
rx="$work/reindex-server"; mkdir -p "$rx" "$work/rx/logs"; printf 'CYANOTYPE_VERSION=v1\n' > "$rx/.env.prod"
rxrun() { HOME="$work/home" FAKE_LOG_DIR="$work/rx/logs" PATH="$work/bin:$PATH" DEPLOY_SSH=fake-host DEPLOY_DIR="$rx" "$here/reindex.sh" "$@" > "$work/rx/out" 2>&1; }
: > "$work/rx/logs/docker"
rxrun; check "reindex.sh dry run exits 0" test "$?" = 0
check "reindex.sh runs the one-shot service with --json only" grep -qE 'run --rm -T reindex --json$' "$work/rx/logs/docker"
check "reindex.sh dry run passes no --apply" no_match -- '--apply' "$work/rx/logs/docker"
rxrun --apply --mark-missing --assign-to me@example.com; check "reindex.sh apply exits 0" test "$?" = 0
check "reindex.sh forwards flags" grep -qF 'reindex --json --apply --mark-missing --assign-to me@example.com' "$work/rx/logs/docker"
rxrun --apply '; rm -rf /'; check "reindex.sh rejects unknown flags" test "$?" = 2
rxrun --assign-to 'x;y'; check "reindex.sh rejects a bad email" test "$?" = 2

[ "$fail" = 0 ] && echo "all passed" || echo "SOME TESTS FAILED"
exit "$fail"
