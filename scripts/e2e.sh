#!/usr/bin/env bash
# End-to-end smoke test through the gateway:
#   register -> /assets/check -> PUT part -> /assets/complete -> wait for worker -> GET asset -> GET thumbnail
# then the same for a second photo through the backend-proxied transport used by the browser UI:
#   /assets/check {transport:proxy} -> PUT <gateway>/assets/uploads/{id}/parts/1 -> complete -> worker thumbnail
# then photo management on those two photos:
#   trash -> gone from the library, visible in /assets/trash with its thumbnail -> restore -> trash -> delete forever
#   (the janitor removes the S3 objects: presigned GETs of the original and thumbnail turn into 404s) -> empty trash
# With E2E_S3_ENDPOINT, E2E_S3_BUCKET, E2E_S3_ACCESS_KEY and E2E_S3_SECRET_KEY set (a path-style endpoint such as
# local MinIO or SeaweedFS; never a real bucket) it also lists object *versions* and delete markers under the
# photos' keys and asserts none are left after the purge.
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
orig1="$orig"; thumb1="$thumb" # presigned, valid for minutes: used after the purge below
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

orig2="$orig"; thumb2="$thumb"

# ---- Photo management: trash, restore, delete forever ----------------------------------------------------------
post() { curl -fsS -b "$jar" -H 'content-type: application/json' -d "$2" "$BASE_URL$1"; }
status_of() { curl -sS -o /dev/null -w '%{http_code}' "$1"; }
ids_of() { python3 -c 'import json,sys; print(json.dumps({"ids": sys.argv[1:]}))' "$@"; }
# wait_gone <url>: the janitor deletes in the background right after the purge request; wait for the 404.
wait_gone() {
  local deadline=$((SECONDS + TIMEOUT_SECS)) code
  while [ "$SECONDS" -lt "$deadline" ]; do
    code="$(status_of "$1")"
    [ "$code" = 404 ] && return 0
    sleep 1
  done
  return 1
}
# s3_versions <key prefix>: how many object versions + delete markers exist under it (needs the E2E_S3_* variables).
s3_versions() {
  python3 - "$1" <<'PY'
import datetime, hashlib, hmac, os, sys, urllib.parse, urllib.request
endpoint = os.environ["E2E_S3_ENDPOINT"].rstrip("/")
bucket, region = os.environ["E2E_S3_BUCKET"], os.environ.get("E2E_S3_REGION", "us-east-1")
access, secret = os.environ["E2E_S3_ACCESS_KEY"], os.environ["E2E_S3_SECRET_KEY"]
host = urllib.parse.urlparse(endpoint).netloc
now = datetime.datetime.now(datetime.timezone.utc)
amz, day = now.strftime("%Y%m%dT%H%M%SZ"), now.strftime("%Y%m%d")
q = "&".join(f"{urllib.parse.quote(k, safe='')}={urllib.parse.quote(v, safe='')}" for k, v in sorted([("prefix", sys.argv[1]), ("versions", "")]))
empty = hashlib.sha256(b"").hexdigest()
canon = f"GET\n/{bucket}\n{q}\nhost:{host}\nx-amz-content-sha256:{empty}\nx-amz-date:{amz}\n\nhost;x-amz-content-sha256;x-amz-date\n{empty}"
scope = f"{day}/{region}/s3/aws4_request"
sts = f"AWS4-HMAC-SHA256\n{amz}\n{scope}\n{hashlib.sha256(canon.encode()).hexdigest()}"
k = ("AWS4" + secret).encode()
for part in (day, region, "s3", "aws4_request"):
    k = hmac.new(k, part.encode(), hashlib.sha256).digest()
sig = hmac.new(k, sts.encode(), hashlib.sha256).hexdigest()
auth = f"AWS4-HMAC-SHA256 Credential={access}/{scope}, SignedHeaders=host;x-amz-content-sha256;x-amz-date, Signature={sig}"
req = urllib.request.Request(f"{endpoint}/{bucket}?{q}", headers={"x-amz-date": amz, "x-amz-content-sha256": empty, "Authorization": auth})
body = urllib.request.urlopen(req).read().decode()
print(body.count("<Version>") + body.count("<DeleteMarker>"))
PY
}
user_id="$(curl -fsS -b "$jar" "$BASE_URL/auth/me" | json 'd["id"]')"
have_s3=""
if [ -n "${E2E_S3_ENDPOINT:-}" ] && [ -n "${E2E_S3_BUCKET:-}" ] && [ -n "${E2E_S3_ACCESS_KEY:-}" ] && [ -n "${E2E_S3_SECRET_KEY:-}" ]; then have_s3=1; fi
if [ -n "$have_s3" ]; then
  [ "$(s3_versions "originals/$user_id/")" -ge 2 ] || fail "S3 inspection finds no originals under originals/$user_id/ (wrong E2E_S3_* settings?)"
  [ "$(s3_versions "derived/$user_id/$asset_id/")" -ge 2 ] || fail "S3 inspection finds no derived objects for the first photo"
fi

step "trash: the photo leaves the library but keeps its thumbnail in the trash"
[ "$(post /assets/trash "$(ids_of "$asset_id" "$asset_id" 00000000-0000-0000-0000-000000000000)" | json 'd["count"]')" = 1 ] || fail "trash: expected count 1"
[ "$(post /assets/trash "$(ids_of "$asset_id")" | json 'd["count"]')" = 0 ] || fail "trash is not idempotent"
curl -fsS -b "$jar" "$BASE_URL/assets?limit=50" | grep -q "$asset_id" && fail "trashed photo is still in the library"
[ "$(curl -sS -o /dev/null -w '%{http_code}' -b "$jar" "$BASE_URL/assets/$asset_id")" = 404 ] || fail "trashed photo is still readable by id"
trash_list="$(curl -fsS -b "$jar" "$BASE_URL/assets/trash")"
[ "$(printf '%s' "$trash_list" | json 'len([i for i in d["items"] if i["id"]=="'"$asset_id"'"])')" = 1 ] || fail "photo missing from /assets/trash"
tthumb="$(printf '%s' "$trash_list" | json '[i for i in d["items"] if i["id"]=="'"$asset_id"'"][0]["thumb_url"]')"
[ -n "$tthumb" ] && [ "$(status_of "$tthumb")" = 200 ] || fail "trashed photo lost its thumbnail"
printf '%s' "$trash_list" | json '[i for i in d["items"] if i["id"]=="'"$asset_id"'"][0]["purge_at"]' | grep -q . || fail "no purge_at"

step "restore"
[ "$(post /assets/restore "$(ids_of "$asset_id")" | json 'd["count"]')" = 1 ] || fail "restore: expected count 1"
curl -fsS -b "$jar" "$BASE_URL/assets?limit=50" | grep -q "$asset_id" || fail "restored photo is not back in the library"

step "delete forever: not allowed for a photo that is not in the trash"
[ "$(post /assets/purge "$(ids_of "$asset_id")" | json 'd["count"]')" = 0 ] || fail "purge of a live photo must count 0"
[ "$(status_of "$orig1")" = 200 ] || fail "live photo's original disappeared"

step "delete forever: trash -> purge -> S3 objects removed by the janitor"
post /assets/trash "$(ids_of "$asset_id")" >/dev/null
[ "$(post /assets/purge "$(ids_of "$asset_id")" | json 'd["count"]')" = 1 ] || fail "purge: expected count 1"
curl -fsS -b "$jar" "$BASE_URL/assets/trash" | grep -q "$asset_id" && fail "purged photo still listed in the trash"
[ "$(post /assets/restore "$(ids_of "$asset_id")" | json 'd["count"]')" = 0 ] || fail "a purged photo must not be restorable"
wait_gone "$orig1" || fail "original still downloadable ${TIMEOUT_SECS}s after the purge (janitor not running?)"
wait_gone "$thumb1" || fail "thumbnail still downloadable after the purge"
if [ -n "$have_s3" ]; then
  # Versioned bucket: a plain delete would leave bytes behind as old versions; none may remain.
  deadline=$((SECONDS + TIMEOUT_SECS)); left=-1
  while [ "$SECONDS" -lt "$deadline" ]; do
    left=$(( $(s3_versions "derived/$user_id/$asset_id/") + $(s3_versions "originals/$user_id/${sha:0:2}/$sha") ))
    [ "$left" = 0 ] && break
    sleep 1
  done
  [ "$left" = 0 ] || fail "$left object versions / delete markers remain after the purge"
  echo "S3: no versions or delete markers left for the purged photo"
else
  echo "(E2E_S3_* not set: skipped the S3 version listing)"
fi
# Uploading the purged photo again is a fresh upload, not a revival.
req="$(python3 -c 'import json,sys; print(json.dumps({"sha256":sys.argv[1],"size_bytes":int(sys.argv[2]),"filename":"e2e.png"}))' "$sha" "$size")"
[ "$(post /assets/check "$req" | json 'd["status"]')" = upload ] || fail "re-upload of a purged photo should start a new upload"

step "empty trash"
post /assets/trash "$(ids_of "$asset2")" >/dev/null
[ "$(curl -fsS -b "$jar" "$BASE_URL/assets/trash" | json 'd["total"]')" = 1 ] || fail "trash total should be 1"
[ "$(curl -fsS -b "$jar" -X POST "$BASE_URL/assets/trash/empty" | json 'd["count"]')" = 1 ] || fail "empty: expected count 1"
[ "$(curl -fsS -b "$jar" -X POST "$BASE_URL/assets/trash/empty" | json 'd["count"]')" = 0 ] || fail "empty is not idempotent"
wait_gone "$orig2" || fail "second photo's original still downloadable after emptying the trash"
wait_gone "$thumb2" || fail "second photo's thumbnail still downloadable after emptying the trash"
if [ -n "$have_s3" ]; then
  [ "$(s3_versions "derived/$user_id/$asset2/")" = 0 ] || fail "derived versions remain for the emptied photo"
fi

echo "e2e OK"
