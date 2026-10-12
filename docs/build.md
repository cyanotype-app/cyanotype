# Build and automation

## Layout

One `Dockerfile`, one shared compile:

```
chef      rust + cargo-chef + C toolchain (build-essential, cmake; libwebp is built from C and linked statically)
planner   cargo chef prepare   -> recipe.json (changes only when manifests / Cargo.lock change)
builder   cargo chef cook      -> dependencies (cached layer)
          cargo build --release --workspace --locked   -> all four binaries, compiled once
runtime   debian:bookworm-slim, ca-certificates, non-root user
auth | assets | worker | cli   runtime + a single binary each (the --target)
```

`docker buildx bake` (group `default` in `docker-bake.hcl`) builds the four targets together; BuildKit
sees the same `builder` stage in all of them and runs it once. `docker compose build` uses the same
`target:` per service, so it shares the stage too. Previously each image compiled the whole workspace
in parallel, which made a cold build take about 25 minutes.

## Caching

- Dependencies: the cargo-chef `cook` layer is reused until `Cargo.toml`/`Cargo.lock` change. Editing
  source only rebuilds the workspace crates.
- Cargo registry and git checkouts are BuildKit cache mounts (`sharing=locked`).
- CI: `CACHE=gha` makes bake use `type=gha` cache (one scope, `mode=max`, so the builder and cook layers
  are stored). Locally, `CACHE=local` writes to `.buildcache/`; with neither set the normal BuildKit
  cache is used.
- `.dockerignore` keeps `target/`, `.git`, `.env*`, docs and other non-build files out of the context,
  so unrelated edits do not invalidate layers.

## Publishing

`.github/workflows/publish.yml` runs on push to main and `v*` tags (never on pull requests). One
`docker buildx bake --push` (gha cache) pushes `ghcr.io/<owner>/cyanotype-{auth,assets,worker,cli,backup}`
with tags `sha-<short>`, `latest` (main) and `X.Y.Z`/`X.Y`/`X` (tags), authenticated with `GITHUB_TOKEN`
(`packages: write`). `REGISTRY`, `TAGS` and `REVISION` are bake variables. `.github/workflows/deploy.yml`
then deploys a version to the VPS; see `docs/deploy.md`.

## Tasks

`just` is the entry point (see the `justfile`); CI calls the same recipes (`just check`, `just migrate`,
`just images`). CI path filters skip Rust jobs for docs-only changes and run the image job only when the
Dockerfile, bake file, `.dockerignore` or Rust sources change.

## End-to-end script

`scripts/e2e.sh` registers a throwaway user, uploads a generated PNG through the gateway (check, PUT to
S3 via the presigned URL, complete), waits for the worker to produce a thumbnail, then fetches the original
and thumbnail. It needs `curl`, `python3` and `sha256sum`, and the presigned URLs (`S3_PUBLIC_ENDPOINT`)
must be reachable from where it runs.

```sh
just up
just e2e                                   # BASE_URL defaults to http://localhost:3000
BASE_URL=https://staging.example scripts/e2e.sh
```

## Why Cargo and not Bazel/Buck2

The slowness was a build-graph problem (four images each compiling everything), not a limit of Cargo,
and it is fixed by compiling once and caching dependencies. Bazel or Buck2 would add a large maintenance
cost (BUILD files for every crate, rules for C dependencies such as libwebp) for little gain in a
single-language repo. Revisit if the repo grows several languages and contributors and CI is still slow
after this; [moonrepo](https://moonrepo.dev) would be the lighter next step (task graph and affected-only
runs on top of Cargo/npm without replacing them).
