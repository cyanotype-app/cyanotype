#!/usr/bin/env bash
# Checks the security headers the gateway really sends (Referrer-Policy, Content-Security-Policy, nosniff, Permissions-Policy,
# COOP/CORP, HSTS), per route, with real Caddy containers: the production gateway (infra/caddy/Caddyfile.prod) in front of
# the real web Caddyfile (apps/web/Caddyfile) and a stub assets service that sets no-referrer on share API routes like the
# real one. Share pages and share API responses must be no-referrer; every other route keeps the default. The web UI gets
# the SPA policy, APIs and /cli/ the lock-everything one; a second gateway runs with CSP_MODE=report-only and extra origins.
# Needs docker; containers are named cy-ref-* and removed at exit.
set -uo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
image="${CADDY_IMAGE:-caddy:2-alpine}" # same image the compose files and the web Dockerfile use
port="${GATEWAY_TEST_PORT:-18093}"
port2=$((port + 1))
work="$(mktemp -d)"
names=(cy-ref-web cy-ref-assets cy-ref-gw cy-ref-gw2)
cleanup() {
  docker rm -f "${names[@]}" >/dev/null 2>&1
  docker network rm cy-ref-net >/dev/null 2>&1
  rm -rf "$work"
}
trap cleanup EXIT
cleanup_early() { docker rm -f "${names[@]}" >/dev/null 2>&1; docker network rm cy-ref-net >/dev/null 2>&1; }
cleanup_early

mkdir -p "$work/srv/cli"
echo '<html>spa</html>' > "$work/srv/index.html"
echo '{"version":"0"}' > "$work/srv/cli/latest.json"
cat > "$work/stub.caddy" <<'STUB'
{
	admin off
	auto_https off
}
:3002 {
	@pub path /assets/public/s/*
	header @pub Referrer-Policy no-referrer
	respond "assets-stub" 200
}
:3001 {
	respond "auth-stub" 200
}
STUB
chmod -R a+rX "$work"

docker network create cy-ref-net >/dev/null || exit 1
docker run -d --name cy-ref-web --network cy-ref-net --network-alias web \
  -v "$root/apps/web/Caddyfile:/etc/caddy/Caddyfile:ro" -v "$work/srv:/srv:ro" "$image" >/dev/null || exit 1
docker run -d --name cy-ref-assets --network cy-ref-net --network-alias assets --network-alias auth \
  -v "$work/stub.caddy:/etc/caddy/Caddyfile:ro" "$image" >/dev/null || exit 1
docker run -d --name cy-ref-gw --network cy-ref-net -e CYANOTYPE_HOSTNAME=http://:3000 -e CSP_S3_ORIGIN=https://s3.example.test \
  -p "127.0.0.1:$port:3000" \
  -v "$root/infra/caddy:/etc/caddy:ro" "$image" caddy run --config /etc/caddy/Caddyfile.prod --adapter caddyfile >/dev/null || exit 1
# The same gateway in report-only mode with extra origins.
docker run -d --name cy-ref-gw2 --network cy-ref-net -e CYANOTYPE_HOSTNAME=http://:3000 -e CSP_MODE=report-only \
  -e CSP_S3_ORIGIN=https://s3.example.test -e CSP_EXTRA_IMG_SRC=https://img.example.test -e CSP_EXTRA_CONNECT_SRC=https://api.example.test \
  -p "127.0.0.1:$port2:3000" \
  -v "$root/infra/caddy:/etc/caddy:ro" "$image" caddy run --config /etc/caddy/Caddyfile.prod --adapter caddyfile >/dev/null || exit 1

for p in "$port" "$port2"; do
  for _ in $(seq 1 30); do curl -sf -o /dev/null "http://127.0.0.1:$p/healthz" && break; sleep 1; done
done

fail=0
expect() { # expect PATH EXPECTED_REFERRER_POLICY: exactly one header with exactly that value
  local path="$1" want="$2" got
  got="$(curl -sI "http://127.0.0.1:$port$path" | tr -d '\r' | grep -i '^referrer-policy:' | sed 's/^[^:]*: *//')"
  if [ "$got" = "$want" ]; then echo "ok   - $path -> Referrer-Policy: $got"; else echo "FAIL - $path: want '$want', got '$got'"; fail=1; fi
}
expect /s/x no-referrer
expect /s no-referrer
expect /assets/public/s/x no-referrer
expect /assets/public/s/x/items no-referrer
expect /assets/me strict-origin-when-cross-origin
expect / strict-origin-when-cross-origin
expect /library strict-origin-when-cross-origin

# hdr PORT PATH NAME WANT: exactly one header NAME with exactly the value WANT (WANT=ABSENT: no such header at all).
hdr() {
  local pt="$1" path="$2" name="$3" want="$4" got n
  got="$(curl -sI "http://127.0.0.1:$pt$path" | tr -d '\r' | grep -i "^$name:" | sed 's/^[^:]*: *//')"
  n="$(printf '%s' "$got" | grep -c .)"
  if [ "$want" = ABSENT ]; then
    if [ "$n" = 0 ]; then echo "ok   - $path has no $name"; else echo "FAIL - $path: unexpected $name: $got"; fail=1; fi
  elif [ "$n" = 1 ] && [ "$got" = "$want" ]; then
    echo "ok   - $path -> $name"
  else
    echo "FAIL - $path $name: want '$want' (once), got $n x '$got'"
    fail=1
  fi
}
# The policies, spelled out here on purpose: a change to infra/caddy/csp.caddy has to be made here too.
# $1 img-src origins, $2 connect-src extras. The test hostname is plain http but Caddyfile.prod is the production file, so
# it appends upgrade-insecure-requests.
spa_csp() {
  echo "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self'; img-src 'self' data: $1; connect-src 'self' $2; worker-src 'self'; manifest-src 'self'; frame-src 'none'; object-src 'none'; base-uri 'self'; form-action 'self'; frame-ancestors 'none'; upgrade-insecure-requests"
}
api_csp="default-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'"
perms="accelerometer=(), autoplay=(), bluetooth=(), camera=(), display-capture=(), geolocation=(), gyroscope=(), hid=(), magnetometer=(), microphone=(), midi=(), payment=(), serial=(), usb=(), xr-spatial-tracking=()"

# Web UI: the SPA, a client-side route, a share page, the map.
for path in / /library /s/x /map /settings/security; do
  hdr "$port" "$path" Content-Security-Policy "$(spa_csp "https://s3.example.test 'self'" "'self'")"
done
# APIs, the CLI download and the health check.
for path in /assets/me /assets/public/s/x /auth/me /cli/latest.json /healthz; do
  hdr "$port" "$path" Content-Security-Policy "$api_csp"
done
# Everything else, on every kind of route.
for path in / /s/x /assets/me /auth/me /cli/latest.json /healthz; do
  hdr "$port" "$path" Content-Security-Policy-Report-Only ABSENT
  hdr "$port" "$path" X-Content-Type-Options nosniff
  hdr "$port" "$path" Permissions-Policy "$perms"
  hdr "$port" "$path" Cross-Origin-Opener-Policy same-origin
  hdr "$port" "$path" Cross-Origin-Resource-Policy same-origin
  hdr "$port" "$path" X-XSS-Protection ABSENT
done
hdr "$port" / Strict-Transport-Security "max-age=31536000"
# CSP_MODE=report-only: the same policies under the Report-Only name, nothing enforced; the extra origins are added.
hdr "$port2" / Content-Security-Policy-Report-Only "$(spa_csp "https://s3.example.test https://img.example.test" "https://api.example.test")"
hdr "$port2" / Content-Security-Policy ABSENT
hdr "$port2" /assets/me Content-Security-Policy-Report-Only "$api_csp"
hdr "$port2" /assets/me Content-Security-Policy ABSENT

if [ "$fail" != 0 ]; then docker logs cy-ref-gw 2>&1 | tail -20; fi
exit "$fail"
