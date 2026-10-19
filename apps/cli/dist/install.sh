#!/bin/sh
# Installs the `cyanotype` command line from your Cyanotype server.
#
#   curl -fsSL https://<your server>/cli/install.sh | sh
#   curl -fsSL https://<your server>/cli/install.sh | sh -s -- --system     # /usr/local/bin (may ask for sudo)
#
# Options: --system  install to /usr/local/bin instead of ~/.local/bin
#          --dir DIR install to DIR
# The server fills in its own address below when it serves this file. Set CYANOTYPE_SERVER to override.
#
# What it does: detects your OS and CPU, downloads the matching build and SHA256SUMS from the same server,
# checks the checksum (and the signature when ssh-keygen is available), installs, and tells you the next step.
# Nothing else is touched. The `cyanotype` program can later replace itself with `cyanotype update`.
set -eu

# Filled in by the server (infra: apps/web/Caddyfile). Left as is when the file is not served by Cyanotype.
SERVER_URL='{{ or (.Req.Header.Get "X-Forwarded-Proto") "http" }}://{{ .Host }}'
# Release public key (SSH ed25519) of this build of the server; empty for an unsigned build.
PUBKEY='@@PUBKEY@@'

say() { printf '%s\n' "$*" >&2; }
die() { say "error: $*"; exit 1; }

dir=""
system=0
while [ $# -gt 0 ]; do
  case "$1" in
    --system) system=1 ;;
    --dir) shift; [ $# -gt 0 ] || die "--dir needs a directory"; dir="$1" ;;
    -h|--help) sed -n '2,12p' "$0" >&2; exit 0 ;;
    *) die "unknown option: $1" ;;
  esac
  shift
done

server="${CYANOTYPE_SERVER:-$SERVER_URL}"
case "$server" in
  https://*|http://*) ;;
  *) die "do not know which server to download from: set CYANOTYPE_SERVER=https://your-server and run again" ;;
esac
server="${server%/}"
# Only a plain URL; the value ends up in commands below.
case "$server" in
  *[!A-Za-z0-9.:/_-]*) die "unexpected characters in the server address" ;;
esac

# ---- what are we running on -------------------------------------------------------------------------------------
os="$(uname -s)"
arch="$(uname -m)"
case "$os" in
  Linux)  os_part="unknown-linux-musl" ;;
  Darwin) os_part="apple-darwin" ;;
  *) die "unsupported system '$os'. On Windows use install.ps1 ($server/cli/install.ps1)." ;;
esac
case "$arch" in
  x86_64|amd64)  arch_part="x86_64" ;;
  aarch64|arm64) arch_part="aarch64" ;;
  *) die "unsupported CPU '$arch' (builds exist for x86_64 and aarch64)" ;;
esac
target="$arch_part-$os_part"
file="cyanotype-$target.gz"

# ---- tools ------------------------------------------------------------------------------------------------------
if command -v curl >/dev/null 2>&1; then
  fetch() { curl -fsSL --retry 3 -o "$2" "$1"; }
elif command -v wget >/dev/null 2>&1; then
  fetch() { wget -q -O "$2" "$1"; }
else
  die "need curl or wget"
fi
command -v gzip >/dev/null 2>&1 || die "need gzip"
if command -v sha256sum >/dev/null 2>&1; then
  sha256() { sha256sum "$1" | cut -d' ' -f1; }
elif command -v shasum >/dev/null 2>&1; then
  sha256() { shasum -a 256 "$1" | cut -d' ' -f1; }
else
  die "need sha256sum or shasum"
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT INT TERM

say "Downloading cyanotype for $target from $server ..."
fetch "$server/cli/SHA256SUMS" "$tmp/SHA256SUMS" || die "could not download $server/cli/SHA256SUMS (does this server publish the CLI?)"
fetch "$server/cli/$file" "$tmp/$file" || die "could not download $server/cli/$file (no build for $target?)"

# ---- verify -----------------------------------------------------------------------------------------------------
want="$(awk -v f="$file" '{ n=$2; sub(/^\*/, "", n); if (n == f) print $1 }' "$tmp/SHA256SUMS")"
[ -n "$want" ] || die "$file is not listed in SHA256SUMS"
got="$(sha256 "$tmp/$file")"
[ "$got" = "$want" ] || die "checksum mismatch for $file (expected $want, got $got); nothing was installed"
say "Checksum OK."

if fetch "$server/cli/SHA256SUMS.sig" "$tmp/SHA256SUMS.sig" 2>/dev/null; then
  if [ -z "$PUBKEY" ] || [ "${PUBKEY#ssh-ed25519 }" = "$PUBKEY" ]; then
    say "Signature not checked: this server did not publish a release key."
  elif command -v ssh-keygen >/dev/null 2>&1; then
    printf 'release %s\n' "$PUBKEY" > "$tmp/allowed"
    if ssh-keygen -Y verify -f "$tmp/allowed" -I release -n cyanotype-cli -s "$tmp/SHA256SUMS.sig" < "$tmp/SHA256SUMS" >/dev/null 2>&1; then
      say "Signature OK."
    else
      die "the signature on SHA256SUMS does not verify; nothing was installed"
    fi
  else
    say "Signature not checked (ssh-keygen not found). The checksum came from the same server as the download."
  fi
else
  say "Note: this server's build is not signed. 'cyanotype update' will refuse to update it."
fi

# ---- install ----------------------------------------------------------------------------------------------------
sudo=""
if [ -n "$dir" ]; then
  dest="$dir"
elif [ "$system" = 1 ]; then
  dest="/usr/local/bin"
else
  [ -n "${HOME:-}" ] || die "HOME is not set; pass --dir"
  dest="$HOME/.local/bin"
fi
if [ ! -d "$dest" ]; then mkdir -p "$dest" 2>/dev/null || { [ "$system" = 1 ] && sudo mkdir -p "$dest"; } || die "cannot create $dest"; fi
if [ ! -w "$dest" ]; then
  if [ "$system" = 1 ] && command -v sudo >/dev/null 2>&1; then sudo="sudo"; else die "$dest is not writable (try --dir DIR, or --system with sudo)"; fi
fi

gzip -dc "$tmp/$file" > "$tmp/cyanotype"
chmod 755 "$tmp/cyanotype"
# Write beside the destination, then rename: a running cyanotype is never half-replaced.
$sudo cp "$tmp/cyanotype" "$dest/.cyanotype.new.$$"
$sudo mv -f "$dest/.cyanotype.new.$$" "$dest/cyanotype"
say "Installed $("$dest/cyanotype" --version 2>/dev/null || echo cyanotype) to $dest/cyanotype"

case ":${PATH:-}:" in
  *":$dest:"*) ;;
  *) say ""; say "Warning: $dest is not on your PATH. Add it, for example:"; say "  export PATH=\"$dest:\$PATH\"   # put this line in ~/.profile or your shell's rc file" ;;
esac

say ""
say "Next step:"
say "  cyanotype login $server"
