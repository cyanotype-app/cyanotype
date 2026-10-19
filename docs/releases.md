# CLI releases

The `cyanotype` command line is handed out by your Cyanotype server itself, from `https://<server>/cli/`. There are no
GitHub Releases and nothing is compiled on the server: the downloads are built in the Docker build of the **web** image
(published by `.github/workflows/publish.yml`) and served as static files by the web container's Caddy.

For users: `curl -fsSL https://<server>/cli/install.sh | sh` (see [`import.md`](import.md)). This page is for the owner.

## What the server serves

| Path | What |
|---|---|
| `/cli/install.sh`, `/cli/install.ps1` | installers; the server fills in its own address when it serves them |
| `/cli/cyanotype-<target>.gz` | the program for one platform, gzipped (`...-windows-gnu.exe.gz` for Windows) |
| `/cli/latest.json` | version, git sha, `signed`, `min_cli_version`, `min_server_api`, and per target: file, sha256, size |
| `/cli/SHA256SUMS`, `/cli/SHA256SUMS.sig` | checksums of `latest.json` and every `.gz`, and their signature |
| `/cli/UNSIGNED` | present only in an unsigned build |

The server keeps **only the latest build**. `cyanotype update --version X` therefore works for the version the server
offers now (and is how you go back when the server was rolled back to an older release); other versions are refused
with a message saying so.

### Targets

| Target | File | Notes |
|---|---|---|
| `x86_64-unknown-linux-musl` | `cyanotype-x86_64-unknown-linux-musl.gz` | static, any distribution |
| `aarch64-unknown-linux-musl` | `cyanotype-aarch64-unknown-linux-musl.gz` | static, Raspberry Pi 4/5, ARM servers |
| `x86_64-apple-darwin` | `cyanotype-x86_64-apple-darwin.gz` | Intel Mac |
| `aarch64-apple-darwin` | `cyanotype-aarch64-apple-darwin.gz` | Apple silicon |
| `x86_64-pc-windows-gnu` | `cyanotype-x86_64-pc-windows-gnu.exe.gz` | 64-bit Windows (MinGW ABI, no Visual C++ runtime needed); runs on Windows on ARM under emulation |

Not shipped: Windows on ARM natively, 32-bit anything, FreeBSD. Windows uses the `-gnu` ABI because the MSVC one needs
Microsoft's SDK, which cannot be fetched in a build.

macOS builds need **no Apple SDK**. The CLI links only libSystem (zig supplies the stubs), because the HTTP client uses
rustls with bundled root certificates rather than Security.framework. This is why the CLI has no native dependencies:
the import never decodes an image, the download is built with `--no-default-features` (no `sqlx`, no AWS SDK, so no
`migrate`, `reindex` or `manifest`: those stay in the server's own `cli` image), and the only C code is `ring`, which zig
compiles. macOS binaries are not notarised: a program fetched with `curl` carries no quarantine flag, so Gatekeeper does
not stop it (a browser download would be stopped; use the install script).

## How it is built

`docker buildx bake cli-dist` (Dockerfile stages `cli-tools` -> `cli-deps` -> `cli-build` -> `cli-dist-assemble` ->
`cli-dist`; `apps/cli/dist/make-dist.sh` does the assembly).

* **Cross-compiler:** zig 0.16.0 as the C compiler and linker, driven by cargo-zigbuild 0.23.4. One x86_64 Linux builder
  makes all five targets. Both tools are pinned by version and sha256 in the Dockerfile (released more than 14 days
  before they were pinned). The Rust standard libraries for the targets come from `rustup target add`, which verifies
  them against Rust's signed manifest. Alternatives that were not chosen: `cross` (needs Docker-in-Docker, no macOS), one
  runner per OS (three CI jobs, a macOS minutes premium, slower), osxcross (needs an Apple SDK).
* **Dependencies per target** are cooked with cargo-chef in their own layers, so a source-only change recompiles just the
  `cyanotype-cli` crate five times. `CARGO_BUILD_JOBS` (default 2) bounds memory on small builders.
* **Version:** `CYANOTYPE_CLI_VERSION` (the `cli-vX.Y.Z` tag) and `CYANOTYPE_GIT_SHA` are build arguments, read by
  `apps/cli/build.rs`. `cyanotype --version` prints `0.3.1 (abc1234)`. A build without a version is a development build:
  it reports `<Cargo version>-dev`, never checks for updates and is never refused for being old.
* **Web image:** `docker-bake.hcl` hands the `cli-dist` output to `apps/web/Dockerfile` as the named context `cli-dist-ctx`;
  it lands in `/srv/cli`. `apps/web/Caddyfile` serves `/cli/*` (no cache without revalidation, a missing file is a 404 and
  never the SPA shell, install scripts rendered with the server's address). The gateway proxies it like any other web path.
  A plain `docker build apps/web` ships an empty `/cli`.
* **Cache:** the cross-compile has its own GitHub Actions cache scope, `cyanotype-cli-dist` (the server images use
  `cyanotype`, the web app `cyanotype-web`), so server-only changes never invalidate it.

## Signing

`SHA256SUMS` lists the hash of every download, `latest.json` included, and is signed with an Ed25519 key in
**SSH signature** format (`ssh-keygen -Y sign -n cyanotype-cli`). SSH signatures need no tool on the signing side beyond
`ssh-keygen`, which the build image has and every owner's machine already has, and the CLI verifies them with `ring`, which
is already in its dependency tree (rustls): no new crate, no new runtime dependency.

* The **public key is compiled into the CLI** from `apps/cli/release-key.pub`. `cyanotype update` verifies the signature
  with it, then checks that `latest.json` and the program are listed in the signed `SHA256SUMS` with matching hashes.
  A server cannot make a CLI install anything the owner did not sign, even if the server is compromised.
* The **private key** is a GitHub Actions secret `CLI_RELEASE_KEY` in the `release` environment. Only the `cli-dist` job of
  `publish.yml` runs in that environment, and it hands the key to BuildKit as a secret mount
  (`--set 'cli-dist.secrets=id=cli_release_key,env=CLI_RELEASE_KEY'`): never a build argument, never in an image layer, never
  printed. The job that pushes images never sees it. Pull-request builds (CI's `images` job) have no key and produce an
  unsigned dist, marked `UNSIGNED` with `"signed": false` in `latest.json`.
* `make-dist.sh` checks its own signature against `apps/cli/release-key.pub` and fails the build on a mismatch (a private
  key with a missing or different public key), so a dist that `cyanotype update` would reject is never published.
* **Until the first signed release exists, `cyanotype update` refuses to install anything** (no key compiled in, or an unsigned
  dist), and says why. The install scripts still work: they verify the checksum, and the signature too when `ssh-keygen`
  is available and the server published a key. Note that for a first install the script, the checksum and the key all come
  from the same server, so the checksum guards against corruption, not against a compromised server; the compiled-in key
  is what protects updates after that.

### One-time setup (on your own machine)

1. Create the key pair (no passphrase, because CI has to use it unattended):

   ```sh
   umask 077
   ssh-keygen -t ed25519 -N '' -C 'cyanotype-cli-release' -f cyanotype-cli-release
   # writes cyanotype-cli-release (private, mode 600) and cyanotype-cli-release.pub
   ```

2. Create the `release` environment, limited to `main` and the `v*` tags (Settings -> Environments -> New environment ->
   Deployment branches and tags -> Selected branches and tags: add branch `main` and tag `v*`), or with the API:

   ```sh
   gh api -X PUT repos/cyanotype-app/cyanotype/environments/release \
     -F 'deployment_branch_policy[protected_branches]=false' -F 'deployment_branch_policy[custom_branch_policies]=true'
   gh api -X POST repos/cyanotype-app/cyanotype/environments/release/deployment-branch-policies -f name=main -f type=branch
   gh api -X POST repos/cyanotype-app/cyanotype/environments/release/deployment-branch-policies -f name='v*' -f type=tag
   ```

3. Store the private key as that environment's secret, then remove the file from disk (keep a copy in a password manager):

   ```sh
   gh secret set CLI_RELEASE_KEY --env release --repo cyanotype-app/cyanotype < cyanotype-cli-release
   shred -u cyanotype-cli-release 2>/dev/null || rm -P cyanotype-cli-release
   ```

4. Commit the public key to the repository (a pull request; the file may keep its `#` comment lines, the first
   `ssh-ed25519` line is used):

   ```sh
   cat cyanotype-cli-release.pub >> apps/cli/release-key.pub
   ```

   The next merge that changes the CLI is tagged, built and signed. Do 4 before, or in the same change as, 3: with the
   secret set but no public key in the repository the `cli-dist` job fails on purpose (see above).

**Rotating** the key means a new pair and a new `apps/cli/release-key.pub`. Installed CLIs only trust the key they were
built with, so they cannot update across a rotation: users re-run the install script once (which installs a build
that knows the new key). Keep the old key until you have decided to accept that.

## Versions and tags

CLI versions are tagged `cli-vX.Y.Z`, separate from the server's `vX.Y.Z` tags.

`.github/workflows/cli-tag.yml` runs on every push to `main` and creates the tag through the API with the workflow's
`GITHUB_TOKEN` (permissions: `contents: write`, `pull-requests: read`; actions pinned by sha like the other workflows).

1. **Did the CLI change?** `scripts/cli_release.py changed BASE HEAD` (BASE is the commit before the push). Yes when a file
   changed under `apps/cli` or under any workspace crate the CLI depends on (computed from `cargo metadata`, normal and build
   dependencies, today `crates/cyanotype-core`), or when the CLI's resolved dependency set differs between the two commits:
   a `Cargo.lock` change that touches a crate the CLI uses, or a feature change in `Cargo.toml`. Changes to other apps,
   the web app, docs and unrelated crates in `Cargo.lock` do not release the CLI.
2. **Which bump?** From the merged pull request: a label `release:major`, `release:minor`, `release:patch` or `release:none`,
   or a line `Release: minor` in the description. A label beats the description; several labels: the largest wins, except that
   `release:none` always wins. Default: patch. `release:none` skips the release.
3. **The next version** is the highest existing `cli-v*` tag plus the bump; with no tags yet the base is 0.0.0, so the first
   release is `cli-v0.0.1` (or `0.1.0` / `1.0.0` with a bigger bump). Runs are serialised (`concurrency`), so two merges in
   a row get consecutive versions.

The four labels exist in the repository (`release:major`, `release:minor`, `release:patch`, `release:none`).

### Why Publish waits for the tag, and how it cannot race

A tag created with `GITHUB_TOKEN` does not trigger other workflows, and the tag job runs in parallel with `publish.yml` on the
same push. So that the images built for a commit carry the right CLI version, the `cli-dist` job of Publish first waits for
the tag workflow's run **for the same commit** (`scripts/wait-for-cli-tag.sh`, 15 minutes at most, fails if the tag job
failed), then fetches the tags and takes the version of the newest `cli-v*` tag reachable from the commit
(`scripts/cli_release.py current <sha>`). Waiting beats re-deriving the version in Publish: there is exactly one place that
decides (the tag job), and the build only reads the result, so the two can never disagree. The version is a pure function
of the commit and the tag set at that time; rebuilding an old commit gives its old version. A commit with no CLI change
reuses the previous tag's version (the CLI is rebuilt but byte-for-byte equivalent in behaviour). With no `cli-v*` tag at all
the build is a development build and is not signed. The Deploy workflow is unchanged: it pulls the web image Publish
built, so the deploy cannot get ahead of the tag.

## `cyanotype update`

* `cyanotype update` downloads the build for its own target from the server it is logged in to (or `--server`), verifies the
  signature and checksums as above, runs the new program once with `--version` to prove it starts, and replaces itself: the
  new file is written next to the old one and renamed over it (atomic on Linux and macOS; on Windows the running `.exe` is
  renamed aside first and removed on the next run).
* `--check` only reports. `--version X.Y.Z` installs that version if the server has it (also a downgrade).
* It refuses when the executable is somewhere a package manager owns it (`/usr/bin`, Homebrew's Cellar, Nix, snap, Scoop,
  Chocolatey, winget, `~/.cargo/bin`) and says which command to use instead.
* It refuses unsigned or badly signed builds, and anything from a CLI that has no release key compiled in.

### The daily hint

After `login`, `import` and `status`, at most once a day, the CLI asks the server it is logged in to for `/cli/latest.json`
and prints to stderr `cyanotype X is available (you have Y). Run `cyanotype update`.` The request starts with the
command, is cached in `~/.cache/cyanotype/update-check.json`, has a 5 second timeout and is given 1.5 seconds after the
command finishes; it never fails or slows a command. It is skipped in CI (`CI` and the usual CI variables), when stderr or
stdin is not a terminal, in development builds, and when `CYANOTYPE_NO_UPDATE_CHECK=1`.

### Minimum supported version

`apps/cli/min-cli-version` holds the oldest CLI this release of the server accepts; it goes into `latest.json` as
`min_cli_version`. `cyanotype import` reads it first and refuses a CLI that is older, saying to run `cyanotype update`. Raise
it only when the server drops something an older CLI uses. If the server's `latest.json` cannot be read (an older server, a
network error) the import carries on. `min_server_api` is reserved for a future breaking change of the HTTP API (always 1
today).

## Build time and size

Measured on a shared 4-CPU host with the builder capped at 2 CPUs, 2.5 GB and `CARGO_BUILD_JOBS=2`:

| | time |
|---|---|
| cold, everything (tools, five dependency layers, five CLI builds, assembly) | 17 min (dependency layers 190 + 183 + 124 + 128 + 254 s, the five CLI builds 83 s) |
| warm dependency layers, a source change | 2 min 50 s |
| peak memory of the builder container | 2.4 GiB, page cache included (the cap was 2.5 GB and not hit); the compilers themselves need far less |

Each program is 6.7 MB (3 MB gzipped); the dist adds 15.5 MB to the web image. The cold cross-compile of five targets is
dominated by compiling `reqwest`/`rustls`/`tokio` once per target; with the layer cache a source-only change costs five
incremental builds of the one crate. The builder's cache for the five targets is several GB, which counts against the
10 GB GitHub Actions cache budget shared with the server images; if it ever evicts the Rust cache, drop `mode=max` for the
`cli-dist` target's `cache-to` in `docker-bake.hcl`.

## Checks

* `scripts/test-cli-dist.sh` (no Docker): `make-dist.sh` with a throwaway key: files, signature, wrong key, missing newline.
* `scripts/test-cli-dist.sh --docker` (CI, `images` job): bakes the real stage, checks the five programs (ELF/Mach-O/PE
  headers, machine type), runs the Linux one, serves everything with the real web `Caddyfile`, runs `install.sh` against it
  and checks that a tampered archive is refused.
* `python3 -I -m unittest discover -s scripts -p 'test_cli_release.py'`: bump rules, version arithmetic, what counts as a
  CLI change (including against this repository's `cargo metadata`).
* `cargo test -p cyanotype-cli` (also with `--no-default-features`): version comparison, `latest.json`, signature verification
  (a throwaway key made in the test, and a fixture signed by the real `ssh-keygen`), the check throttle and switches, the
  atomic replace, and `update` against a stub server (signed, unsigned, tampered, wrong key, downgrade, package-managed path).
