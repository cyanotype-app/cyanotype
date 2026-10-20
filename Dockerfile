# syntax=docker/dockerfile:1.7
#
# One shared builder compiles the whole workspace once; each service image is a tiny
# runtime stage that copies a single binary out of it.
#
#   docker buildx bake                    # all four images (builder runs once)
#   docker build --target assets -t cyanotype-assets .
#
# Targets: auth | assets | worker | cli.   See docs/build.md.

# Keep the Rust version in step with rust-toolchain.toml (CI checks this; docs/build.md).
ARG RUST_IMAGE=lukemathwalker/cargo-chef:0.1.78-rust-1.99.0-trixie

# ---- chef: Rust toolchain + cargo-chef + the C toolchain -------------------------------
# The worker's libwebp (webp / libwebp-sys crates) is compiled from bundled C sources and linked
# statically, and so is libheif (libheif-rs "embedded-libheif": the libheif 1.23.1 sources vendored in
# the libheif-sys crate, built with cmake), so build-essential + cmake are needed here and nowhere else.
# libheif's HEVC (HEIC) and AV1 (AVIF) decoders are separate libraries it links dynamically:
# libde265-dev and libdav1d-dev here, libde265-0 and libdav1d7 in the worker runtime image.
FROM ${RUST_IMAGE} AS chef
RUN apt-get update \
 && apt-get install -y --no-install-recommends build-essential cmake pkg-config libde265-dev libdav1d-dev \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /src

# ---- planner: reduce the workspace to a dependency "recipe" ----------------------------
# recipe.json only changes when manifests/Cargo.lock change, so the cook layer below is
# reused across any source-only edit.
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# ---- builder: compile dependencies (cached), then the workspace, exactly once ----------
FROM chef AS builder
COPY --from=planner /src/recipe.json recipe.json
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    cargo chef cook --release --workspace --locked --recipe-path recipe.json
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    cargo build --release --workspace --locked \
 && mkdir /out \
 && for b in auth assets worker; do cp "target/release/cyanotype-$b" /out/; done \
 && cp target/release/cyanotype /out/cyanotype-cli

# ---- search model files, pinned by sha256 in models/*.lock --------------------------------
# One stage per consumer, so each image carries only its own tower: the worker the image tower, the assets service the text
# tower and tokenizer. The layers depend only on the lock files and the scripts, never on source code, so they are rebuilt
# (and pulled to the server) only when a model changes. The same script verifies the checksums for a developer checkout
# (`just models`). Only the default model (SigLIP 2) goes into the worker. The assets image also carries CLIP's small text
# tower (66 MB), for one release: while a library is re-indexed with SigLIP 2, searches are answered with the CLIP vectors it
# already has (docs/library.md, "Changing the search model"). Drop the `clip-text` lines once nobody is upgrading from CLIP.
FROM debian:trixie-slim AS model-fetch
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl \
 && rm -rf /var/lib/apt/lists/*
COPY scripts/fetch-models.sh /usr/local/bin/fetch-models
COPY models/clip.lock models/siglip2.lock models/ocr.lock /etc/models/

# The SigLIP 2 text tower is 283 MB, 197 MB of it one embedding table of which a query reads a few rows. With the table
# moved to a side file ONNX Runtime maps it instead of copying it onto the heap (524 MB resident against about 120 MB);
# `onnx-split` does that to the downloaded file (crates/cyanotype-clip/src/split.rs, docs/build.md). It is one std-only
# Rust file, compiled here on its own so this layer does not depend on the workspace.
FROM chef AS onnx-split
COPY tools/onnx_split.rs /src/tools/onnx_split.rs
COPY crates/cyanotype-clip/src/split.rs /src/crates/cyanotype-clip/src/split.rs
RUN rustc --edition 2024 -O -o /onnx-split tools/onnx_split.rs

FROM model-fetch AS siglip2-vision
RUN LOCK=/etc/models/siglip2.lock fetch-models /models vision_model_q4.onnx

FROM model-fetch AS siglip2-text
COPY --from=onnx-split /onnx-split /usr/local/bin/onnx-split
RUN LOCK=/etc/models/siglip2.lock fetch-models /models text_model_int8.onnx tokenizer.json \
 && onnx-split /models/text_model_int8.onnx

FROM model-fetch AS clip-text
RUN LOCK=/etc/models/clip.lock fetch-models /models text_model_int8.onnx tokenizer.json

# Text in photos (PP-OCRv6 small, 31 MB): detection, recognition and the recognition model's dictionary (its yml), for the worker only.
FROM model-fetch AS ocr
RUN LOCK=/etc/models/ocr.lock fetch-models /models det.onnx rec.onnx rec.yml

# ---- runtime base: slim, non-root, CA certs for S3/TLS ---------------------------------
FROM debian:trixie-slim AS runtime
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --uid 10001 --no-create-home cyanotype
USER cyanotype
# Logs are JSON on stdout (see docs/observability.md).

FROM runtime AS auth
COPY --from=builder /out/cyanotype-auth /usr/local/bin/app
ENTRYPOINT ["/usr/local/bin/app"]

# The assets service embeds search queries with the text tower (ONNX Runtime is linked statically into the binary; the
# C++ runtime it needs, libstdc++6, is in the trixie base image). The model is loaded on the first search.
FROM runtime AS assets
COPY --from=siglip2-text /models /opt/cyanotype/models/siglip2
COPY --from=clip-text /models /opt/cyanotype/models/clip
COPY --from=builder /out/cyanotype-assets /usr/local/bin/app
ENTRYPOINT ["/usr/local/bin/app"]

# libwebp and libheif are statically linked into the binary. The extra runtime packages are the codec
# libraries libheif was linked against in the builder: libde265 (HEVC/HEIC), dav1d (AV1/AVIF), libjpeg and
# OpenJPEG (libheif's JPEG-in-HEIF and JPEG 2000 decoders, found in the rust builder image). About 3 MB.
# No libheif1 and no plugin packages: plugin loading is compiled out, so the decoder set is fixed at build
# time. The ldd check fails the build if the builder ever links something this stage does not install.
FROM runtime AS worker
USER root
RUN apt-get update \
 && apt-get install -y --no-install-recommends libde265-0 libdav1d7 libjpeg62-turbo libopenjp2-7 \
 && rm -rf /var/lib/apt/lists/*
COPY --from=siglip2-vision /models /opt/cyanotype/models/siglip2
COPY --from=ocr /models /opt/cyanotype/models/ocr
COPY --from=builder /out/cyanotype-worker /usr/local/bin/app
RUN if ldd /usr/local/bin/app | grep "not found"; then echo "worker has unresolved shared libraries (see above): add their packages to this stage" >&2; exit 1; fi
USER cyanotype
ENTRYPOINT ["/usr/local/bin/app"]

FROM runtime AS cli
COPY --from=builder /out/cyanotype-cli /usr/local/bin/app
ENTRYPOINT ["/usr/local/bin/app"]

# ==== cli-dist: the downloadable `cyanotype` CLI for every platform (docs/releases.md) ===================================
# The web image serves the result under /cli/. Cross-compiled with cargo-zigbuild (zig is the C compiler and linker, so one
# Linux builder produces Linux musl, macOS and Windows programs; no per-OS runners, no Apple SDK: the CLI links only libSystem
# on macOS because reqwest uses rustls with bundled roots, not Security.framework). The CLI is built with
# --no-default-features: no sqlx, no AWS SDK, no image decoding, nothing native besides rustls' ring.
#
#   docker buildx bake cli-dist                                   # unsigned dist (marked UNSIGNED), into ./cli-dist-out
#   CLI_RELEASE_KEY="$(cat release_key)" docker buildx bake cli-dist --set '*.secrets=id=cli_release_key,env=CLI_RELEASE_KEY'
#
# The signing key only ever exists as a BuildKit secret mount in the last RUN of cli-dist-assemble: not in a layer, a build arg
# or a log. Builds without it (pull requests, local) produce an unsigned dist that `cyanotype update` refuses.

# Pinned build tools (released more than 14 days before they were pinned), each verified against a sha256 below.
#   zig 0.16.0:        https://ziglang.org/download/index.json  (x86_64-linux)
#   cargo-zigbuild:    https://github.com/rust-cross/cargo-zigbuild/releases/tag/v0.23.4  (x86_64-unknown-linux-musl)
# The cross-compilation standard libraries come from rustup (verified by rustup against the signed Rust manifest).
FROM chef AS cli-tools
ARG ZIG_VERSION=0.16.0
ARG ZIG_SHA256=70e49664a74374b48b51e6f3fdfbf437f6395d42509050588bd49abe52ba3d00
ARG ZIGBUILD_VERSION=0.23.4
ARG ZIGBUILD_SHA256=9e3cf73485edbd45905c8aadbc0fdf869c7ddc3848f0c898229f2680db52e44b
RUN apt-get update \
 && apt-get install -y --no-install-recommends curl xz-utils \
 && rm -rf /var/lib/apt/lists/* \
 && test "$(uname -m)" = x86_64 || { echo "cli-dist is built on x86_64 hosts" >&2; exit 1; }
RUN set -eux; \
    curl -fsSL -o /tmp/zig.tar.xz "https://ziglang.org/download/${ZIG_VERSION}/zig-x86_64-linux-${ZIG_VERSION}.tar.xz"; \
    echo "${ZIG_SHA256}  /tmp/zig.tar.xz" | sha256sum -c -; \
    mkdir /opt/zig; tar -xJf /tmp/zig.tar.xz -C /opt/zig --strip-components=1; rm /tmp/zig.tar.xz; \
    curl -fsSL -o /tmp/zb.tar.xz "https://github.com/rust-cross/cargo-zigbuild/releases/download/v${ZIGBUILD_VERSION}/cargo-zigbuild-x86_64-unknown-linux-musl.tar.xz"; \
    echo "${ZIGBUILD_SHA256}  /tmp/zb.tar.xz" | sha256sum -c -; \
    tar -xJf /tmp/zb.tar.xz -C /usr/local/bin --strip-components=1 cargo-zigbuild-x86_64-unknown-linux-musl/cargo-zigbuild; rm /tmp/zb.tar.xz
ENV PATH="/opt/zig:${PATH}" ZIG_GLOBAL_CACHE_DIR=/zig-cache
RUN rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl x86_64-apple-darwin aarch64-apple-darwin x86_64-pc-windows-gnu \
 && zig version && cargo zigbuild --help >/dev/null

# Dependencies per target, one layer each (cache-friendly, and only one target compiles at a time, which bounds memory).
# CARGO_BUILD_JOBS keeps a small builder from running out of memory; raise it on a big one.
FROM cli-tools AS cli-deps
ARG CARGO_BUILD_JOBS=2
ENV CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS}
COPY --from=planner /src/recipe.json recipe.json
RUN cargo chef cook --profile dist --zigbuild --locked --no-default-features -p cyanotype-cli --target x86_64-unknown-linux-musl --recipe-path recipe.json
RUN cargo chef cook --profile dist --zigbuild --locked --no-default-features -p cyanotype-cli --target aarch64-unknown-linux-musl --recipe-path recipe.json
RUN cargo chef cook --profile dist --zigbuild --locked --no-default-features -p cyanotype-cli --target x86_64-apple-darwin --recipe-path recipe.json
RUN cargo chef cook --profile dist --zigbuild --locked --no-default-features -p cyanotype-cli --target aarch64-apple-darwin --recipe-path recipe.json
RUN cargo chef cook --profile dist --zigbuild --locked --no-default-features -p cyanotype-cli --target x86_64-pc-windows-gnu --recipe-path recipe.json

# The CLI itself, stamped with its version (the cli-vX.Y.Z tag) and commit. No version = a development build.
FROM cli-deps AS cli-build
ARG CYANOTYPE_CLI_VERSION=""
ARG CYANOTYPE_GIT_SHA=""
ENV CYANOTYPE_CLI_VERSION=${CYANOTYPE_CLI_VERSION} CYANOTYPE_GIT_SHA=${CYANOTYPE_GIT_SHA}
COPY . .
# COPY keeps the files' own (older) mtimes; without the touch cargo would keep the cook layer's dummy build script and
# stub sources as "fresh" and the build.rs stamps (version, sha, target) would never be produced.
RUN set -eux; \
    find apps/cli crates -name '*.rs' -exec touch {} +; \
    for t in x86_64-unknown-linux-musl aarch64-unknown-linux-musl x86_64-apple-darwin aarch64-apple-darwin x86_64-pc-windows-gnu; do \
      cargo zigbuild --profile dist --locked --no-default-features -p cyanotype-cli --target "$t"; \
      bin=cyanotype; case "$t" in *windows*) bin=cyanotype.exe ;; esac; \
      mkdir -p "/out/$t"; \
      cp "target/$t/dist/$bin" "/out/$t/"; \
    done; \
    ls -lR /out

# Archives, latest.json, SHA256SUMS and (when the secret is present) its signature.
FROM debian:trixie-slim AS cli-dist-assemble
RUN apt-get update \
 && apt-get install -y --no-install-recommends openssh-client \
 && rm -rf /var/lib/apt/lists/*
ARG CYANOTYPE_CLI_VERSION=""
ARG CYANOTYPE_GIT_SHA=""
WORKDIR /w
COPY --from=cli-build /out /w/bin
COPY apps/cli/dist /w/dist
COPY apps/cli/min-cli-version apps/cli/release-key.pub /w/
COPY Cargo.toml /w/Cargo.toml
# `mode=0600`: ssh-keygen refuses a key file others could read. `required=false`: no secret = unsigned dist.
RUN --mount=type=secret,id=cli_release_key,mode=0600,required=false \
    set -eu; \
    version="${CYANOTYPE_CLI_VERSION#cli-v}"; \
    if [ -z "$version" ]; then version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)-dev"; fi; \
    key=""; if [ -s /run/secrets/cli_release_key ]; then key=/run/secrets/cli_release_key; fi; \
    bash dist/make-dist.sh --bin-dir bin --out /dist --version "$version" --sha "${CYANOTYPE_GIT_SHA:-unknown}" \
      --min-cli "$(tr -d ' \n' < min-cli-version)" --key "$key" --pubkey release-key.pub

FROM scratch AS cli-dist
COPY --from=cli-dist-assemble /dist /
