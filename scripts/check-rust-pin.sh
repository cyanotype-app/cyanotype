#!/usr/bin/env bash
# Fails if the Dockerfile's Rust image is not the version pinned in rust-toolchain.toml.
set -euo pipefail
cd "$(dirname "$0")/.."
pin=$(sed -n 's/^channel *= *"\(.*\)"/\1/p' rust-toolchain.toml)
if ! grep -Eq "^ARG RUST_IMAGE=.*-rust-${pin//./\\.}-trixie\$" Dockerfile; then
  echo "::error::Dockerfile RUST_IMAGE does not match rust-toolchain.toml channel $pin" >&2
  exit 1
fi
echo "Dockerfile matches Rust $pin"
