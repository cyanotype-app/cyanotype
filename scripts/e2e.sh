#!/usr/bin/env bash
# End-to-end smoke test through the gateway:
#   register -> /assets/check -> PUT part -> /assets/complete -> wait for worker -> GET asset -> GET thumbnail
#
# Usage: BASE_URL=http://localhost:3000 scripts/e2e.sh
# Needs: bash, curl, python3, sha256sum. A running stack (`just up`) and the worker.
# Presigned URLs point at S3_PUBLIC_ENDPOINT, which must be reachable from where this runs.
# Credentials are generated per run; nothing secret is printed.
set -euo pipefail

BASE_URL="${BASE_URL:-http://localhost:3000}"
BASE_URL="${BASE_URL%/}"
TIMEOUT_SECS="${E2E_TIMEOUT_SECS:-60}"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
jar="$work/cookies"

step() { printf '==> %s\n' "$*"; }
fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }
# json <python-expr over d>: read JSON from stdin, print the expression.
json() { python3 -c 'import sys,json; d=json.load(sys.stdin); v=eval(sys.argv[1]); print("" if v is None else v)' "$1"; }

# A tiny unique PNG (random colour => unique sha256 => never a dedupe hit).
python3 - "$work/photo.png" <<'PY'
import os, struct, sys, zlib
w = h = 64
r, g, b = os.urandom(3)
raw = b"".join(b"\x00" + bytes([r, g, b]) * w for _ in range(h))
def chunk(t, d):
    c = struct.pack(">I", len(d)) + t + d
    return c + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)
png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)) \
    + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b"")
open(sys.argv[1], "wb").write(png)
PY
file="$work/photo.png"
sha="$(sha256sum "$file" | cut -d' ' -f1)"
size="$(wc -c < "$file" | tr -d ' ')"

step "gateway health ($BASE_URL)"
curl -fsS "$BASE_URL/healthz" >/dev/null || fail "gateway not reachable"

step "register"
email="e2e-$(date +%s)-$RANDOM@example.invalid"
password="$(python3 -c 'import secrets; print(secrets.token_urlsafe(18))')"
body="$(python3 -c 'import json,sys; print(json.dumps({"email":sys.argv[1],"password":sys.argv[2]}))' "$email" "$password")"
curl -fsS -c "$jar" -H 'content-type: application/json' -d "$body" "$BASE_URL/auth/register" >/dev/null
curl -fsS -b "$jar" "$BASE_URL/auth/me" >/dev/null || fail "session cookie not accepted by /auth/me"

step "check (expect upload)"
req="$(python3 -c 'import json,sys; print(json.dumps({"sha256":sys.argv[1],"size_bytes":int(sys.argv[2]),"filename":"e2e.png","content_type":"image/png"}))' "$sha" "$size")"
check="$(curl -fsS -b "$jar" -H 'content-type: application/json' -d "$req" "$BASE_URL/assets/check")"
[ "$(printf '%s' "$check" | json 'd["status"]')" = upload ] || fail "expected status=upload"
upload_id="$(printf '%s' "$check" | json 'd["upload_id"]')"
part_url="$(printf '%s' "$check" | json 'd["parts"][0]["url"]')"

step "PUT part straight to S3"
etag="$(curl -fsS -D - -o /dev/null -X PUT --data-binary "@$file" "$part_url" | tr -d '\r' | awk 'tolower($1)=="etag:"{print $2}')"
[ -n "$etag" ] || fail "no ETag returned by S3 (is S3_PUBLIC_ENDPOINT reachable from here?)"

step "complete"
req="$(python3 -c 'import json,sys; print(json.dumps({"upload_id":sys.argv[1],"parts":[{"number":1,"etag":sys.argv[2]}]}))' "$upload_id" "$etag")"
done_resp="$(curl -fsS -b "$jar" -H 'content-type: application/json' -d "$req" "$BASE_URL/assets/complete")"
asset_id="$(printf '%s' "$done_resp" | json 'd["asset_id"]')"
[ -n "$asset_id" ] || fail "no asset_id"
echo "asset_id=$asset_id"

step "wait for worker thumbnail (up to ${TIMEOUT_SECS}s)"
deadline=$((SECONDS + TIMEOUT_SECS))
thumb=""
while [ "$SECONDS" -lt "$deadline" ]; do
  asset="$(curl -fsS -b "$jar" "$BASE_URL/assets/$asset_id")"
  thumb="$(printf '%s' "$asset" | json 'd.get("thumb_url")')"
  [ -n "$thumb" ] && break
  sleep 1
done
[ -n "$thumb" ] || fail "thumb_url still null after ${TIMEOUT_SECS}s (is the worker running?)"

step "GET original and thumbnail via presigned URLs"
orig="$(printf '%s' "$asset" | json 'd["original_url"]')"
curl -fsS "$orig" | sha256sum | grep -q "^$sha" || fail "original bytes differ from what was uploaded"
ctype="$(curl -fsS -o "$work/thumb.bin" -w '%{content_type}' "$thumb")"
[ -s "$work/thumb.bin" ] || fail "empty thumbnail"
echo "thumbnail: $ctype, $(wc -c < "$work/thumb.bin" | tr -d ' ') bytes"

step "list contains the asset"
curl -fsS -b "$jar" "$BASE_URL/assets?limit=10" | grep -q "$asset_id" || fail "asset missing from list"

echo "e2e OK"
