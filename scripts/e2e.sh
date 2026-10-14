#!/usr/bin/env bash
# End-to-end smoke test through the gateway:
#   register -> /assets/check -> PUT part -> /assets/complete -> wait for worker -> GET asset -> GET thumbnail
# then the same for a second photo through the backend-proxied transport used by the browser UI:
#   /assets/check {transport:proxy} -> PUT <gateway>/assets/uploads/{id}/parts/1 -> complete -> worker thumbnail
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

# Against an instance in REGISTRATION_MODE=first_user/closed (production), registration answers 403 once an
# owner exists: set E2E_EMAIL and E2E_PASSWORD to log in as an existing user instead (the upload leaves a
# real 64x64 test photo in that library). The local compose stack runs REGISTRATION_MODE=open.
step "register"
email="e2e-$(date +%s)-$RANDOM@example.invalid"
password="$(python3 -c 'import secrets; print(secrets.token_urlsafe(18))')"
body="$(python3 -c 'import json,sys; print(json.dumps({"email":sys.argv[1],"password":sys.argv[2]}))' "$email" "$password")"
code="$(curl -sS -o /dev/null -w '%{http_code}' -c "$jar" -H 'content-type: application/json' -d "$body" "$BASE_URL/auth/register")"
if [ "$code" = 403 ] && [ -n "${E2E_EMAIL:-}" ] && [ -n "${E2E_PASSWORD:-}" ]; then
  step "registration closed (403): logging in as E2E_EMAIL instead"
  body="$(python3 -c 'import json,os; print(json.dumps({"email":os.environ["E2E_EMAIL"],"password":os.environ["E2E_PASSWORD"]}))')"
  curl -fsS -c "$jar" -H 'content-type: application/json' -d "$body" "$BASE_URL/auth/login" >/dev/null || fail "login failed"
elif [ "$code" = 403 ]; then
  fail "registration is closed on this instance (set E2E_EMAIL and E2E_PASSWORD to use an existing account)"
elif [ "$code" = 429 ]; then
  fail "rate limited (429) by the auth service; wait a minute and retry"
elif [ "$code" != 201 ]; then
  fail "register returned HTTP $code"
fi
curl -fsS -b "$jar" "$BASE_URL/auth/me" >/dev/null || fail "session cookie not accepted by /auth/me (is COOKIE_SECURE=true over plain http?)"

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

# ---- Proxied (browser) transport -------------------------------------------------------------------------------
# A second unique PNG; its bytes go through the gateway and the assets service instead of straight to S3.
python3 - "$work/photo2.png" <<'PY'
import os, struct, sys, zlib
w = h = 48
r, g, b = os.urandom(3)
raw = b"".join(b"\x00" + bytes([r, g, b]) * w for _ in range(h))
def chunk(t, d):
    c = struct.pack(">I", len(d)) + t + d
    return c + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)
png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)) \
    + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b"")
open(sys.argv[1], "wb").write(png)
PY
file2="$work/photo2.png"
sha2="$(sha256sum "$file2" | cut -d' ' -f1)"
size2="$(wc -c < "$file2" | tr -d ' ')"

step "proxy: check (transport=proxy)"
req="$(python3 -c 'import json,sys; print(json.dumps({"sha256":sys.argv[1],"size_bytes":int(sys.argv[2]),"filename":"e2e-proxy.png","content_type":"image/png","transport":"proxy"}))' "$sha2" "$size2")"
check="$(curl -fsS -b "$jar" -H 'content-type: application/json' -d "$req" "$BASE_URL/assets/check")"
[ "$(printf '%s' "$check" | json 'd["status"]')" = upload ] || fail "proxy: expected status=upload"
upload_id="$(printf '%s' "$check" | json 'd["upload_id"]')"
part_url="$(printf '%s' "$check" | json 'd["parts"][0]["url"]')"
case "$part_url" in /assets/uploads/*/parts/1) ;; *) fail "proxy: part url is not a relative proxy url: $part_url" ;; esac

step "proxy: PUT without a session is refused, a wrong-sized body is a 400"
code="$(curl -sS -o /dev/null -w '%{http_code}' -X PUT --data-binary "@$file2" "$BASE_URL$part_url")"
[ "$code" = 401 ] || fail "proxy: unauthenticated PUT returned $code, expected 401"
code="$(curl -sS -o /dev/null -w '%{http_code}' -b "$jar" -X PUT --data-binary "short" "$BASE_URL$part_url")"
[ "$code" = 400 ] || fail "proxy: wrong-size PUT returned $code, expected 400"

step "proxy: PUT part through the assets service"
etag="$(curl -fsS -b "$jar" -X PUT --data-binary "@$file2" "$BASE_URL$part_url" | json 'd["etag"]')"
[ -n "$etag" ] || fail "proxy: no etag in the response"

step "proxy: complete + worker thumbnail"
req="$(python3 -c 'import json,sys; print(json.dumps({"upload_id":sys.argv[1],"parts":[{"number":1,"etag":sys.argv[2]}]}))' "$upload_id" "$etag")"
asset2="$(curl -fsS -b "$jar" -H 'content-type: application/json' -d "$req" "$BASE_URL/assets/complete" | json 'd["asset_id"]')"
[ -n "$asset2" ] || fail "proxy: no asset_id"
deadline=$((SECONDS + TIMEOUT_SECS))
thumb=""
while [ "$SECONDS" -lt "$deadline" ]; do
  thumb="$(curl -fsS -b "$jar" "$BASE_URL/assets/$asset2" | json 'd.get("thumb_url")')"
  [ -n "$thumb" ] && break
  sleep 1
done
[ -n "$thumb" ] || fail "proxy: thumb_url still null after ${TIMEOUT_SECS}s"
orig="$(curl -fsS -b "$jar" "$BASE_URL/assets/$asset2" | json 'd["original_url"]')"
curl -fsS "$orig" | sha256sum | grep -q "^$sha2" || fail "proxy: original bytes differ from what was uploaded"

echo "e2e OK"
