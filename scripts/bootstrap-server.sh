#!/usr/bin/env bash
# One-time bootstrap of a fresh Linode (Debian 12, Ubuntu 22.04/24.04) for the Cyanotype deploy flow
# (docs/deploy.md, scripts/deploy.sh). Run as root. Self-contained: no git, nothing fetched from GitHub.
# Safe to run again: every step checks before it changes anything.
#
# Usage (as root on the server):
#   DEPLOY_PUBKEY="ssh-ed25519 AAAA... cyanotype-deploy" bash bootstrap-server.sh
#   bash bootstrap-server.sh --pubkey "ssh-ed25519 AAAA... cyanotype-deploy" [options]
#
# Environment / flags:
#   DEPLOY_PUBKEY    (required) public key for the deploy user      --pubkey KEY
#   DEPLOY_USER      deploy account name (default: deploy)           --user NAME
#   HOSTNAME_FQDN    public hostname (default: photos-dev.cyanotype.app)   --hostname FQDN
#   HARDEN_SSH       1 (default) = key-only SSH; 0 = leave sshd alone      --no-harden-ssh
#   PUBLIC_IP        override the detected public IPv4 in the summary
#   DEPLOY_DIR       directory under the deploy user's home (default: cyanotype)
#   BOOTSTRAP_DRY_RUN=1 (or --no-services) skips apt, Docker, unattended-upgrades and service
#                    start/reload; it still does users, keys, directories, .env.prod, the SSH safety
#                    checks and the summary. For testing in a container.
set -euo pipefail

usage() {
  cat >&2 <<'USAGE'
Usage: DEPLOY_PUBKEY="ssh-ed25519 AAAA... comment" bash bootstrap-server.sh [options]

  DEPLOY_PUBKEY   (required) one-line SSH public key (.pub file contents) for the deploy user
  DEPLOY_USER     deploy account (default: deploy)
  HOSTNAME_FQDN   public hostname written to .env.prod (default: photos-dev.cyanotype.app)
  HARDEN_SSH      1 (default) or 0
  PUBLIC_IP       override the detected public IPv4
  DEPLOY_DIR      directory under the deploy user's home (default: cyanotype)

Options: --pubkey KEY  --user NAME  --hostname FQDN  --no-harden-ssh  --no-services  -h

Generate the keypair on YOUR machine, never on the server:
  ssh-keygen -t ed25519 -f ~/.ssh/cyanotype-deploy -C cyanotype-deploy -N ''
USAGE
}

while [ $# -gt 0 ]; do
  case "$1" in
    --pubkey)   DEPLOY_PUBKEY="${2:?--pubkey needs a value}"; shift 2 ;;
    --pubkey=*) DEPLOY_PUBKEY="${1#*=}"; shift ;;
    --user)     DEPLOY_USER="${2:?--user needs a value}"; shift 2 ;;
    --user=*)   DEPLOY_USER="${1#*=}"; shift ;;
    --hostname) HOSTNAME_FQDN="${2:?--hostname needs a value}"; shift 2 ;;
    --hostname=*) HOSTNAME_FQDN="${1#*=}"; shift ;;
    --no-harden-ssh) HARDEN_SSH=0; shift ;;
    --no-services) BOOTSTRAP_DRY_RUN=1; shift ;;
    -h|--help)  usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage; exit 2 ;;
  esac
done

DEPLOY_PUBKEY="${DEPLOY_PUBKEY:-}"
DEPLOY_USER="${DEPLOY_USER:-deploy}"
HOSTNAME_FQDN="${HOSTNAME_FQDN:-photos-dev.cyanotype.app}"
HARDEN_SSH="${HARDEN_SSH:-1}"
DEPLOY_DIR="${DEPLOY_DIR:-cyanotype}"
DRY="${BOOTSTRAP_DRY_RUN:-0}"

log()  { printf '==> %s\n' "$*"; }
warn() { printf '\n!!! WARNING: %s\n\n' "$*" >&2; }
die()  { printf 'error: %s\n' "$*" >&2; exit 1; }

# ---- Preflight ---------------------------------------------------------------------------------
[ "$(id -u)" -eq 0 ] || die "run as root"

# Strip CR/trailing whitespace from a pasted key.
DEPLOY_PUBKEY="$(printf '%s' "$DEPLOY_PUBKEY" | tr -d '\r' | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//')"
key_re='^(ssh-ed25519|ssh-rsa|ecdsa-sha2-nistp(256|384|521)|sk-ssh-ed25519@openssh\.com|sk-ecdsa-sha2-nistp256@openssh\.com) [A-Za-z0-9+/]+=*( [^[:cntrl:]]*)?$'
if [ -z "$DEPLOY_PUBKEY" ] || [[ "$DEPLOY_PUBKEY" == *$'\n'* ]] || ! [[ "$DEPLOY_PUBKEY" =~ $key_re ]]; then
  echo "error: DEPLOY_PUBKEY is missing or does not look like a one-line SSH public key." >&2
  usage
  exit 2
fi
[[ "$DEPLOY_USER" =~ ^[a-z_][a-z0-9_-]{0,31}$ ]] || die "invalid DEPLOY_USER '$DEPLOY_USER'"
[[ "$HOSTNAME_FQDN" =~ ^[A-Za-z0-9]([A-Za-z0-9.-]*[A-Za-z0-9])?$ ]] || die "invalid HOSTNAME_FQDN '$HOSTNAME_FQDN'"
[[ "$DEPLOY_DIR" =~ ^[A-Za-z0-9._-]+$ ]] || die "invalid DEPLOY_DIR '$DEPLOY_DIR'"
case "$HARDEN_SSH" in 0|1) ;; *) die "HARDEN_SSH must be 0 or 1" ;; esac
[ "$DEPLOY_USER" != root ] || die "DEPLOY_USER must not be root"

[ -r /etc/os-release ] || die "/etc/os-release not found; only Debian and Ubuntu are supported"
# shellcheck disable=SC1091
OS_ID="$(. /etc/os-release && printf '%s' "${ID:-}")"
# shellcheck disable=SC1091
OS_CODENAME="$(. /etc/os-release && printf '%s' "${VERSION_CODENAME:-}")"
case "$OS_ID" in
  debian|ubuntu) ;;
  *) die "unsupported OS '$OS_ID': this script supports Debian 12 and Ubuntu 22.04/24.04 only" ;;
esac
[ -n "$OS_CODENAME" ] || die "cannot determine the OS codename from /etc/os-release"
log "OS: $OS_ID $OS_CODENAME"
if [ "$DRY" = 1 ]; then warn "dry run (BOOTSTRAP_DRY_RUN=1 / --no-services): packages, Docker and service changes are SKIPPED (test mode)."; fi

export DEBIAN_FRONTEND=noninteractive

# ---- 1. Packages -------------------------------------------------------------------------------
if [ "$DRY" != 1 ]; then
  log "apt: update and base packages"
  apt-get update -q
  # tar/diffutils/findutils/sed/grep are used by scripts/deploy.sh on the server. No git: not needed.
  apt-get install -y -q --no-install-recommends ca-certificates curl gnupg unattended-upgrades jq \
    tar diffutils findutils sed grep iproute2 openssh-server
  printf '%s\n' 'APT::Periodic::Update-Package-Lists "1";' 'APT::Periodic::Unattended-Upgrade "1";' \
    > /etc/apt/apt.conf.d/20auto-upgrades
  systemctl enable --now unattended-upgrades >/dev/null 2>&1 || true
fi

# ---- 2. Docker Engine + compose plugin (Docker's official apt repo) -----------------------------
# daemon.json first, so the first start already has log rotation; merge, never clobber.
DAEMON_JSON=/etc/docker/daemon.json
daemon_changed=0
merge_daemon_json() {
  command -v jq >/dev/null || die "jq is required"
  local new tmp
  install -d -m 755 /etc/docker
  tmp="$(mktemp)"
  if [ -s "$DAEMON_JSON" ]; then
    jq -e 'type == "object"' "$DAEMON_JSON" >/dev/null 2>&1 || { rm -f "$tmp"; die "$DAEMON_JSON exists but is not a JSON object; fix it and re-run"; }
    new="$(jq -S '. + {"log-driver": "json-file", "log-opts": ((.["log-opts"] // {}) + {"max-size": "10m", "max-file": "5"})}' "$DAEMON_JSON")"
    if [ "$new" = "$(jq -S . "$DAEMON_JSON")" ]; then rm -f "$tmp"; log "daemon.json already has log rotation"; return 0; fi
  else
    new="$(jq -nS '{"log-driver": "json-file", "log-opts": {"max-size": "10m", "max-file": "5"}}')"
  fi
  printf '%s\n' "$new" > "$tmp"
  chmod 644 "$tmp"
  mv -f "$tmp" "$DAEMON_JSON"
  daemon_changed=1
  log "wrote $DAEMON_JSON (json-file, max-size 10m, max-file 5)"
}

if [ "$DRY" != 1 ]; then
  log "Docker Engine from download.docker.com ($OS_ID)"
  was_active=0
  systemctl is-active --quiet docker 2>/dev/null && was_active=1
  merge_daemon_json
  install -m 0755 -d /etc/apt/keyrings
  curl -fsSL "https://download.docker.com/linux/$OS_ID/gpg" -o /etc/apt/keyrings/docker.asc
  chmod a+r /etc/apt/keyrings/docker.asc
  printf 'deb [arch=%s signed-by=/etc/apt/keyrings/docker.asc] https://download.docker.com/linux/%s %s stable\n' \
    "$(dpkg --print-architecture)" "$OS_ID" "$OS_CODENAME" > /etc/apt/sources.list.d/docker.list
  apt-get update -q
  apt-get install -y -q docker-ce docker-ce-cli containerd.io docker-buildx-plugin docker-compose-plugin
  systemctl enable --now docker
  if [ "$daemon_changed" = 1 ] && [ "$was_active" = 1 ]; then
    log "restarting docker to apply daemon.json"
    systemctl restart docker
  fi
  docker compose version
fi

# ---- 3. Deploy user ----------------------------------------------------------------------------
if id "$DEPLOY_USER" >/dev/null 2>&1; then
  log "user $DEPLOY_USER exists"
else
  log "creating user $DEPLOY_USER (password locked)"
  useradd --create-home --shell /bin/bash "$DEPLOY_USER"
fi
passwd -l "$DEPLOY_USER" >/dev/null 2>&1 || true
getent group docker >/dev/null || groupadd --system docker
usermod -aG docker "$DEPLOY_USER"
DEPLOY_HOME="$(getent passwd "$DEPLOY_USER" | cut -d: -f6)"
[ -d "$DEPLOY_HOME" ] || die "home directory of $DEPLOY_USER not found"
echo "NOTE: membership of the 'docker' group is root-equivalent on this host. Keep the deploy key dedicated and private."

install -d -m 700 -o "$DEPLOY_USER" -g "$DEPLOY_USER" "$DEPLOY_HOME/.ssh"
AK="$DEPLOY_HOME/.ssh/authorized_keys"
[ -e "$AK" ] || install -m 600 -o "$DEPLOY_USER" -g "$DEPLOY_USER" /dev/null "$AK"
key_core="$(printf '%s' "$DEPLOY_PUBKEY" | awk '{print $1 " " $2}')"
if grep -qF -- "$key_core" "$AK"; then
  log "deploy key already in $AK"
else
  # Make sure the file ends with a newline before appending.
  if [ -s "$AK" ] && [ -n "$(tail -c1 "$AK")" ]; then printf '\n' >> "$AK"; fi
  printf '%s\n' "$DEPLOY_PUBKEY" >> "$AK"
  log "installed deploy key into $AK"
fi
chown "$DEPLOY_USER:$DEPLOY_USER" "$AK"; chmod 600 "$AK"

# ---- 4. Deploy directory and .env.prod ----------------------------------------------------------
APP_DIR="$DEPLOY_HOME/$DEPLOY_DIR"
install -d -m 700 -o "$DEPLOY_USER" -g "$DEPLOY_USER" "$APP_DIR" "$APP_DIR/certs"
ENV_FILE="$APP_DIR/.env.prod"
if [ -e "$ENV_FILE" ]; then
  log "$ENV_FILE exists; leaving it untouched"
else
  log "writing $ENV_FILE from the embedded .env.prod.example"
  tmp_env="$(umask 077; mktemp "$APP_DIR/.env.prod.XXXXXX")"
  {
    # Embedded copy of the repository's .env.prod.example (placeholders only).
    cat <<'ENV_PROD_EXAMPLE'
# Copy to .env.prod on the server, fill in every value, then `chmod 600 .env.prod`.
# Placeholders only: never commit real values. Used as:
#   docker compose --env-file .env.prod -f docker-compose.yml -f docker-compose.prod.yml ...
# (.env.prod is gitignored and dockerignored; keep it on the server only.)

# ---- Release ------------------------------------------------------------------------------
# Image tag to run: a release (v0.1.0), a commit (sha-1a2b3c4) or latest. Pin it to roll back.
CYANOTYPE_VERSION=sha-0000000

# ---- Gateway (Caddy, automatic HTTPS) ------------------------------------------------------
# Hostname of this instance (example: the dev-tier instance); its A (and optionally AAAA) record must already point at the VPS.
# Use a one-level subdomain of the zone, e.g. photos-dev.cyanotype.app (Cloudflare Universal SSL does not cover deeper names), and keep the apex for the project site.
CYANOTYPE_HOSTNAME=photos-dev.cyanotype.app

# TLS mode: auto = Let's Encrypt (clients connect directly; DNS not proxied).
# cloudflare-origin = hostname proxied by Cloudflare (orange cloud, SSL/TLS Full (strict)); Caddy serves the
# Origin CA cert from ./certs/origin.pem + ./certs/origin-key.pem (chmod 600) and trusts CF-Connecting-IP
# only from Cloudflare's ranges. See docs/deploy.md "Cloudflare".
CADDY_TLS_MODE=auto

# ---- Database ------------------------------------------------------------------------------
# Used when the Postgres volume is first created. Changing it later does NOT change the
# existing role's password (ALTER ROLE it too). Generate: openssl rand -base64 24 | tr -d '/+='
POSTGRES_PASSWORD=change-me

# ---- Auth ----------------------------------------------------------------------------------
# invite (default): the first account registers freely and becomes the admin; everyone after needs an invite link
# made on the Admin page. first_user: only the first account, then sign-up closes. open: anyone. closed: nobody.
REGISTRATION_MODE=invite

# ---- Object storage: Linode Object Storage (us-ord-10) --------------------------------------
S3_ENDPOINT=https://us-ord-10.linodeobjects.com
# Embedded in presigned URLs; clients must reach it. Same as S3_ENDPOINT for Linode.
S3_PUBLIC_ENDPOINT=https://us-ord-10.linodeobjects.com
S3_REGION=us-ord-10
S3_BUCKET=your-bucket-name
S3_ACCESS_KEY=your-access-key
S3_SECRET_KEY=your-secret-key
S3_FORCE_PATH_STYLE=true

# ---- Backups (nightly pg_dump -> s3://$S3_BUCKET/backups/postgres/) ------------------------
BACKUP_KEEP=14
# UTC, HH:MM
BACKUP_AT=03:17

# ---- Worker --------------------------------------------------------------------------------
WORKER_HEAVY_CONCURRENCY=2

# ---- Datadog (only read when started with --profile datadog) -------------------------------
DD_API_KEY=your-datadog-api-key
DD_SITE=us5.datadoghq.com
# Unified service tag `env`. dev for the dev-tier instance; use prod for a future production instance.
DD_ENV=dev
DD_HOSTNAME=cyanotype-dev
ENV_PROD_EXAMPLE
  } | sed -e "s|^CADDY_TLS_MODE=.*|CADDY_TLS_MODE=cloudflare-origin|" \
          -e "s|^CYANOTYPE_HOSTNAME=.*|CYANOTYPE_HOSTNAME=$HOSTNAME_FQDN|" > "$tmp_env"
  chown "$DEPLOY_USER:$DEPLOY_USER" "$tmp_env"; chmod 600 "$tmp_env"
  mv -n "$tmp_env" "$ENV_FILE"; rm -f "$tmp_env"
fi
chown "$DEPLOY_USER:$DEPLOY_USER" "$ENV_FILE"; chmod 600 "$ENV_FILE"

# ---- 5. SSH hardening (never without working key access for root AND deploy) ---------------------
has_keys() { [ -s "$1" ] && grep -qvE '^[[:space:]]*(#|$)' "$1"; }
DROPIN_DIR=/etc/ssh/sshd_config.d
DROPIN="$DROPIN_DIR/10-cyanotype.conf"
if [ "$HARDEN_SSH" != 1 ]; then
  log "HARDEN_SSH=0: leaving sshd configuration alone"
elif ! has_keys "$AK"; then
  warn "SSH hardening SKIPPED: $AK has no keys. Passwords stay enabled."
elif ! has_keys /root/.ssh/authorized_keys; then
  warn "SSH hardening SKIPPED: /root/.ssh/authorized_keys is empty or missing. Disabling passwords now could lock you out of root. Add your own public key to /root/.ssh/authorized_keys, then re-run this script."
else
  log "hardening sshd (keys only; root login by key stays allowed)"
  if [ "$DRY" = 1 ] && ! command -v sshd >/dev/null; then
    log "dry run: sshd not installed, writing the drop-in without validation"
  fi
  mkdir -p /run/sshd "$DROPIN_DIR"
  want="$(printf '%s\n' '# Managed by bootstrap-server.sh' 'PasswordAuthentication no' 'KbdInteractiveAuthentication no' 'PermitRootLogin prohibit-password')"
  if [ -f "$DROPIN" ] && [ "$(cat "$DROPIN")" = "$want" ]; then
    log "$DROPIN already in place"
  else
    printf '%s\n' "$want" > "$DROPIN"; chmod 644 "$DROPIN"
  fi
  if command -v sshd >/dev/null; then
    if ! sshd -t; then
      rm -f "$DROPIN"
      die "sshd -t failed with the hardening drop-in; removed it. SSH config left as before."
    fi
    # The drop-in must actually take effect (an Include line is needed in sshd_config).
    if [ "$(sshd -T 2>/dev/null | awk '$1=="passwordauthentication"{print $2}')" != "no" ]; then
      rm -f "$DROPIN"
      die "the drop-in has no effect (does /etc/ssh/sshd_config Include sshd_config.d/*.conf?); removed it."
    fi
  fi
  if [ "$DRY" = 1 ]; then
    log "dry run: not reloading ssh"
  else
    if systemctl list-unit-files ssh.service >/dev/null 2>&1 && systemctl cat ssh.service >/dev/null 2>&1; then
      systemctl reload ssh
    else
      systemctl reload sshd
    fi
    log "ssh reloaded. Keep this session open and test a new login before closing it."
  fi
fi

# ---- 6. Summary --------------------------------------------------------------------------------
HOSTKEY_PUB=/etc/ssh/ssh_host_ed25519_key.pub
command -v ssh-keygen >/dev/null && ssh-keygen -A >/dev/null 2>&1 || true
[ -r "$HOSTKEY_PUB" ] || die "$HOSTKEY_PUB not found (is openssh-server installed with an ed25519 host key?)"
IP="${PUBLIC_IP:-}"
if [ -z "$IP" ]; then
  IP="$(ip -4 route get 1.1.1.1 2>/dev/null | awk '{for(i=1;i<NF;i++) if($i=="src"){print $(i+1); exit}}' || true)"
fi
if [ -z "$IP" ]; then
  IP="<PUBLIC_IPV4>"
  warn "could not detect the public IPv4; replace $IP below or re-run with PUBLIC_IP=..."
elif [[ "$IP" =~ ^(10\.|127\.|192\.168\.|172\.(1[6-9]|2[0-9]|3[01])\.) ]]; then
  warn "detected address $IP is private; re-run with PUBLIC_IP=<public ipv4> for a correct DEPLOY_KNOWN_HOSTS."
fi
HK_TYPE_BLOB="$(awk '{print $1 " " $2}' "$HOSTKEY_PUB")"
FPR="$(ssh-keygen -lf "$HOSTKEY_PUB" -E sha256 | awk '{print $2}')"

cat <<SUMMARY

================================================================================================
Bootstrap complete. Copy these into GitHub (Settings -> Environments -> dev):

DEPLOY_HOST=$IP
DEPLOY_USER=$DEPLOY_USER
DEPLOY_KNOWN_HOSTS (secret; one line, read from this server's own host key):
$IP $HK_TYPE_BLOB

Host key SHA256 fingerprint: $FPR

Next steps
  1. Fill in the secrets in $ENV_FILE (POSTGRES_PASSWORD, S3_*, DD_*). It was created with
     CADDY_TLS_MODE=cloudflare-origin and CYANOTYPE_HOSTNAME=$HOSTNAME_FQDN. Mode 600, owner $DEPLOY_USER.
  2. Paste the Cloudflare Origin CA certificate and key into $APP_DIR/certs/origin.pem and
     $APP_DIR/certs/origin-key.pem, then:
       chown $DEPLOY_USER:$DEPLOY_USER $APP_DIR/certs/origin.pem $APP_DIR/certs/origin-key.pem
       chmod 600 $APP_DIR/certs/origin.pem $APP_DIR/certs/origin-key.pem
  3. In GitHub, environment "dev": secrets DEPLOY_HOST, DEPLOY_USER, DEPLOY_SSH_KEY (the PRIVATE key
     that matches the public key you gave this script), DEPLOY_KNOWN_HOSTS; variable CYANOTYPE_HOSTNAME=$HOSTNAME_FQDN.
  4. Run Actions -> Deploy -> Run workflow.
  5. Once the site works, restrict the Linode Cloud Firewall's inbound 80/443 to Cloudflare's IP ranges.

Reminders
  * No host firewall (ufw) was configured: use the Linode Cloud Firewall. ufw does not filter
    Docker-published ports anyway.
  * The '$DEPLOY_USER' user is in the docker group, which is root-equivalent.
  * SSH: $( [ -f "$DROPIN" ] && echo "key-only (drop-in $DROPIN); root may log in by key." || echo "NOT hardened; password login is still enabled unless you changed it.")
================================================================================================
SUMMARY
