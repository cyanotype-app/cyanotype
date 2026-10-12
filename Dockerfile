# syntax=docker/dockerfile:1.7
#
# One shared builder compiles the whole workspace once; each service image is a tiny
# runtime stage that copies a single binary out of it.
#
#   docker buildx bake                    # all four images (builder runs once)
#   docker build --target assets -t cyanotype-assets .
#
# Targets: auth | assets | worker | cli.   See docs/build.md.

ARG RUST_IMAGE=lukemathwalker/cargo-chef:0.1.77-rust-1-bookworm

# ---- chef: Rust toolchain + cargo-chef + the C toolchain -------------------------------
# The worker's libwebp (webp / libwebp-sys crates) is compiled from bundled C sources and
# linked statically, so build-essential + cmake are needed here and nowhere else.
FROM ${RUST_IMAGE} AS chef
RUN apt-get update \
 && apt-get install -y --no-install-recommends build-essential cmake pkg-config \
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

# ---- runtime base: slim, non-root, CA certs for S3/TLS ---------------------------------
FROM debian:bookworm-slim AS runtime
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --uid 10001 --no-create-home cyanotype
USER cyanotype
# Logs are JSON on stdout (see docs/observability.md).

FROM runtime AS auth
COPY --from=builder /out/cyanotype-auth /usr/local/bin/app
ENTRYPOINT ["/usr/local/bin/app"]

FROM runtime AS assets
COPY --from=builder /out/cyanotype-assets /usr/local/bin/app
ENTRYPOINT ["/usr/local/bin/app"]

# libwebp is statically linked into the binary; no extra runtime packages needed.
FROM runtime AS worker
COPY --from=builder /out/cyanotype-worker /usr/local/bin/app
ENTRYPOINT ["/usr/local/bin/app"]

FROM runtime AS cli
COPY --from=builder /out/cyanotype-cli /usr/local/bin/app
ENTRYPOINT ["/usr/local/bin/app"]
