# Deploying to a single VPS

One Linode (shared CPU, 4 GB, us-ord, Debian 12) runs the whole stack with `docker compose`: Postgres,
the three services, the static web UI container, the Caddy gateway (automatic HTTPS) and a nightly backup job. Object storage is Linode
Object Storage (us-ord-10), and images come from GHCR (private by default). Nothing is built on the server.

```
internet -> :80/:443 Caddy (TLS) -> auth :3001 / assets :3002 / web :8080 (everything else)       worker (no port)
                                        \-> Postgres (not published)   backup -> bucket backups/postgres/
clients  <-> presigned URLs <-> Linode Object Storage (bytes never touch the API)
```

Files: `docker-compose.yml` (base, local) + `docker-compose.prod.yml` (override), `.env.prod` (from
`.env.prod.example`, on the server only), `infra/caddy/Caddyfile.prod` (+ `infra/caddy/tls/<mode>/`,
`infra/caddy/cloudflare-ips.caddy`), `infra/backup/`. Behind the Cloudflare proxy, see "Cloudflare" below.

You create the server, Cloud Firewall and DNS yourself; this runbook says what to create.

## Security settings (auth service)

| Variable | Default | Meaning |
|---|---|---|
| `REGISTRATION_MODE` | `first_user` | `open`: anyone may register. `first_user`: sign-up works only while the `users` table is empty, so the owner claims the instance and registration then closes (403). `closed`: `POST /auth/register` always returns 403. |
| `COOKIE_SECURE` | `true` | `Secure` attribute on the session cookie. Local compose sets it to `false` (plain http); the prod override pins `true`. |
| `AUTH_RATE_LIMIT_BURST` / `AUTH_RATE_LIMIT_PER_MINUTE` | `10` / `10` | Token bucket per client IP for `POST /auth/login` and `/auth/register` (they share one bucket). Over the limit: `429` with `Retry-After`. |
| `AUTH_RATE_LIMIT_MAX_KEYS` | `10000` | Upper bound on tracked client IPs (memory is bounded; stale entries are evicted first). IPv6 clients are keyed by /64. |
| `TRUSTED_PROXIES` | loopback + private ranges | Peers allowed to supply `X-Forwarded-For`. `none` disables forwarded headers. |

Invalid values stop the service at startup instead of falling back to something looser.

`TRASH_RETENTION_DAYS` (assets service, default `30`, 1 to 36500): how long a photo stays in the trash before it is
deleted permanently. Trash, restore and permanent deletion (including old object versions on this versioned
bucket) are described in [`library.md`](library.md).

`first_user` is race-safe: registration takes a transaction-scoped Postgres advisory lock, checks that the
table is empty and inserts, so concurrent sign-ups produce exactly one account (covered by a test that fires
12 at once). The local compose stack sets `REGISTRATION_MODE=open` by default so `just e2e` can register a
throwaway user on every run; production defaults to `first_user`.

**Client-IP trust model.** The rate limiter keys on the socket peer. If, and only if, the peer is inside
`TRUSTED_PROXIES` (the Caddy container on the private compose network, which is the only thing that can
reach auth), the right-most `X-Forwarded-For` entry that is not itself a trusted proxy is used instead.
Caddy replaces any client-supplied `X-Forwarded-For` with the real peer address, and the auth port is not
published, so clients cannot choose their own address. A direct connection from anywhere else ignores the
header entirely. Limits are per process: with several auth replicas each keeps its own bucket.

Behind Cloudflare, Caddy resolves the real client from `CF-Connecting-IP` (only when the TCP peer is a
Cloudflare edge range) and sends the auth service exactly that address as the single `X-Forwarded-For`
value, so the limiter keys on the real user (see "Cloudflare"). Without Cloudflare (`CADDY_TLS_MODE=auto`),
Caddy uses the TCP peer.

Caveat (direct mode only, no Cloudflare): Docker publishes IPv6 ports through a userland proxy, which hides
the real client address (every IPv6 visitor then looks like the bridge gateway and shares one bucket).
Unless you have enabled `"ip6tables": true` in `/etc/docker/daemon.json`, create only an A record, not an
AAAA. Behind Cloudflare this does not matter: clients reach Cloudflare over IPv4 or IPv6, Cloudflare
connects to the origin over IPv4, and the client address arrives in `CF-Connecting-IP`.

## 1. Create the Linode

Cloud Manager -> Create Linode: Image Debian 12, Region Chicago, IL (us-ord), Shared CPU 4 GB, a root
password you will not use (SSH keys only, below), add your SSH public key, and enable backups if you want
VM-level snapshots on top of the Postgres dumps. Note the IPv4 address.

### Object storage settings (Linode, us-ord-10)

Use `S3_ENDPOINT=https://us-ord-10.linodeobjects.com`, `S3_REGION=us-ord-10` and `S3_FORCE_PATH_STYLE=true`.
The region is the cluster ID (`us-ord-10`), not `us-east-1`. These values were verified against us-ord-10
(head-bucket, versioning status and listing all work with them). The backup job uses the same variables.

## 2. Cloud Firewall

Cloud Manager -> Firewalls -> Create, attach it to the Linode. Inbound policy Drop, rules:

| Protocol | Ports | Sources |
|---|---|---|
| TCP | 80 | all IPv4 / IPv6 (Let's Encrypt HTTP challenge and the redirect to https) |
| TCP | 443 | all IPv4 / IPv6 |

With the Cloudflare proxy, restrict 80/443 to Cloudflare's ranges instead (see "Cloudflare", step 4).
| TCP | 22 | your own IP (`/32`) only |

Outbound: accept. Postgres (5432) and the service ports are neither published by compose nor allowed by
the firewall. Docker bypasses host firewalls like ufw by writing its own iptables rules, so rely on the Cloud
Firewall.

### Bootstrap script

`scripts/bootstrap-server.sh` does steps 3 to 5 below in one go (and the Docker install and the `.env.prod`
skeleton). Run it once, as root, on the fresh server. It is idempotent, needs no git or GitHub access, and never
overwrites an existing `.env.prod`. Generate the deploy keypair on your own machine, never on the server:

```sh
ssh-keygen -t ed25519 -f ~/.ssh/cyanotype-deploy -C cyanotype-deploy -N ''
scp scripts/bootstrap-server.sh root@<ip>:
ssh root@<ip> 'DEPLOY_PUBKEY="$(cat)" bash bootstrap-server.sh' < ~/.ssh/cyanotype-deploy.pub
```

(Or paste it: `ssh root@<ip>`, `cat > bootstrap-server.sh`, paste, Ctrl-D, then
`DEPLOY_PUBKEY='ssh-ed25519 AAAA... cyanotype-deploy' bash bootstrap-server.sh`.) Optional variables: `DEPLOY_USER`
(`deploy`), `HOSTNAME_FQDN` (`photos-dev.cyanotype.app`), `HARDEN_SSH` (`1`), `PUBLIC_IP`. SSH hardening (keys only,
`PermitRootLogin prohibit-password`) is applied only if both root and the deploy user already have an
authorized key, so add your own key to `/root/.ssh/authorized_keys` first (Linode does this when you pick an SSH
key at creation). It ends by printing `DEPLOY_HOST`, `DEPLOY_USER`, the exact `DEPLOY_KNOWN_HOSTS` line (read from
the server's own host key) with its fingerprint, and the remaining steps. It does not configure a host firewall;
use the Linode Cloud Firewall.

## 3. Harden the host

As root, once:

```sh
adduser --disabled-password --gecos "" deploy
install -d -m 700 -o deploy -g deploy /home/deploy/.ssh
cp /root/.ssh/authorized_keys /home/deploy/.ssh/ && chown deploy:deploy /home/deploy/.ssh/authorized_keys
usermod -aG sudo deploy      # optional; set a password or use passwordless sudo deliberately

# SSH keys only, no root login
cat >/etc/ssh/sshd_config.d/10-hardening.conf <<'CONF'
PermitRootLogin no
PasswordAuthentication no
KbdInteractiveAuthentication no
CONF
systemctl reload ssh      # keep this session open and test a second login as deploy first

apt-get update && apt-get -y install unattended-upgrades
dpkg-reconfigure -plow unattended-upgrades    # answer Yes
```

## 4. Install Docker

As `deploy` (with sudo), from Docker's official apt repository:

```sh
sudo apt-get install -y ca-certificates curl
sudo install -m 0755 -d /etc/apt/keyrings
sudo curl -fsSL https://download.docker.com/linux/debian/gpg -o /etc/apt/keyrings/docker.asc
echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/docker.asc] https://download.docker.com/linux/debian $(. /etc/os-release && echo $VERSION_CODENAME) stable" | sudo tee /etc/apt/sources.list.d/docker.list
sudo apt-get update
sudo apt-get install -y docker-ce docker-ce-cli containerd.io docker-buildx-plugin docker-compose-plugin
sudo usermod -aG docker deploy      # log out and in again
docker compose version              # must be >= 2.24 (the prod file uses !reset / !override)
```

Members of the `docker` group are effectively root on the host; that is why the deploy user is dedicated.
Docker's default json-file logging has no size cap; add `{"log-driver":"json-file","log-opts":{"max-size":"20m","max-file":"5"}}`
to `/etc/docker/daemon.json` and `sudo systemctl restart docker` before the first deploy.

## 5. Prepare the deploy directory (no git on the server)

The repository is private and the server never runs git or holds any GitHub credential. It only needs the
runtime files (compose files and `infra/caddy`), which every deploy ships to it as a small "deploy bundle"
(`scripts/make-deploy-bundle.sh`; no source, no `.env*`, no `certs/`). GitHub deploy keys are not needed (and
are disabled for the organization). So the manual part is just the directory and the secrets:

```sh
mkdir -p ~/cyanotype/certs && chmod 700 ~/cyanotype/certs
cd ~/cyanotype
$EDITOR .env.prod        # start from .env.prod.example in the repo; fill every value; chmod 600 .env.prod
chmod 600 .env.prod
```

Put `.env.prod` (mode 600) and, for Cloudflare, `certs/` (see "Cloudflare") there. The first deploy
(Actions -> Deploy -> Run workflow, or `just deploy`) ships `docker-compose.yml`, `docker-compose.prod.yml`
and `infra/caddy/**` into the directory. Deploys replace only those managed files; `.env.prod`, `certs/` and
all data are never touched (only the `CYANOTYPE_VERSION` line of `.env.prod` is updated).

(If you copy the env file from your machine: `scp -p .env.prod deploy@HOST:cyanotype/ && ssh deploy@HOST chmod 600 cyanotype/.env.prod`.)
Generate the database password with `openssl rand -base64 24 | tr -d '/+='`. The bucket must exist, with
an access key limited to it (Linode: Object Storage -> Access Keys -> Limited Access, read/write on that bucket).

## 6. Pull access to GHCR (images are private)

The published images stay private, so the server must authenticate to pull them. There are two ways:

- **Automated deploys (recommended)**: the GitHub Actions deploy workflow logs the server in with its own
  short-lived `GITHUB_TOKEN` for the duration of a deploy and logs out afterwards. No long-lived token is
  stored on the server. See "Setting up automated deploys".
- **Manual deploys** (`just deploy` or running compose by hand): either export `GHCR_TOKEN` (and `GHCR_USER`) on
  your machine so `just deploy` pipes it over stdin for that deploy only (login, then logout), or log in once on the
  server. For the latter create a fine-grained or classic personal
  access token with only `read:packages`, then on the server `echo "$PAT" | docker login ghcr.io -u <github-user> --password-stdin`
  (typed into the session, not saved in a file). The credential is stored in `~/.docker/config.json` (mode 600);
  revoke the PAT when you retire the server.

To make the images public instead, change the package visibility in GitHub (Packages -> Package settings) and
skip authentication; the default is private.

## 7. DNS

Create an `A` record for the instance hostname pointing at the Linode's IPv4 address (TTL 300 while
testing). Keep the apex `cyanotype.app` for the project website and give each instance a one-level subdomain
(e.g. `photos-dev`, not `photos.dev`; see the Cloudflare note below); this
dev-tier instance is `photos-dev.cyanotype.app`, set as `CYANOTYPE_HOSTNAME` in `.env.prod`. Skip the AAAA
record unless IPv6 is handled as described under the client-IP caveat. In the default `auto` mode, Caddy
requests the certificate on the first start once the name resolves to the server and ports 80/443 are open.
If the record is proxied by Cloudflare, follow "Cloudflare" instead.

## Cloudflare (proxied DNS)

`photos-dev.cyanotype.app` is proxied by Cloudflare (orange cloud). Caddy then runs with
`CADDY_TLS_MODE=cloudflare-origin`: it serves a Cloudflare Origin CA certificate, does no ACME (Let's Encrypt
cannot validate through the proxy), and takes the real client IP from `CF-Connecting-IP`. The other mode,
`auto` (default), is for DNS-only records and gets a Let's Encrypt certificate. Cloudflare terminates the
browser's TLS, then opens a second TLS connection to the origin and checks the Origin CA certificate there.

**Hostname depth.** With the proxy on, the hostname must be ONE level under the zone (e.g.
`photos-dev.cyanotype.app`), because Cloudflare's free Universal SSL edge certificate covers only
`cyanotype.app` and `*.cyanotype.app`, not deeper names such as `photos.dev.cyanotype.app` (those fail the TLS
handshake at the edge). The alternatives are Cloudflare Advanced Certificate Manager (paid) or a grey-cloud
(DNS-only) record with `CADDY_TLS_MODE=auto`. The Origin CA certificate must also cover the hostname; the
default `*.cyanotype.app` does.

```
client -> Cloudflare edge --(TLS, Origin CA cert, SNI photos-dev.cyanotype.app)--> Caddy :443 -> auth/assets
```

### 1. DNS

Dashboard -> DNS -> Records: one `A` record, name `photos-dev` (zone `cyanotype.app`), IPv4 `66.228.50.66`,
Proxy status **Proxied** (orange cloud). No AAAA record is needed: Cloudflare answers IPv6 clients itself
and reaches the origin over IPv4.

### 2. Origin CA certificate

Dashboard -> SSL/TLS -> Origin Server -> Create Certificate. Choose "Generate private key and CSR with
Cloudflare" with key type RSA (2048) or ECDSA, hostnames `photos-dev.cyanotype.app` (add more only if you
need them; the default wildcard `*.cyanotype.app` also covers it), validity 15 years. Copy the certificate (PEM) and
the private key (shown once). On the server:

```sh
cd ~/cyanotype
mkdir -p certs && chmod 700 certs
$EDITOR certs/origin.pem        # paste the certificate
$EDITOR certs/origin-key.pem    # paste the private key
chmod 600 certs/origin.pem certs/origin-key.pem
```

`certs/` is gitignored and mounted read-only into the gateway at `/etc/cyanotype-certs/`. The key must
never be committed or pasted anywhere else. Caddy runs as root in its container, so mode 600 owned by
`deploy` is enough for it to read the files.

### 3. SSL/TLS mode: Full (strict)

Dashboard -> SSL/TLS -> Overview -> Full (strict). Do not use Flexible (plain http to the origin) or Full
(non-strict). Optionally enable "Always Use HTTPS" and a minimum TLS version of 1.2 (SSL/TLS -> Edge Certificates).

### 4. Restrict the origin to Cloudflare (Linode Cloud Firewall)

Origin CA certificates are trusted only by Cloudflare, and anyone who reaches the origin directly could
forge `CF-Connecting-IP`-less traffic or bypass Cloudflare's protections. In the Linode UI (Firewalls ->
your firewall -> Inbound rules) replace the "all IPv4 / IPv6" rules for 80 and 443 with:

| Protocol | Ports | Sources |
|---|---|---|
| TCP | 80, 443 | Cloudflare IPv4 ranges from https://www.cloudflare.com/ips-v4 and IPv6 ranges from https://www.cloudflare.com/ips-v6 |
| TCP | 22 | your own IP (`/32`) only (unchanged) |

Policy stays Drop. Linode firewall rules take a limited number of addresses per rule, so split the ranges
over several rules (for example one for 80,443 with the v4 list, one for v6). Even with a correct firewall,
Caddy only believes `CF-Connecting-IP` from those same ranges, so a direct request cannot spoof it.
Cloudflare publishes changes rarely; re-check the lists now and then (below).

### 5. Server: switch the gateway to the mode

In `.env.prod` set `CADDY_TLS_MODE=cloudflare-origin` (and keep `CYANOTYPE_HOSTNAME=photos-dev.cyanotype.app`), then:

```sh
dc up -d gateway        # recreates only the gateway
dc logs gateway | tail  # no "obtaining certificate" lines: ACME is off
curl -fsS https://photos-dev.cyanotype.app/healthz      # through Cloudflare
```

### Real client IP

`infra/caddy/tls/cloudflare-origin/servers.caddy` sets, in Caddy's global `servers` options,
`trusted_proxies static <Cloudflare ranges>` (from `infra/caddy/cloudflare-ips.caddy`),
`client_ip_headers CF-Connecting-IP X-Forwarded-For` and `trusted_proxies_strict`. Caddy therefore uses
`CF-Connecting-IP` only when the TCP peer is a Cloudflare edge; from any other peer, headers are ignored and
the peer address is the client. The access log's `request.client_ip` is that resolved address
(`request.remote_ip` is the edge or peer). The gateway then sets the upstream `X-Forwarded-For` to the
single value `{client_ip}`, discarding Cloudflare's own XFF chain (which would otherwise end in an edge
address). The auth service trusts `X-Forwarded-For` only from the gateway (`TRUSTED_PROXIES`, default private
ranges, which covers the compose network), so the rate limiter keys on the real user. Chain:
client -> Cloudflare (adds `CF-Connecting-IP`) -> Caddy (trusted peer, resolves `client_ip`) -> auth
(trusted peer, reads the single XFF entry). Verified locally with a stand-in upstream; after going live,
check that `request.client_ip` in `dc logs gateway` is your own public IP, not a `104.x`/`172.x` address.

**Refreshing the ranges.** Run `scripts/update-cloudflare-ips.sh` (fetches ips-v4 and ips-v6 and rewrites
`infra/caddy/cloudflare-ips.caddy`), review the diff, commit, merge and deploy (the server has no git, so a change made only on the server is
overwritten by the next deploy). Update the Linode firewall rules to match.

### Large uploads and caching

- **Uploads and downloads bypass Cloudflare.** Photo bytes move through presigned URLs straight to and from
  Linode Object Storage, so Cloudflare's request-body limit (100 MB on Free/Pro) does not apply. Only small
  JSON API calls (`/auth/*`, `/assets/*`) go through Cloudflare, plus `/healthz`.
- **API responses are not cached.** The gateway sets `Cache-Control: no-store` on every `/auth/*` and
  `/assets/*` response, replacing whatever the service sent (the services send none). Cloudflare does not
  cache JSON by default anyway; this keeps it that way even if someone adds a "cache everything" rule. Do not
  add Cache Rules for this hostname.
- The Deploy workflow's health check (`https://<hostname>/healthz`) works through Cloudflare unchanged.

## 8. First deploy

Run Actions -> Deploy -> Run workflow (see "Setting up automated deploys"), or `DEPLOY_HOST=deploy@<origin-ip> just deploy`.
That ships the compose and Caddy files, then pulls, migrates and starts the stack. To drive it by hand on the server
afterwards (the files are there once a deploy has run):

```sh
cd ~/cyanotype
alias dc='docker compose --env-file .env.prod -f docker-compose.yml -f docker-compose.prod.yml'
dc pull                    # images from GHCR
dc run --rm migrate        # starts Postgres, applies migrations
dc up -d --wait
dc ps
curl -fsS https://photos-dev.cyanotype.app/healthz
```

Register the owner account right away (first_user mode lets the first caller claim the instance):

```sh
curl -fsS -c /dev/null -H 'content-type: application/json' \
  -d '{"email":"you@example.com","password":"a long passphrase"}' https://photos-dev.cyanotype.app/auth/register
```

A second registration now returns 403. Then run the smoke test from your machine against the instance:
`E2E_EMAIL=... E2E_PASSWORD=... BASE_URL=https://photos-dev.cyanotype.app just e2e` (note: browser-style
direct uploads are subject to the CORS caveat below; the script uploads with curl, which is fine).

## 9. Updating

Image versions are tags published by CI: `sha-<short>` on every push to main, `latest` for main, and
`X.Y.Z` for `vX.Y.Z` tags. `CYANOTYPE_VERSION` in `.env.prod` selects one.

- **Automatic/CI**: see "Setting up automated deploys".
- **Manual from your machine**: `DEPLOY_HOST=deploy@<origin-ip> just deploy [version]` (default `latest`). It builds
  the deploy bundle for that version locally (`sha-<short>` -> that commit, `X.Y.Z` -> tag `vX.Y.Z`, `latest` ->
  `origin/main`; `DEPLOY_REF=worktree` ships your working tree instead), streams it over SSH, replaces only the managed
  files in `~/cyanotype`, sets `CYANOTYPE_VERSION`, then pulls, migrates and brings the stack up (and restarts the
  gateway if the Caddy config changed). Registry access for the pull: the workflow is the preferred path (token
  never stored). For `just deploy`, either `export GHCR_TOKEN=<read:packages PAT> GHCR_USER=<you>` (piped over stdin,
  logged out afterwards) or run `docker login ghcr.io` once on the server (step 6). `just bundle [version]` writes the
  tarball without deploying.
- **Manual on the server** (config already in place, no new compose files): `cd ~/cyanotype && dc pull && dc run --rm migrate && dc up -d --wait`
  with `dc` as defined in step 8. There is no git on the server, so compose/Caddy changes arrive only via a deploy.

Migrations run before the new containers start while the old ones still serve, so keep migrations backward
compatible with the previous release (add before you remove).

### Rollback

Pin an older version: `just deploy sha-1a2b3c4` (or edit `CYANOTYPE_VERSION` in `.env.prod`, then
`dc pull && dc up -d`; that keeps the current compose files). From GitHub: Actions -> Deploy -> Run workflow -> `version` = the older tag; the
workflow (and `just deploy`) also ships the compose and Caddy files from the matching commit (or `vX.Y.Z` tag), so an
older version runs with its own configuration. Migrations are not
rolled back: if a release added a migration that the old code cannot live with, restore a database backup
(below) as well.

## Setting up automated deploys

The `Deploy` workflow (`.github/workflows/deploy.yml`) runs on manual dispatch (input: version, default the
`sha-<short>` of the selected ref) and, after a successful `Publish images` run on main, automatically. It
uses the GitHub Environment `dev`, so you can add required reviewers there to approve each deploy.

**CI gate.** Branch protection is not available (private repo, free plan), so a merged red PR would otherwise
deploy. For automatic deploys the job first runs `scripts/wait-for-ci.sh` for the head commit of the `Publish images`
run: it polls the `CI` workflow run for that commit (every 30 s, up to 40 min) and fails the deploy unless it
concluded `success`. Manual runs (workflow_dispatch, including rollbacks) skip the gate: that is an explicit operator
override.

1. **Deploy user with docker access.** The `deploy` user from step 3, in the `docker` group (step 4). Use a
   dedicated account for CI if you prefer separating it from your own login.
2. **Deploy keypair** (a plain SSH keypair, not a GitHub deploy key; GitHub deploy keys are not used and the server
   needs no GitHub access. No passphrase because CI uses it unattended; dedicated to this one purpose). The private key goes
   in the `dev` environment secret `DEPLOY_SSH_KEY`, the public key in the server's `authorized_keys`:

   ```sh
   ssh-keygen -t ed25519 -N '' -C 'cyanotype-deploy' -f ./cyanotype_deploy
   ssh-copy-id -i ./cyanotype_deploy.pub deploy@<origin-ip>   # or append it to ~deploy/.ssh/authorized_keys
   ```

   Paste the private key into the `DEPLOY_SSH_KEY` secret, then delete the local copy
   (`shred -u cyanotype_deploy`).
3. **Pinned host key.** `ssh-keyscan -t ed25519 <origin-ip>` prints the server's host key line. Before
   trusting it, compare its fingerprint (`ssh-keyscan -t ed25519 HOST | ssh-keygen -lf -`) with the one shown
   by the Linode console (Launch LISH Console, then `ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub` on the
   server). Store the verified line as `DEPLOY_KNOWN_HOSTS`. The workflow uses `StrictHostKeyChecking yes`
   against only that file, never `no`.
4. **GitHub secrets.** Repository -> Settings -> Environments -> New environment `dev` (add required reviewers
   if you want approvals), then add the environment secrets `DEPLOY_HOST` (the **origin IP**, e.g. `<origin-ip>`, or a DNS-only hostname; not the Cloudflare-proxied hostname, because SSH cannot pass through the orange-cloud proxy), `DEPLOY_USER`
   (`deploy`), `DEPLOY_SSH_KEY` and `DEPLOY_KNOWN_HOSTS`. Add `CYANOTYPE_HOSTNAME` (the public hostname; required when `DEPLOY_HOST` is an IP) as an environment
   variable or as a secret; either works. The health check goes through the public hostname and calls
   `https://<hostname>/healthz` and fails the job if it is unhealthy.
5. **Optional: restrict the key with a forced command.** In `~deploy/.ssh/authorized_keys` prefix the key:
   `restrict,command="/home/deploy/deploy-gate.sh" ssh-ed25519 AAAA... cyanotype-deploy`, where the script reads
   `$SSH_ORIGINAL_COMMAND` and only allows `docker login ghcr.io ...`, `docker logout ghcr.io`, and the
   two `bash -c '...' _ <args>` commands (bundle unpack and rollout). This limits what a leaked key can do (`restrict` disables
   forwarding and a pty), but note the deploy script itself runs Docker, so the key stays powerful.

**What a deploy does.** The job checks out the repository (full history and tags), maps the version to a commit,
builds the deploy bundle with `git archive`, and streams it over the pinned SSH connection into `~/cyanotype`.
Only the managed files are replaced (`.env.prod` and `certs/` are never touched).

**Registry authentication without a stored token.** The job pipes its short-lived `GITHUB_TOKEN`
(`packages: read`, expires when the job ends) into `docker login ghcr.io --password-stdin` over SSH, pulls,
migrates, brings the stack up, and always runs `docker logout ghcr.io`. The token is only ever on stdin and
in an environment variable of the runner (GitHub masks it), never on a command line or in a process list.
So no PAT lives on the server; a read-only PAT is needed only for manual deploys that do not pass `GHCR_TOKEN` (step 6). For the token to be
allowed to read the packages, grant this repository access under each package's settings (Manage Actions access).

## Backups

The `backup` service runs `pg_dump -Fc` nightly (default 03:17 UTC, `BACKUP_AT`), checks that `pg_restore`
can read it, uploads it to `s3://$S3_BUCKET/backups/postgres/cyanotype-<UTC timestamp>.dump`, and keeps the
newest `BACKUP_KEEP` (default 14). It is a small image (Alpine, `pg_dump`, `rclone`; `infra/backup`), using
the same `S3_*` variables as the services and no cloud-specific settings. A failed night is logged
(`backup FAILED`) and the next night tries again; watch for it in the logs.

```sh
just prod-backup-now                        # from your machine (DEPLOY_HOST set); or on the server:
dc exec backup /usr/local/bin/backup.sh now
dc exec backup /usr/local/bin/backup.sh list
```

**The bucket has object versioning turned on.** (Permanently deleted photos are removed version by version by
the assets service itself, see [`library.md`](library.md); the advice below is for database dumps.) Pruning an old dump only adds a delete marker; the old
versions stay and keep using (billed) storage. Add a lifecycle rule on the bucket that expires noncurrent
versions after some days (Linode supports lifecycle rules via the S3 API or a tool such as `s3cmd`), or
purge old versions by hand now and then. Backups cover the database only: originals and derived files are
already in the bucket, and `cyanotype reindex` rebuilds the photo index from them (restoring the newest dump first
gets back users and API keys; per-user daily manifests in the bucket carry names, trash, favorites and albums), so
keep the dumps. See [`recovery.md`](recovery.md) for exactly what comes back and the runbook.

### Restore

1. Stop everything that writes: `dc stop auth assets worker`.
2. Pick a dump: `dc run --rm backup list` (names sort by time).
3. Restore into the running database (this drops and recreates the objects in the dump):
   `dc run --rm -e CONFIRM=yes backup restore cyanotype-20261012T031700Z.dump` (omit the name for the newest).
4. `dc run --rm migrate` (a no-op if the dump is current).
5. **If the dump is older than the newest uploads, reconcile before starting the stack**: `dc run --rm reindex`
   (dry run), review, `dc run --rm reindex --apply` (or the **Reindex** workflow), then `dc up -d --wait`. The full
   runbook and what can and cannot be recovered: [`recovery.md`](recovery.md).

To rehearse without touching production, restore into a scratch database: create it
(`dc exec postgres createdb -U cyanotype scratch`) and run the restore with
`-e DATABASE_URL=postgres://cyanotype:$POSTGRES_PASSWORD@postgres:5432/scratch`.

If the server is lost: do steps 1-8 on a new Linode (with the same `.env.prod` and bucket), `dc up -d postgres`,
restore as above, then `dc up -d`.

## Datadog

The Agent stays off unless asked for. With `DD_API_KEY` in `.env.prod` (site `us5.datadoghq.com`, env `dev`
unless you set `DD_ENV`):

```sh
dc --profile datadog up -d
```

`just deploy` and the Deploy workflow include the profile when `DD_API_KEY` is set (workflow) or
`DEPLOY_PROFILES=datadog` (just). With the profile on, the worker and assets service also send queue and upload metrics to the Agent's DogStatsD (`datadog-agent:8125` on the compose network; nothing is published on the host), and the Agent collects container CPU/memory/OOM metrics for `cyanotype-*` containers only. The dashboard and monitors in `infra/datadog/` are imported by hand (`docs/observability.md`). Details and the log schema are in `docs/observability.md`. Services emit
`env:<DD_ENV>` and `version:<CYANOTYPE_VERSION>`. Use `DD_ENV=prod` for a future production instance.

## Known limitation: browser direct uploads and Linode CORS

Browsers cannot PUT file parts straight to the bucket without CORS on the bucket. Linode Object Storage in
us-ord-10 answers `PutBucketCors` with 501 (not implemented), so there is no way to configure CORS there today.
The web UI therefore sends parts through the assets service (`PUT /assets/uploads/{id}/parts/{n}`, at most 3
parts of 16 MiB buffered at once; see "Browser upload" in `docs/import.md`). This is temporary, until the bucket
moves to a CORS-capable region or provider. The gateway forwards these PUTs without a body-size limit (Cloudflare's
free plan allows 100 MB request bodies, 16 MiB parts fit). The CLI and native clients do not need CORS and keep
using presigned URLs.
Downloads through presigned GET URLs in `<img>` tags are not affected by CORS.

## Pre-go-live checklist

- [ ] Cloud Firewall: 80/443 from anywhere (direct mode) or from Cloudflare ranges only (proxied), 22 from your IP only; no other inbound.
- [ ] Cloudflare mode: SSL/TLS Full (strict), Origin CA cert in `certs/` (chmod 600), `CADDY_TLS_MODE=cloudflare-origin`; gateway log `client_ip` is your real IP.
- [ ] `PermitRootLogin no`, `PasswordAuthentication no`; unattended-upgrades active; Docker log rotation set.
- [ ] `.env.prod` is `chmod 600`, not in git, has a generated `POSTGRES_PASSWORD` and a bucket-scoped access key.
- [ ] `REGISTRATION_MODE=first_user` (or `closed` after creating your account); verify a second registration returns 403.
- [ ] DNS resolves to the server; `https://<hostname>/healthz` works with a valid certificate; http redirects to https.
- [ ] Session cookie has `Secure; HttpOnly; SameSite=Lax` (check in browser devtools or `curl -i`).
- [ ] Repeated bad logins return 429 (`for i in $(seq 15); do curl -s -o /dev/null -w '%{http_code}\n' -XPOST -H 'content-type: application/json' -d '{"email":"x@x.x","password":"nope"}' https://HOST/auth/login; done`).
- [ ] `dc ps` shows services healthy; `docker stats` memory is within limits; only 80/443 are published (`ss -ltnp`).
- [ ] A backup ran (`backup list` shows a dump) and a restore into a scratch database was rehearsed; a lifecycle rule handles old object versions.
- [ ] GHCR login is via the deploy workflow (no PAT on the server) or a read-only PAT you can revoke.
- [ ] Datadog: logs arrive with `env:dev` and the right `service` tags (if enabled).
- [ ] The CORS limitation is understood (no browser direct uploads yet).
