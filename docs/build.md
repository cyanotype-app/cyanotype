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

## Web image

The UI has its own `apps/web/Dockerfile` (context `apps/web`, no Rust): `node:24-alpine` runs
`pnpm install --frozen-lockfile` and `pnpm build`, and the result is copied into `caddy:2-alpine`
(`apps/web/Caddyfile`, :8080, unprivileged). `docker-bake.hcl` has a `web` target in the default group, so
`just images` and the publish workflow build and push `cyanotype-web` with the other images (its own gha
cache scope). Caching headers: `/static/*` (Vite's hashed output; `build.assetsDir` is `static` because
`/assets/*` is the API) is `immutable` for a year, everything else, `index.html` included, is `no-cache`, and
unknown paths fall back to `index.html` for client-side routes. `/static/*` misses stay real 404s.

## Web app

`apps/web`: React 19, TypeScript, Vite, React Router, plain CSS (no UI kit), `thumbhash` for placeholders.
Versions are pinned by `pnpm-lock.yaml` and `packageManager` in `package.json`.

- `just web-dev`: Vite on :5173 proxying `/auth`, `/assets`, `/healthz` to the gateway (`GATEWAY_URL`, default
  `http://localhost:3000`, so run `just up` or the services first).
- `just web-check`: `tsc --noEmit`, eslint and vitest (API client error handling, lightbox and grid keyboard
  behaviour with Testing Library). `just web-build`: production build. CI runs both when `apps/web/**` changes.
- Session: the HttpOnly cookie from `/auth/login`, sent with `credentials: 'include'`; `GET /auth/me` decides
  signed in or out. The API has no "is registration open" endpoint, so the register link is always offered and a
  403 shows a "registration is closed" message (and hides the link for the rest of the tab session).
- Library: `GET /assets` pages with `before`/`next_before`. Each list item carries `thumbhash` (base64) and a
  presigned `thumb_url` (both null until the worker has produced the derived images). The lightbox fetches
  `GET /assets/{id}` for the `preview_url`, and ←/→/Esc work. An expired thumbnail URL is refreshed once from
  `GET /assets/{id}`.
- `src/api.ts` is a hand-written typed client matching the Rust types. Generating it from a utoipa OpenAPI
  document is a follow-up.
- Not built yet: browser upload (blocked by CORS on Linode Object Storage; the empty state points to the CLI),
  a service worker (the manifest and icons are in place for PWA install later).

## Publishing

`.github/workflows/publish.yml` runs on push to main and `v*` tags (never on pull requests). One
`docker buildx bake --push` (gha cache) pushes `ghcr.io/<owner>/cyanotype-{auth,assets,worker,cli,backup,web}`
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
