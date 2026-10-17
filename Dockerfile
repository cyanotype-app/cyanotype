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

# ---- CLIP model files, pinned by sha256 in models/clip.lock -------------------------------
# One stage per consumer, so each image carries only its own tower: the worker the image tower (89 MB), the assets service
# the text tower and tokenizer (66 MB). The layers depend only on the lock file and the script, never on source code, so
# they are rebuilt (and pulled to the server) only when the model changes. The same script verifies the checksums for
# a developer checkout (`just models`).
FROM debian:trixie-slim AS clip-fetch
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl \
 && rm -rf /var/lib/apt/lists/*
COPY scripts/fetch-models.sh /usr/local/bin/fetch-models
COPY models/clip.lock /etc/clip.lock

FROM clip-fetch AS clip-vision
RUN LOCK=/etc/clip.lock fetch-models /models vision_model_int8.onnx

FROM clip-fetch AS clip-text
RUN LOCK=/etc/clip.lock fetch-models /models text_model_int8.onnx tokenizer.json

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

# The assets service embeds search queries with the CLIP text tower (ONNX Runtime is linked statically into the binary; the
# C++ runtime it needs, libstdc++6, is in the trixie base image). The model is loaded on the first search.
FROM runtime AS assets
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
COPY --from=clip-vision /models /opt/cyanotype/models/clip
COPY --from=builder /out/cyanotype-worker /usr/local/bin/app
RUN if ldd /usr/local/bin/app | grep "not found"; then echo "worker has unresolved shared libraries (see above): add their packages to this stage" >&2; exit 1; fi
USER cyanotype
ENTRYPOINT ["/usr/local/bin/app"]

FROM runtime AS cli
COPY --from=builder /out/cyanotype-cli /usr/local/bin/app
ENTRYPOINT ["/usr/local/bin/app"]
