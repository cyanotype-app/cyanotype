#!/usr/bin/env bash
# Installs the map's basemap: extracts a Protomaps PMTiles archive for the zoom levels you want and uploads it to the
# bucket, where the assets service serves it to the browser (docs/deploy.md, "Map"). Run it ON THE SERVER, in the
# deploy directory (default ~/cyanotype, where .env.prod with the bucket settings lives), not on your dev machine.
# The planet build is ~140 GB but only the needed byte ranges are downloaded.
#
#   ssh <server>
#   cd ~/cyanotype
#
#   scripts/fetch-basemap.sh                              # the world to zoom 7 -> basemap/world.pmtiles
#   scripts/fetch-basemap.sh --maxzoom 6                  # a smaller world
#   scripts/fetch-basemap.sh --detail --bbox=-10,35,30,60 # optional regional detail (here Europe) -> basemap/detail.pmtiles
#   scripts/fetch-basemap.sh --detail --bbox=... --maxzoom 12
#   scripts/fetch-basemap.sh --file my.pmtiles --as world # upload an archive you made yourself
#   scripts/fetch-basemap.sh --remove detail              # delete an installed archive
#
# Bucket settings come from the environment, or from ENV_FILE (default ./.env.prod when it exists):
#   S3_ENDPOINT  S3_BUCKET  S3_ACCESS_KEY  S3_SECRET_KEY  [S3_REGION, default us-east-1]
# Needs curl (7.75+, for --aws-sigv4), tar and sha256sum. The `pmtiles` tool is downloaded at a pinned version and
# checked against a pinned sha256 (cached in ~/.cache/cyanotype-basemap).
#
# Not pinned, and why: Protomaps publishes a rolling daily planet build and keeps only the last few days, so the
# newest available build is used (BUILD_DATE=YYYYMMDD picks one). Re-running replaces the archive; the map picks it up
# on the next page load. Data (c) OpenStreetMap contributors (ODbL), tiles by Protomaps.
set -euo pipefail

PMTILES_VERSION=1.31.2
PMTILES_SHA256_X86_64=3ed7dbf4ec2e6dfe5e25b6f70d1ffc932729f93c86db353bf514dd71010a312f
PMTILES_SHA256_ARM64=f8bd47e7ea866863489cad588fbaf2f31f42e5821f7a03f009b3769f05801cb1
BUILD_URL=${BUILD_URL:-https://build.protomaps.com}

die() { echo "error: $*" >&2; exit 1; }

kind=world maxzoom="" bbox="" file="" remove=""
while [ $# -gt 0 ]; do
  case "$1" in
    --detail) kind=detail ;;
    --maxzoom) maxzoom="${2:?--maxzoom needs a number}"; shift ;;
    --maxzoom=*) maxzoom="${1#*=}" ;;
    --bbox) bbox="${2:?--bbox needs west,south,east,north}"; shift ;;
    --bbox=*) bbox="${1#*=}" ;;
    --file) file="${2:?--file needs a path}"; shift ;;
    --as) kind="${2:?--as needs world or detail}"; shift ;;
    --remove) remove="${2:?--remove needs world or detail}"; shift ;;
    -h | --help) sed -n '2,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) die "unknown argument: $1 (see --help)" ;;
  esac
  shift
done
case "$kind" in world | detail) ;; *) die "--as must be world or detail" ;; esac
[ -z "$maxzoom" ] && { [ "$kind" = world ] && maxzoom=7 || maxzoom=12; }
case "$maxzoom" in '' | *[!0-9]*) die "--maxzoom must be a whole number" ;; esac
[ "$maxzoom" -le 15 ] || die "--maxzoom above 15 is not useful for a photo map and gets huge"
if [ "$kind" = detail ] && [ -z "$file" ] && [ -z "$bbox" ]; then
  die "--detail needs --bbox=west,south,east,north (a region; the whole planet at detail is far too large)"
fi

env_file="${ENV_FILE:-}"
[ -z "$env_file" ] && [ -f .env.prod ] && env_file=.env.prod
if [ -n "$env_file" ]; then
  [ -f "$env_file" ] || die "ENV_FILE $env_file not found"
  set -a
  # shellcheck disable=SC1090
  . "$env_file"
  set +a
fi
missing=""
for v in S3_ENDPOINT S3_BUCKET S3_ACCESS_KEY S3_SECRET_KEY; do
  [ -n "${!v:-}" ] || missing="$missing $v"
done
if [ -n "$missing" ]; then
  if [ -n "$env_file" ]; then
    where="$env_file was read but does not set:$missing"
  else
    where="no .env.prod in $PWD, and these are not exported:$missing"
  fi
  die "$where

This script is meant to run on the server, in the deploy directory (default ~/cyanotype), where .env.prod holds the
bucket settings. On the server:

  ssh <server>
  cd ~/cyanotype
  scripts/fetch-basemap.sh

Elsewhere, point it at an env file with the four settings:

  ENV_FILE=/path/to/.env.prod scripts/fetch-basemap.sh

or export them first:

  export S3_ENDPOINT=... S3_BUCKET=... S3_ACCESS_KEY=... S3_SECRET_KEY=...   # S3_REGION is optional"
fi
region="${S3_REGION:-us-east-1}"
endpoint="${S3_ENDPOINT%/}"
key="basemap/$kind.pmtiles"
url="$endpoint/$S3_BUCKET/$key"

s3curl() { # s3curl <curl args...>: a SigV4-signed request to the bucket
  curl -sS --fail-with-body --aws-sigv4 "aws:amz:$region:s3" --user "$S3_ACCESS_KEY:$S3_SECRET_KEY" \
    -H "x-amz-content-sha256: UNSIGNED-PAYLOAD" "$@"
}

if [ -n "$remove" ]; then
  case "$remove" in world | detail) ;; *) die "--remove takes world or detail" ;; esac
  s3curl -X DELETE -o /dev/null "$endpoint/$S3_BUCKET/basemap/$remove.pmtiles"
  echo "removed basemap/$remove.pmtiles"
  exit 0
fi

work="$(mktemp -d "${TMPDIR:-/tmp}/cyanotype-basemap.XXXXXX")"
trap 'rm -rf "$work"' EXIT
archive="$work/$kind.pmtiles"

if [ -n "$file" ]; then
  [ -f "$file" ] || die "$file not found"
  archive="$file"
else
  cache="${XDG_CACHE_HOME:-$HOME/.cache}/cyanotype-basemap"
  mkdir -p "$cache"
  case "$(uname -m)" in
    x86_64 | amd64) arch=x86_64 sum=$PMTILES_SHA256_X86_64 ;;
    aarch64 | arm64) arch=arm64 sum=$PMTILES_SHA256_ARM64 ;;
    *) die "no pinned pmtiles build for $(uname -m)" ;;
  esac
  tarball="go-pmtiles_${PMTILES_VERSION}_Linux_${arch}.tar.gz"
  tool="$cache/pmtiles-$PMTILES_VERSION-$arch"
  if [ ! -x "$tool" ]; then
    curl -sSfL --retry 3 -o "$work/$tarball" "https://github.com/protomaps/go-pmtiles/releases/download/v$PMTILES_VERSION/$tarball"
    echo "$sum  $work/$tarball" | sha256sum -c - >/dev/null || die "pmtiles download does not match the pinned sha256"
    tar -xzf "$work/$tarball" -C "$work" pmtiles
    install -m 755 "$work/pmtiles" "$tool"
  fi

  date_wanted="${BUILD_DATE:-}"
  if [ -z "$date_wanted" ]; then
    for back in 0 1 2 3 4 5 6 7 8 9 10 11 12 13; do
      d="$(date -u -d "-$back days" +%Y%m%d)"
      if curl -sfI -o /dev/null "$BUILD_URL/$d.pmtiles"; then date_wanted="$d"; break; fi
    done
    [ -n "$date_wanted" ] || die "no Protomaps build found in the last 14 days at $BUILD_URL (set BUILD_DATE=YYYYMMDD)"
  fi
  src="$BUILD_URL/$date_wanted.pmtiles"
  echo "extracting $kind (zoom 0-$maxzoom${bbox:+, bbox $bbox}) from build $date_wanted; this downloads only what is needed"
  args=(extract "$src" "$archive" "--maxzoom=$maxzoom")
  [ -n "$bbox" ] && args+=("--bbox=$bbox")
  "$tool" "${args[@]}"
fi

size="$(wc -c <"$archive" | tr -d ' ')"
[ "$size" -gt 0 ] || die "the archive is empty"
echo "uploading $((size / 1048576)) MiB to $S3_BUCKET/$key"
s3curl -X PUT -T "$archive" -H "Content-Type: application/octet-stream" -o /dev/null "$url"
got="$(s3curl -I "$url" | tr -d '\r' | awk 'tolower($1)=="content-length:"{print $2}')"
[ "$got" = "$size" ] || die "the bucket reports $got bytes for $key, expected $size"
echo "installed $key ($((size / 1048576)) MiB). Reload the map page; no restart needed."
