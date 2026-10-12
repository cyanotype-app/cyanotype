#!/usr/bin/env bash
# Download a few real camera RAW files (CC0, from https://raw.pixls.us) for the worker's sample tests.
#
#   scripts/fetch-raw-samples.sh [dir]            # default: target/raw-samples
#   CYANOTYPE_RAW_SAMPLES=target/raw-samples cargo test -p cyanotype-worker raw_samples -- --nocapture
#
# Files are verified against pinned sha256 sums; a mismatch aborts. They total about 90 MB, which is why
# they are fetched on demand instead of committed: the tests skip when CYANOTYPE_RAW_SAMPLES is unset,
# so plain `cargo test` and CI need none of this. raw.pixls.us samples are released under CC0 1.0.
set -euo pipefail

dir="${1:-target/raw-samples}"
base="https://raw.pixls.us/data"
mkdir -p "$dir"

# name | url path (under $base) | sha256
samples=(
  "APS-C_CRAW.CR3|Canon/Canon%20EOS%20R5m2/APS-C_CRAW.CR3|6a597497776a6abbfab8fcf12d0086ac7a3f8afea2879d61accf6bbf48d357d7"
  "DSC_0451.NEF|Nikon/1%20J2/DSC_0451.NEF|81051ee36135883acea55bd1e609d6d31212c0bf03640b7ae9fe177a1ca51219"
  "apsc_compressed.ARW|SONY/ILCE-7M5/apsc_compressed.ARW|8315af2a9889709f4c836dbaadc23d226847e5a0630fda212a9f896d5fe0569f"
  "IMGP4424.PEF|PENTAX/PENTAX%20K-r/IMGP4424.PEF|dd0d7b76d89039995d253adddcfb44d131414fad2179610d9ea4d8d4bb876b7b"
  "P1000336.RW2|Panasonic/DC-FZ45/P1000336.RW2|30c02f392d1d0dfba93cff429172b768225f11da2ce6a6b40a195611d8486954"
  "RAW_FUJI_E550.RAF|Fujifilm/FinePix%20E550/RAW_FUJI_E550.RAF|82d1010a1eb8cdf4b2de7ca30a3a4db4e96f290536f06ed2798a67c1fe81e6ae"
  "RAW_OLYMPUS_C5050Z.ORF|Olympus/C5050Z/RAW_OLYMPUS_C5050Z.ORF|f4b013e78204ebaadd740d93416080856b3c51ec154760e2e2de0e552873044a"
  "CANON_A410_CHDK_CRW_0741.DNG|Canon/Canon%20PowerShot%20A410/CANON_A410_CHDK_CRW_0741.DNG|f0d2fe47507fadf50008bddbc6bd2c5e39fddbe90cca6bca72dd360fa0e0eb38"
)

for entry in "${samples[@]}"; do
  IFS='|' read -r name path sum <<<"$entry"
  out="$dir/$name"
  if [ -f "$out" ] && echo "$sum  $out" | sha256sum -c --status -; then
    echo "ok (cached) $name"
    continue
  fi
  echo "fetching $name"
  curl -fsSL --retry 3 -o "$out.part" "$base/$path"
  if ! echo "$sum  $out.part" | sha256sum -c --status -; then
    rm -f "$out.part"
    echo "sha256 mismatch for $name" >&2
    exit 1
  fi
  mv "$out.part" "$out"
done
echo "done: $dir"
