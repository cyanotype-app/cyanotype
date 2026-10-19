#!/usr/bin/env bash
# Tests for the CLI downloads (docs/releases.md).
#
#   scripts/test-cli-dist.sh            part 1 only: apps/cli/dist/make-dist.sh with fake programs and a throwaway key
#   scripts/test-cli-dist.sh --docker   also part 2: bake the real `cli-dist` stage (unsigned: no key), check every file,
#                                       run the Linux program, and serve the result with the real web Caddyfile
# Part 2 needs docker buildx (and the build cache, CACHE=gha in CI); containers are named cy-clidist-* and removed at exit.
set -uo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
work="$(mktemp -d)"
fail=0
ok() { echo "ok   - $*"; }
bad() { echo "FAIL - $*"; fail=1; }
check() { # check DESCRIPTION CMD...
  local d="$1"; shift
  if "$@" >/dev/null 2>&1; then ok "$d"; else bad "$d"; fi
}
cleanup() { docker rm -f cy-clidist-web >/dev/null 2>&1; rm -rf "$work" "$root/cli-dist-out"; }
trap cleanup EXIT

targets=(x86_64-unknown-linux-musl aarch64-unknown-linux-musl x86_64-apple-darwin aarch64-apple-darwin x86_64-pc-windows-gnu)
files=(cyanotype-x86_64-unknown-linux-musl.gz cyanotype-aarch64-unknown-linux-musl.gz cyanotype-x86_64-apple-darwin.gz
  cyanotype-aarch64-apple-darwin.gz cyanotype-x86_64-pc-windows-gnu.exe.gz)

# ---- part 1: make-dist.sh ----------------------------------------------------------------------------------------
mkdir -p "$work/bin"
for t in "${targets[@]}"; do
  mkdir -p "$work/bin/$t"
  name=cyanotype; case "$t" in *windows*) name=cyanotype.exe ;; esac
  head -c 4096 /dev/urandom > "$work/bin/$t/$name"
done
ssh-keygen -q -t ed25519 -N '' -C test-only -f "$work/key" || { echo "ssh-keygen is needed" >&2; exit 1; }
ssh-keygen -q -t ed25519 -N '' -C other -f "$work/other" || exit 1
dist="$root/apps/cli/dist/make-dist.sh"

# Unsigned: no key.
if bash "$dist" --bin-dir "$work/bin" --out "$work/unsigned" --version 1.2.3 --sha abc1234 --min-cli 1.0.0 >/dev/null 2>&1; then ok "unsigned dist builds"; else bad "unsigned dist builds"; fi
check "unsigned dist is marked UNSIGNED" test -f "$work/unsigned/UNSIGNED"
check "unsigned dist has no signature" test ! -e "$work/unsigned/SHA256SUMS.sig"
check "unsigned latest.json says signed:false" grep -q '"signed": false' "$work/unsigned/latest.json"
check "unsigned install.sh has no public key" grep -q "^PUBKEY=''$" "$work/unsigned/install.sh"

# Signed: key + matching public key.
if bash "$dist" --bin-dir "$work/bin" --out "$work/signed" --version 1.2.3 --sha abc1234 --min-cli 1.0.0 \
     --key "$work/key" --pubkey "$work/key.pub" >/dev/null 2>&1; then ok "signed dist builds"; else bad "signed dist builds"; fi
for f in "${files[@]}" latest.json SHA256SUMS SHA256SUMS.sig install.sh install.ps1; do
  check "signed dist has $f" test -s "$work/signed/$f"
done
check "signed dist is not marked UNSIGNED" test ! -e "$work/signed/UNSIGNED"
check "signed latest.json says signed:true" grep -q '"signed": true' "$work/signed/latest.json"
check "SHA256SUMS matches the files" bash -c "cd '$work/signed' && sha256sum -c SHA256SUMS"
check "SHA256SUMS lists latest.json and every archive" bash -c "cd '$work/signed' && for f in latest.json ${files[*]}; do grep -q \"  \$f\$\" SHA256SUMS || exit 1; done"
printf 'release %s\n' "$(cat "$work/key.pub")" > "$work/allowed"
check "signature verifies with ssh-keygen" bash -c "ssh-keygen -Y verify -f '$work/allowed' -I release -n cyanotype-cli -s '$work/signed/SHA256SUMS.sig' < '$work/signed/SHA256SUMS'"
printf 'release %s\n' "$(cat "$work/other.pub")" > "$work/allowed-other"
check "signature fails against another key" bash -c "! ssh-keygen -Y verify -f '$work/allowed-other' -I release -n cyanotype-cli -s '$work/signed/SHA256SUMS.sig' < '$work/signed/SHA256SUMS'"
check "install.sh carries the public key" grep -qF "PUBKEY='$(cat "$work/key.pub")'" "$work/signed/install.sh"
check "install.sh is valid POSIX sh" sh -n "$work/signed/install.sh"
check "a key that is not the compiled-in public key is refused" bash -c "! bash '$dist' --bin-dir '$work/bin' --out '$work/mismatch' --version 1.2.3 --min-cli 1.0.0 --key '$work/key' --pubkey '$work/other.pub'"
check "a key without a public key is refused" bash -c "! bash '$dist' --bin-dir '$work/bin' --out '$work/nopub' --version 1.2.3 --min-cli 1.0.0 --key '$work/key' --pubkey /dev/null"
# A key pasted into a CI secret often loses its final newline.
head -c -1 "$work/key" > "$work/key-nonl"; chmod 600 "$work/key-nonl"
check "a key without a trailing newline still signs" bash "$dist" --bin-dir "$work/bin" --out "$work/nonl" --version 1.2.3 --min-cli 1.0.0 --key "$work/key-nonl" --pubkey "$work/key.pub"

if [ "${1:-}" != "--docker" ]; then
  [ "$fail" = 0 ] && echo "all dist script checks passed"
  exit "$fail"
fi

# ---- part 2: the real stage and the real Caddyfile ---------------------------------------------------------------------
cd "$root" || exit 1
out="$root/cli-dist-out" # the target's own output directory, inside the checkout (ignored by Docker and version control)
rm -rf "$out"
if docker buildx bake -f docker-bake.hcl cli-dist >"$work/bake.log" 2>&1; then
  ok "docker buildx bake cli-dist"
else
  bad "docker buildx bake cli-dist"; tail -40 "$work/bake.log"; exit 1
fi
for f in "${files[@]}" latest.json SHA256SUMS install.sh install.ps1; do check "real dist has $f" test -s "$out/$f"; done
check "unsigned build (no key) is marked UNSIGNED and has no .sig" bash -c "test -f '$out/UNSIGNED' && test ! -e '$out/SHA256SUMS.sig'"
check "SHA256SUMS matches the files" bash -c "cd '$out' && sha256sum -c SHA256SUMS"
check "latest.json is valid and lists all five targets" python3 -I -c "
import json,sys
m=json.load(open('$out/latest.json'))
assert m['schema']==1 and m['signed'] is False and m['min_cli_version'] and m['version']
assert sorted(m['targets'])==sorted('${targets[*]}'.split()), m['targets'].keys()
for t in m['targets'].values(): assert t['file'] and len(t['sha256'])==64 and t['size']>1000
"
# Programs are what they claim to be.
unz() { gzip -dc "$out/$1" | head -c "$2" | od -An -tx1 | tr -d ' \n'; }
check "linux x86_64 is an ELF" test "$(unz cyanotype-x86_64-unknown-linux-musl.gz 4)" = 7f454c46
check "linux aarch64 is an ELF" test "$(unz cyanotype-aarch64-unknown-linux-musl.gz 4)" = 7f454c46
check "linux aarch64 is machine type 0xb7" test "$(gzip -dc "$out/cyanotype-aarch64-unknown-linux-musl.gz" | head -c 20 | tail -c 2 | od -An -tx1 | tr -d ' \n')" = b700
check "macOS x86_64 is a Mach-O" test "$(unz cyanotype-x86_64-apple-darwin.gz 4)" = cffaedfe
check "macOS aarch64 is a Mach-O" test "$(unz cyanotype-aarch64-apple-darwin.gz 4)" = cffaedfe
check "windows is a PE (MZ)" test "$(unz cyanotype-x86_64-pc-windows-gnu.exe.gz 2)" = 4d5a
gzip -dc "$out/cyanotype-x86_64-unknown-linux-musl.gz" > "$work/cyanotype"; chmod +x "$work/cyanotype"
if ver="$("$work/cyanotype" --version 2>&1)"; then ok "linux program runs: $ver"; else bad "linux program runs: $ver"; fi
check "linux program is statically linked" bash -c "! ldd '$work/cyanotype' 2>&1 | grep -q 'libc.so'"
check "the downloadable CLI has no server tools (migrate)" bash -c "! '$work/cyanotype' --help | grep -qw migrate"
check "the downloadable CLI has update and import" bash -c "'$work/cyanotype' --help | grep -qw update && '$work/cyanotype' --help | grep -qw import"
check "update refuses an unsigned or dev build" bash -c "! '$work/cyanotype' update --server http://127.0.0.1:9 2>&1 | grep -q Updated"

# The web Caddyfile serves it: install scripts get this server's address, nothing falls through to the SPA shell.
mkdir -p "$work/srv/cli"
cp -r "$out"/. "$work/srv/cli/"
echo '<html>spa</html>' > "$work/srv/index.html"
chmod -R a+rX "$work/srv"
port="${CLI_DIST_TEST_PORT:-18094}"
docker rm -f cy-clidist-web >/dev/null 2>&1
docker run -d --name cy-clidist-web -p "127.0.0.1:$port:8080" -v "$root/apps/web/Caddyfile:/etc/caddy/Caddyfile:ro" \
  -v "$work/srv:/srv:ro" "${CADDY_IMAGE:-caddy:2-alpine}" >/dev/null || exit 1
for _ in $(seq 1 30); do curl -sf -o /dev/null "http://127.0.0.1:$port/" && break; sleep 1; done
get() { curl -s "http://127.0.0.1:$port$1" "${@:2}"; }
check "/cli/latest.json is served as JSON" bash -c "curl -sI http://127.0.0.1:$port/cli/latest.json | tr -d '\r' | grep -qi '^content-type: application/json'"
check "/cli/SHA256SUMS is served as text" bash -c "curl -sI http://127.0.0.1:$port/cli/SHA256SUMS | tr -d '\r' | grep -qi '^content-type: text/plain'"
check "/cli/ metadata is never cached without revalidation" bash -c "curl -sI http://127.0.0.1:$port/cli/latest.json | tr -d '\r' | grep -qi '^cache-control: no-cache'"
check "a missing /cli file is a 404, not the SPA shell" bash -c "test \"\$(curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:$port/cli/nope.sh)\" = 404"
check "the archive downloads intact" bash -c "curl -s http://127.0.0.1:$port/cli/cyanotype-x86_64-unknown-linux-musl.gz | cmp - '$out/cyanotype-x86_64-unknown-linux-musl.gz'"
script="$(get /cli/install.sh -H 'Host: photos.example.org' -H 'X-Forwarded-Proto: https')"
if printf '%s\n' "$script" | grep -q "^SERVER_URL='https://photos.example.org'$"; then ok "install.sh is served with this server's address"; else bad "install.sh server address: $(printf '%s\n' "$script" | grep '^SERVER_URL' )"; fi
check "the served install.sh is valid sh" bash -c "curl -s http://127.0.0.1:$port/cli/install.sh | sh -n"
ps1="$(get /cli/install.ps1 -H 'Host: photos.example.org' -H 'X-Forwarded-Proto: https')"
if printf '%s\n' "$ps1" | grep -q "^\$ServerUrl = 'https://photos.example.org'$"; then ok "install.ps1 is served with this server's address"; else bad "install.ps1 server address"; fi
# End to end: the served install.sh installs the served program (checksum verified) into a scratch directory.
if CYANOTYPE_SERVER="http://127.0.0.1:$port" sh -c "curl -fsSL http://127.0.0.1:$port/cli/install.sh | sh -s -- --dir '$work/inst'" >"$work/install.log" 2>&1; then
  ok "curl | sh installs"; else bad "curl | sh installs"; cat "$work/install.log"; fi
check "installed program runs" "$work/inst/cyanotype" --version
check "installer printed the login step with the server" grep -q "cyanotype login http://127.0.0.1:$port" "$work/install.log"
check "installer warned about PATH" grep -q "not on your PATH" "$work/install.log"
check "installer says the build is unsigned" grep -qi "not signed" "$work/install.log"
# A tampered download is refused.
cp "$work/srv/cli/cyanotype-x86_64-unknown-linux-musl.gz" "$work/good.gz"
printf 'x' >> "$work/srv/cli/cyanotype-x86_64-unknown-linux-musl.gz"
if CYANOTYPE_SERVER="http://127.0.0.1:$port" sh -c "curl -fsSL http://127.0.0.1:$port/cli/install.sh | sh -s -- --dir '$work/inst2'" >"$work/install2.log" 2>&1; then
  bad "a tampered archive is refused"; else ok "a tampered archive is refused"; fi
check "nothing was installed from the tampered archive" test ! -e "$work/inst2/cyanotype"

[ "$fail" = 0 ] && echo "all CLI dist checks passed"
exit "$fail"
