#!/usr/bin/env bash
# End-to-end test of `cyanotype import` through the gateway:
#   register -> login (mints an API key) -> import a generated tree -> re-run (all skipped) -> interrupted run
#   resumes -> files visible in GET /assets.
#
# Usage: BASE_URL=http://localhost:3000 scripts/e2e-import.sh
# Needs: bash, curl, python3, a running stack (`just up`), and the CLI (built here with cargo unless CYANOTYPE_BIN is set).
# Registers a throwaway user (REGISTRATION_MODE=open, as in the local compose stack). Nothing secret is printed.
set -euo pipefail

BASE_URL="${BASE_URL:-http://localhost:3000}"
BASE_URL="${BASE_URL%/}"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

step() { printf '==> %s\n' "$*"; }
fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }

if [ -z "${CYANOTYPE_BIN:-}" ]; then
  step "build the CLI"
  (cd "$root" && cargo build -q -p cyanotype-cli)
  CYANOTYPE_BIN="${CARGO_TARGET_DIR:-$root/target}/debug/cyanotype"
fi
export XDG_CONFIG_HOME="$work/config" XDG_CACHE_HOME="$work/cache"
unset CYANOTYPE_SERVER CYANOTYPE_API_KEY
cy() { "$CYANOTYPE_BIN" "$@"; }

step "generate a library (jpg/png, fake cr3/heic, a 20 MiB multipart dng, sidecars and dotfiles to skip)"
python3 - "$work/lib" <<'PY'
import os, struct, sys, zlib
root = sys.argv[1]
def w(rel, data):
    p = os.path.join(root, rel); os.makedirs(os.path.dirname(p), exist_ok=True); open(p, "wb").write(data)
def png():
    r, g, b = os.urandom(3)
    raw = b"".join(b"\x00" + bytes([r, g, b]) * 16 for _ in range(16))
    def chunk(t, d):
        c = struct.pack(">I", len(d)) + t + d
        return c + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 16, 16, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b"")
for i in range(20): w(f"trip/IMG_{i:03d}.png", png())
for i in range(3):  w(f"raw/DSC_{i:03d}.CR3", os.urandom(4000 + i))
for i in range(2):  w(f"phone/IMG_{i:03d}.HEIC", os.urandom(3000 + i))
w("raw/big.dng", os.urandom(20 * 1024 * 1024 + 7))
w("trip/IMG_000.png.xmp", b"<x/>"); w("phone/IMG_000.AAE", b"x"); w(".DS_Store", b"x"); w("trip/.hidden.png", png()); w("notes.txt", b"x")
PY
expected=26   # 20 png + 3 cr3 + 2 heic + 1 dng

step "register + login (creates an API key)"
email="e2e-import-$(date +%s)-$RANDOM@example.invalid"
password="$(python3 -c 'import secrets; print(secrets.token_urlsafe(18))')"
body="$(python3 -c 'import json,sys; print(json.dumps({"email":sys.argv[1],"password":sys.argv[2]}))' "$email" "$password")"
code="$(curl -sS -o /dev/null -w '%{http_code}' -H 'content-type: application/json' -d "$body" "$BASE_URL/auth/register")"
[ "$code" = 201 ] || fail "register returned HTTP $code (needs REGISTRATION_MODE=open)"
printf '%s\n' "$password" | cy login --server "$BASE_URL" --email "$email" --password-stdin >/dev/null
[ "$(stat -c %a "$XDG_CONFIG_HOME/cyanotype/config.toml")" = 600 ] || fail "config.toml is not mode 600"

step "dry run makes no changes"
cy import --dry-run "$work/lib" | grep -q "would check $expected files" || fail "dry run count"
[ "$(cy status | awk '/^Assets:/{print $2}')" = 0 ] || fail "dry run uploaded something"

step "import"
cy import -v "$work/lib" | tee "$work/run1.txt"
grep -q "uploaded:         $expected files" "$work/run1.txt" || fail "expected $expected uploads"

step "re-run: everything already present"
cy import "$work/lib" | tee "$work/run2.txt"
grep -q "uploaded:         0 files" "$work/run2.txt" && grep -q "already present:  $expected files" "$work/run2.txt" || fail "re-run should skip all"

step "interrupted import resumes"
python3 -c 'import os; open("'"$work"'/lib/raw/second.dng","wb").write(os.urandom(40*1024*1024))'
( timeout -s KILL 0.4 "$CYANOTYPE_BIN" import --jobs 1 --part-jobs 1 "$work/lib" >/dev/null 2>&1 || true )
cy import "$work/lib" | tee "$work/run3.txt"
grep -q "failed:           0" "$work/run3.txt" || fail "resume run had failures"

step "assets are visible"
n="$(cy status | awk '/^Assets:/{print $2}')"
[ "$n" = $((expected + 1)) ] || fail "expected $((expected + 1)) assets, status says $n"
key="$(sed -n 's/^api_key *= *"\(.*\)"/\1/p' "$XDG_CONFIG_HOME/cyanotype/config.toml")"
listed="$(curl -fsS -H "Authorization: Bearer $key" "$BASE_URL/assets?limit=500" | python3 -c 'import sys,json; print(len(json.load(sys.stdin)["items"]))')"
[ "$listed" = $((expected + 1)) ] || fail "GET /assets lists $listed items"

step "an API key cannot mint keys"
code="$(curl -sS -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $key" -H 'content-type: application/json' -d '{"name":"x"}' "$BASE_URL/auth/api-keys")"
[ "$code" = 403 ] || fail "expected 403 minting a key with a key, got $code"

printf 'PASS: import e2e against %s\n' "$BASE_URL"
