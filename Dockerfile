# One parameterized image for every Cyanotype binary:
#   docker build --build-arg BIN=cyanotype-assets -t cyanotype-assets .
# BIN is a cargo package name: cyanotype-auth | cyanotype-assets | cyanotype-worker | cyanotype-cli
# (Requires BuildKit, which is the default in current Docker.)

FROM rust:1-bookworm AS build
ARG BIN
# The worker builds libwebp from C sources (webp / libwebp-sys), so it needs a C toolchain.
RUN apt-get update \
 && apt-get install -y --no-install-recommends build-essential cmake pkg-config \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,target=/src/target,id=cyanotype-target-${BIN} \
    test -n "$BIN" \
 && cargo build --release --locked -p "$BIN" \
 && install -D "target/release/$BIN" /out/app

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --uid 10001 --no-create-home cyanotype
COPY --from=build /out/app /usr/local/bin/app
USER cyanotype
# Logs are JSON on stdout (see docs/observability.md).
ENTRYPOINT ["/usr/local/bin/app"]
