#!/usr/bin/env bash
# Checks the Referrer-Policy the gateway really sends, with real Caddy containers: the production gateway
# (infra/caddy/Caddyfile.prod) in front of the real web Caddyfile (apps/web/Caddyfile) and a stub assets service
# that sets no-referrer on share API routes like the real one. Share pages and share API responses must be
# no-referrer; every other route keeps the default. Needs docker; containers are named cy-ref-* and removed at exit.
set -uo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
image="${CADDY_IMAGE:-caddy:2-alpine}" # same image the compose files and the web Dockerfile use
port="${GATEWAY_TEST_PORT:-18093}"
work="$(mktemp -d)"
names=(cy-ref-web cy-ref-assets cy-ref-gw)
cleanup() {
  docker rm -f "${names[@]}" >/dev/null 2>&1
  docker network rm cy-ref-net >/dev/null 2>&1
  rm -rf "$work"
}
trap cleanup EXIT
cleanup_early() { docker rm -f "${names[@]}" >/dev/null 2>&1; docker network rm cy-ref-net >/dev/null 2>&1; }
cleanup_early

mkdir -p "$work/srv"
echo '<html>spa</html>' > "$work/srv/index.html"
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
docker run -d --name cy-ref-gw --network cy-ref-net -e CYANOTYPE_HOSTNAME=http://:3000 -p "127.0.0.1:$port:3000" \
  -v "$root/infra/caddy:/etc/caddy:ro" "$image" caddy run --config /etc/caddy/Caddyfile.prod --adapter caddyfile >/dev/null || exit 1

for _ in $(seq 1 30); do curl -sf -o /dev/null "http://127.0.0.1:$port/healthz" && break; sleep 1; done

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
if [ "$fail" != 0 ]; then docker logs cy-ref-gw 2>&1 | tail -20; fi
exit "$fail"
