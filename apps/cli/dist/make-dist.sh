#!/usr/bin/env bash
# Assemble the CLI download directory the web image serves under /cli/ (docs/releases.md).
# Runs inside the Dockerfile's `cli-dist` stage; also runnable by hand to try it.
#
#   make-dist.sh --bin-dir DIR --out DIR --version X.Y.Z --sha GITSHA --min-cli X.Y.Z [--key FILE --pubkey FILE]
#
# DIR/<rust-target>/cyanotype[.exe] are the built programs. Output (all under --out):
#   cyanotype-<target>.gz[...]   one gzip per target (cyanotype-x86_64-pc-windows-gnu.exe.gz for Windows)
#   latest.json                  version, minimum CLI version, per-target file + sha256 + size, "signed"
#   SHA256SUMS, SHA256SUMS.sig   checksums of the .gz files and latest.json, and their SSH signature
#                                (only with --key; without it the dist is marked unsigned: file UNSIGNED, signed:false)
#   install.sh, install.ps1      the install scripts (served with the server's URL filled in by the web Caddyfile)
#
# The private key file is only read by ssh-keygen; its path and content are never printed. The signature is
# checked against --pubkey (the key compiled into the CLI) and the build fails if they do not match: a dist that
# `cyanotype update` would reject is worse than no dist.
set -euo pipefail

bin_dir=""; out=""; version=""; sha=""; min_cli=""; key=""; pubkey=""
while [ $# -gt 0 ]; do
  case "$1" in
    --bin-dir) bin_dir="$2"; shift 2 ;;
    --out) out="$2"; shift 2 ;;
    --version) version="$2"; shift 2 ;;
    --sha) sha="$2"; shift 2 ;;
    --min-cli) min_cli="$2"; shift 2 ;;
    --key) key="$2"; shift 2 ;;
    --pubkey) pubkey="$2"; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
: "${bin_dir:?--bin-dir}" "${out:?--out}" "${version:?--version}" "${min_cli:?--min-cli}"
here="$(cd "$(dirname "$0")" && pwd)"
sha="${sha:-unknown}"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] || { echo "bad version: $version" >&2; exit 2; }
[[ "$min_cli" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "bad minimum version: $min_cli" >&2; exit 2; }
[[ "$sha" =~ ^[0-9A-Za-z]+$ ]] || { echo "bad sha: $sha" >&2; exit 2; }

mkdir -p "$out"
rm -f "$out"/*
targets=()
for d in "$bin_dir"/*/; do targets+=("$(basename "$d")"); done
[ "${#targets[@]}" -gt 0 ] || { echo "no targets under $bin_dir" >&2; exit 1; }

signed=false
if [ -n "$key" ] && [ -s "$key" ]; then signed=true; fi

json_targets=""
for t in "${targets[@]}"; do
  src="$bin_dir/$t/cyanotype"; name="cyanotype-$t"
  if [ -f "$src.exe" ]; then src="$src.exe"; name="$name.exe"; fi
  [ -f "$src" ] || { echo "missing program for $t" >&2; exit 1; }
  gzip -9n -c "$src" > "$out/$name.gz"
  h="$(sha256sum "$out/$name.gz" | cut -d' ' -f1)"
  size="$(stat -c %s "$out/$name.gz")"
  json_targets+="${json_targets:+,}
    \"$t\": { \"file\": \"$name.gz\", \"sha256\": \"$h\", \"size\": $size }"
done

cat > "$out/latest.json" <<JSON
{
  "schema": 1,
  "version": "$version",
  "git_sha": "$sha",
  "signed": $signed,
  "min_cli_version": "$min_cli",
  "min_server_api": 1,
  "targets": {$json_targets
  }
}
JSON

# The public key (never the private one) is put into the install scripts so they can check the signature too.
pub_line=""
if [ -s "${pubkey:-/nonexistent}" ]; then pub_line="$(grep -m1 '^ssh-ed25519 ' "$pubkey" || true)"; fi
if [ "$signed" != true ]; then pub_line=""; fi
for f in install.sh install.ps1; do
  sed "s|@@PUBKEY@@|$pub_line|g" "$here/$f" > "$out/$f"
done

# The signed list: every file a CLI downloads and verifies. Not the install scripts: the web server fills the
# server URL into them on the fly, so the bytes served differ from these.
(cd "$out" && sha256sum latest.json cyanotype-*.gz > SHA256SUMS)

if [ "$signed" = true ]; then
  [ -s "${pubkey:-/nonexistent}" ] && grep -q '^ssh-ed25519 ' "$pubkey" \
    || { echo "a release key was given but $pubkey holds no ssh-ed25519 public key (docs/releases.md)" >&2; exit 1; }
  # ssh-keygen -Y sign needs a passphrase-less key file readable only by us (BuildKit mounts secrets as 0600).
  # A copy that ends in a newline (a key pasted into a CI secret often loses it), 0600, gone when the script exits.
  tmpkey="$(mktemp)"; trap 'rm -f "$tmpkey"' EXIT; chmod 600 "$tmpkey"
  cat "$key" > "$tmpkey"; [ -z "$(tail -c1 "$tmpkey")" ] || echo >> "$tmpkey"
  (cd "$out" && ssh-keygen -q -Y sign -f "$tmpkey" -n cyanotype-cli SHA256SUMS)
  allowed="$(mktemp)"
  { printf 'release '; grep -m1 '^ssh-ed25519 ' "$pubkey"; } > "$allowed"
  if ! ssh-keygen -Y verify -f "$allowed" -I release -n cyanotype-cli -s "$out/SHA256SUMS.sig" < "$out/SHA256SUMS" >/dev/null 2>&1; then
    rm -f "$allowed" "$out/SHA256SUMS.sig"
    echo "the release key does not match apps/cli/release-key.pub: refusing to build a dist that 'cyanotype update' would reject" >&2
    exit 1
  fi
  rm -f "$allowed"
  echo "dist $version ($sha): signed, ${#targets[@]} targets"
else
  printf 'This CLI dist is NOT signed: it was built without the release key.\n`cyanotype update` refuses it; install.sh installs it without signature verification.\n' > "$out/UNSIGNED"
  echo "dist $version ($sha): UNSIGNED (no release key), ${#targets[@]} targets"
fi
(cd "$out" && ls -l)
