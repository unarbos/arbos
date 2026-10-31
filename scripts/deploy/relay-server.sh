#!/usr/bin/env bash
# Deploy the Arbos relay + the arbos.life landing on Jacob's server (Ubuntu, systemd, Caddy).
# Idempotent: run it again to upgrade the relay, refresh the site, or re-render the env file.
#
#   sudo -E scripts/deploy/relay-server.sh                 # install/upgrade everything
#   sudo -E scripts/deploy/relay-server.sh --import-data /path/to/relay-data   # also import a relay data dir
#   RELAY_VERSION=0.3.0.189 sudo -E scripts/deploy/relay-server.sh             # pin a release instead of the newest edge
#   sudo -E scripts/deploy/relay-server.sh --from-source main   # build arbos-relay + arbosctl from a git ref on this box
#                                                               # (any ref of github.com/unarbos/arbos; needs ~2 min on 128 cores)
#
# Secrets never live in the repo. They are written to /etc/arbos/relay.env (root:root 0600) from
# the environment at deploy time; the usual way is:
#   export OP_SERVICE_ACCOUNT_TOKEN=…   (1Password service account for the Arbos vault)
#   sudo -E scripts/deploy/relay-server.sh      # pulls the keys with `op read`
# or export OPENROUTER_API_KEY / OPENAI_API_KEY / DAYTONA_API_KEY / ARBOS_RELAY_ADMIN_TOKEN /
# ARBOS_RELAY_KEY / ARBOS_GITHUB_CLIENT_ID / ARBOS_GITHUB_CLIENT_SECRET yourself.
#
# Layout on the server:
#   /usr/local/bin/arbos-relay           binary from the GitHub edge release (linux-x86_64)
#   /var/lib/arbos-relay                 data dir (SQLite + blobs), owned by the `arbos` system user
#   /etc/arbos/relay.env                 systemd EnvironmentFile (root only)
#   /etc/systemd/system/arbos-relay.service
#   /home/const/arbos-www/Caddyfile      the :443 Caddy (arbos-www.service) — site + relay vhost
#   /home/const/arbos-www/site           landing page (copy of site/ in this repo)
#   /etc/arbos/download.caddy            `/download/mac` redirect, rewritten by refresh-mac-download.sh
set -euo pipefail

REPO_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
RELAY_VERSION="${RELAY_VERSION:-}"
PUBLIC_URL="${ARBOS_RELAY_PUBLIC_URL:-https://relay.arbos.life}"
# Server invite links are minted on the site host (arbos.life/s/<code> → deep link + fallback page).
SITE_URL="${ARBOS_SITE_URL:-https://arbos.life}"
LISTEN="${ARBOS_RELAY_LISTEN:-127.0.0.1:7700}"
DATA_DIR="${ARBOS_RELAY_DATA_DIR:-/var/lib/arbos-relay}"
ENV_FILE=/etc/arbos/relay.env
WWW_USER="${WWW_USER:-const}"
WWW_DIR="${WWW_DIR:-/home/$WWW_USER/arbos-www}"
IMPORT_DATA=""
FROM_SOURCE=""
BUILD_USER="${BUILD_USER:-${SUDO_USER:-const}}"
BUILD_DIR="${BUILD_DIR:-/home/$BUILD_USER/arbos-build}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --import-data) IMPORT_DATA="$2"; shift 2;;
    --from-source) FROM_SOURCE="$2"; shift 2;;
    *) echo "unknown arg $1" >&2; exit 2;;
  esac
done
[[ $EUID -eq 0 ]] || { echo "run as root (sudo -E)" >&2; exit 1; }

say() { printf '\n==> %s\n' "$*"; }

# ---- 1. relay binary: built from a git ref on this box, or from the newest edge release ----------
say "relay binary"
if [[ -n "$FROM_SOURCE" ]]; then
  # Build as $BUILD_USER (never root): rustup + the toolchain pinned by rust-toolchain.toml.
  id "$BUILD_USER" >/dev/null 2>&1 || { echo "build user $BUILD_USER does not exist" >&2; exit 1; }
  install -d -o "$BUILD_USER" -g "$BUILD_USER" "$BUILD_DIR"
  sudo -u "$BUILD_USER" -H bash -euo pipefail -c '
    export PATH="$HOME/.cargo/bin:$PATH"
    command -v rustup >/dev/null 2>&1 || curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable >/dev/null
    cd "'"$BUILD_DIR"'"
    [[ -d repo/.git ]] || git clone -q https://github.com/unarbos/arbos.git repo
    # GitHub only serves fetch-by-SHA for full 40-char ids, so fetch everything and resolve locally
    # (works for branch names, tags, full and abbreviated commit ids).
    git -C repo fetch -q --tags --force origin "+refs/heads/*:refs/remotes/origin/*"
    REF="'"$FROM_SOURCE"'"
    COMMIT=$(git -C repo rev-parse -q --verify "origin/$REF^{commit}" 2>/dev/null \
          || git -C repo rev-parse -q --verify "$REF^{commit}" 2>/dev/null) \
      || { echo "cannot resolve $REF in unarbos/arbos" >&2; exit 1; }
    # Never roll the relay back by accident: refuse a commit that is an ancestor of the one
    # running now (ALLOW_OLDER=1 overrides, for a deliberate rollback). sudo drops the environment
    # of the caller, so ALLOW_OLDER is passed into this shell as text, like REF.
    CUR=$(cat /etc/arbos/relay.deployed-sha 2>/dev/null || true)
    ALLOW_OLDER="'"${ALLOW_OLDER:-}"'"
    if [[ -n "$CUR" && "$CUR" != "$COMMIT" && "$ALLOW_OLDER" != 1 ]] && git -C repo merge-base --is-ancestor "$COMMIT" "$CUR" 2>/dev/null; then
      echo "refusing: $COMMIT is older than the deployed relay ($CUR); set ALLOW_OLDER=1 to roll back" >&2
      exit 1
    fi
    git -C repo checkout -q --detach "$COMMIT"
    echo "building $(git -C repo log --oneline -1)"
    cd repo && cargo build --release -p arbos-relay -p arbosctl 2>&1 | tail -3
  '
  install -m 0755 "$BUILD_DIR/repo/target/release/arbos-relay" /usr/local/bin/arbos-relay
  install -m 0755 "$BUILD_DIR/repo/target/release/arbosctl" /usr/local/bin/arbosctl
  # Read as $BUILD_USER: root without SUDO_UID (a systemd unit) is refused by git's safe.directory
  # check, and the marker must never be written empty — the rollback guard above trusts it.
  DEPLOYED_SHA=$(sudo -u "$BUILD_USER" -H git -C "$BUILD_DIR/repo" rev-parse HEAD)
  [[ "$DEPLOYED_SHA" =~ ^[0-9a-f]{40}$ ]] || { echo "cannot read the built commit ($DEPLOYED_SHA); relay.deployed-sha not written" >&2; exit 1; }
  printf '%s\n' "$DEPLOYED_SHA" > /etc/arbos/relay.deployed-sha
  chmod 0644 /etc/arbos/relay.deployed-sha
  echo "installed arbos-relay $(/usr/local/bin/arbos-relay --version | awk '{print $2}') from $FROM_SOURCE @ ${DEPLOYED_SHA:0:9}"
else
if [[ -z "$RELAY_VERSION" ]]; then
  RELAY_VERSION=$(curl -fsSL "https://api.github.com/repos/unarbos/arbos/releases?per_page=20" \
    | python3 -c 'import sys,json; r=[x for x in json.load(sys.stdin) if x["tag_name"].startswith("edge-")]; print(r[0]["tag_name"][5:] if r else "")')
  [[ -n "$RELAY_VERSION" ]] || { echo "no edge release found" >&2; exit 1; }
fi
CURRENT=$(/usr/local/bin/arbos-relay --version 2>/dev/null | awk '{print $2}' || true)
if [[ "$CURRENT" == "$RELAY_VERSION" ]]; then
  echo "arbos-relay $RELAY_VERSION already installed"
else
  TMP=$(mktemp -d)
  curl -fsSL -o "$TMP/relay.tgz" \
    "https://github.com/unarbos/arbos/releases/download/edge-$RELAY_VERSION/arbos-relay-$RELAY_VERSION-linux-x86_64.tar.gz"
  tar -xzf "$TMP/relay.tgz" -C "$TMP"
  BIN=$(find "$TMP" -type f -name 'arbos-relay*' ! -name '*.tar.gz' | head -1)
  install -m 0755 "$BIN" /usr/local/bin/arbos-relay
  rm -rf "$TMP"
  echo "installed arbos-relay $RELAY_VERSION"
fi
# arbosctl beside it (handy for ops)
if ! /usr/local/bin/arbosctl --version 2>/dev/null | grep -q "$RELAY_VERSION"; then
  TMP=$(mktemp -d)
  if curl -fsSL -o "$TMP/ctl.tgz" "https://github.com/unarbos/arbos/releases/download/edge-$RELAY_VERSION/arbosctl-$RELAY_VERSION-linux-x86_64.tar.gz"; then
    tar -xzf "$TMP/ctl.tgz" -C "$TMP"
    BIN=$(find "$TMP" -type f -name 'arbosctl*' ! -name '*.tar.gz' | head -1)
    install -m 0755 "$BIN" /usr/local/bin/arbosctl
  fi
  rm -rf "$TMP"
fi
fi  # FROM_SOURCE

# ---- 1b. 1Password CLI, pinned: the relay resolves `op://` references of connected vaults with it
# (D83 Phase 2). Installed or replaced only when the version differs; the token is never on disk.
say "1Password CLI"
OP_VERSION="${OP_VERSION:-2.30.3}"
if [[ "$(op --version 2>/dev/null || true)" != "$OP_VERSION" ]]; then
  case "$(uname -m)" in aarch64|arm64) OP_ARCH=arm64;; *) OP_ARCH=amd64;; esac
  OP_TMP=$(mktemp -d)
  curl -fsSL "https://cache.agilebits.com/dist/1P/op2/pkg/v${OP_VERSION}/op_linux_${OP_ARCH}_v${OP_VERSION}.zip" -o "$OP_TMP/op.zip"
  unzip -oq "$OP_TMP/op.zip" -d "$OP_TMP"
  install -m 0755 "$OP_TMP/op" /usr/local/bin/op
  rm -rf "$OP_TMP"
fi
echo "op $(op --version) at $(command -v op)"

# ---- 2. system user + data dir ---------------------------------------------------------------------
say "user + data dir"
id arbos >/dev/null 2>&1 || useradd --system --home "$DATA_DIR" --shell /usr/sbin/nologin arbos
mkdir -p "$DATA_DIR" /etc/arbos
if [[ -n "$IMPORT_DATA" ]]; then
  [[ -f "$IMPORT_DATA/relay.db" ]] || { echo "no relay.db in $IMPORT_DATA" >&2; exit 1; }
  if [[ -f "$DATA_DIR/relay.db" ]]; then
    BK="$DATA_DIR.bak-$(date -u +%Y%m%dT%H%M%SZ)"; cp -a "$DATA_DIR" "$BK"; echo "backed up existing data to $BK"
  fi
  rsync -a --delete "$IMPORT_DATA/" "$DATA_DIR/"
  echo "imported relay data from $IMPORT_DATA"
fi
# Own the data dir — but never descend into the user sandboxes' /work images mounted under
# $DATA_DIR/boxes/<user>/mnt (D89): their files belong to the sandbox's own users, and a
# recursive chown there breaks every workspace at the next start. `-xdev` stays on this fs.
find "$DATA_DIR" -xdev -not -path "$DATA_DIR/boxes/*/mnt" \( ! -user arbos -o ! -group arbos \) -exec chown arbos:arbos {} + 2>/dev/null || true
chmod 0750 "$DATA_DIR"

# ---- 3. env file (secrets) -------------------------------------------------------------------------
say "env file"
# Items are referenced by id: `op read` rejects titles with parentheses ("invalid character in secret reference").
# Run as root under `sudo -E`: HOME is still the invoking user's, and op refuses a ~/.config/op it does not own,
# so point it at root's config (the service-account token in the environment is all it needs).
opread() { HOME=/root XDG_CONFIG_HOME=/root/.config op read "$1" 2>/dev/null </dev/null || true; }
if command -v op >/dev/null 2>&1 && [[ -n "${OP_SERVICE_ACCOUNT_TOKEN:-}" ]]; then
  : "${OPENROUTER_API_KEY:=$(opread 'op://Arbos/xbirrctuljw2m6aieoway2szom/credential')}"
  : "${OPENAI_API_KEY:=$(opread 'op://Arbos/6k3kuhvc3xxwpgao4mblskj45e/notesPlain' | tr -d '[:space:]')}"
  : "${DAYTONA_API_KEY:=$(opread 'op://Arbos/fywmj6vtq5delybw5c7a53l2qa/notesPlain' | tr -d '[:space:]')}"
  : "${ARBOS_RELAY_ADMIN_TOKEN:=$(opread 'op://Arbos/pjwsl7tjrhcbzbqos2masde2u4/credential')}"
  : "${ARBOS_RELAY_KEY:=$(opread 'op://Arbos/t5a4ugctkgljhb3t6f732cxbeu/credential')}"
  : "${ARBOS_JWT_KEY:=$(opread 'op://Arbos/3f4ks7535m2riceop42hyohlaq/credential')}"
  : "${KLIPY_API_KEY:=$(opread 'op://Arbos/ikdzr7vpentedu2a2mhj3d5uye/credential' | tr -d '[:space:]')}"
  : "${ARBOS_GITHUB_CLIENT_ID:=$(opread 'op://Arbos/Arbos GitHub OAuth app/ARBOS_GITHUB_CLIENT_ID')}"
  : "${ARBOS_GITHUB_CLIENT_SECRET:=$(opread 'op://Arbos/Arbos GitHub OAuth app/ARBOS_GITHUB_CLIENT_SECRET')}"
  # Calls (LiveKit, scripts/deploy/livekit): absent until the SFU is set up; the relay keeps calls off without them.
  : "${LIVEKIT_URL:=$(opread 'op://Arbos/ypyfs6fm7dtkzphirv6ep7lfjq/LIVEKIT_URL')}"
  : "${LIVEKIT_API_KEY:=$(opread 'op://Arbos/ypyfs6fm7dtkzphirv6ep7lfjq/LIVEKIT_API_KEY')}"
  : "${LIVEKIT_API_SECRET:=$(opread 'op://Arbos/ypyfs6fm7dtkzphirv6ep7lfjq/LIVEKIT_API_SECRET')}"
fi
# Push notifications (APNs, crates/arbos-relay/src/apns.rs): ONE vault item titled "Arbos APNs auth key"
# (ARB-56) — the APNs .p8 from developer.apple.com → Keys attached as a file (AuthKey_<KEYID>.p8; or its
# PEM text in a field APNS_KEY_P8), the 10-character Key ID in a field APNS_KEY_ID (taken from the file
# name when the field is missing) and optionally APNS_TEAM_ID / APNS_TOPIC. The PEM goes to
# $APNS_KEY_FILE (root:arbos 0640 — the relay runs as `arbos`) and the env file points at it. Until the
# item exists the relay says "push notifications are off" once at start and keeps every registration.
APNS_KEY_FILE=/etc/arbos/apns-key.p8
APNS_ITEM="Arbos APNs auth key"
apns_from_vault() {
  local json pem="" file=""
  json=$(HOME=/root XDG_CONFIG_HOME=/root/.config op item get "$APNS_ITEM" --vault Arbos --format json 2>/dev/null </dev/null) || return 0
  [[ -n "$json" ]] || return 0
  apns_field() { jq -r --arg l "$1" '[.fields[]? | select((.label // "") == $l) | .value // empty][0] // empty' <<<"$json"; }
  : "${APNS_KEY_ID:=$(apns_field APNS_KEY_ID | tr -d '[:space:]')}"
  : "${APNS_TEAM_ID:=$(apns_field APNS_TEAM_ID | tr -d '[:space:]')}"
  : "${APNS_TOPIC:=$(apns_field APNS_TOPIC | tr -d '[:space:]')}"
  pem=$(apns_field APNS_KEY_P8)
  if [[ "$pem" != *"-----BEGIN"* ]]; then
    file=$(jq -r '[.files[]? | select((.name // "") | endswith(".p8"))][0].name // empty' <<<"$json")
    [[ -z "$file" ]] || pem=$(opread "op://Arbos/$APNS_ITEM/$file")
    if [[ -z "${APNS_KEY_ID:-}" && "$file" =~ AuthKey_([A-Za-z0-9]{10})\.p8 ]]; then APNS_KEY_ID=${BASH_REMATCH[1]}; fi
  fi
  if [[ "$pem" == *"-----BEGIN"* ]]; then
    (umask 077; printf '%s\n' "$pem" > "$APNS_KEY_FILE")
    chown root:arbos "$APNS_KEY_FILE"; chmod 0640 "$APNS_KEY_FILE"
    APNS_KEY_P8="$APNS_KEY_FILE"
    echo "APNs key from the vault item \"$APNS_ITEM\" → $APNS_KEY_FILE (key id ${APNS_KEY_ID:-?})"
  else
    echo "WARNING: vault item \"$APNS_ITEM\" has no .p8 file and no APNS_KEY_P8 field; push stays off"
  fi
}
if command -v op >/dev/null 2>&1 && [[ -n "${OP_SERVICE_ACCOUNT_TOKEN:-}" ]]; then
  apns_from_vault
fi
APNS_KEYS=(APNS_KEY_P8 APNS_KEY_ID APNS_TEAM_ID APNS_TOPIC)
# Chats on Matrix (D88, scripts/deploy/matrix): the homeserver next to the relay and the three
# appservice token pairs. ARBOS_MATRIX itself (off|dual|on) is an operator switch, never read from
# the vault: it is kept from the env file and defaults to off.
# Host fleet (D89): operator switches for gVisor sandboxes on this host, kept from the env file.
FLEET_KEYS=(ARBOS_DEFAULT_PROVIDER ARBOS_LOCAL_BOX_IMAGE ARBOS_HOST_MAX_BOXES ARBOS_HOST_MIN_FREE_DISK_GB
  ARBOS_HOST_EVICT_MIN_AVAILABLE_GB ARBOS_HOST_EVICT_MAX_SWAP_GB ARBOS_HOST_EVICT_LOAD
  ARBOS_BOX_CPUS ARBOS_BOX_MEM_GB ARBOS_BOX_WORK_GB ARBOS_BOX_PIDS ARBOS_BOX_NETWORK ARBOS_BOX_DNS ARBOS_BOX_RUNTIME)
MATRIX_KEYS=(ARBOS_MATRIX ARBOS_MATRIX_HS_URL ARBOS_MATRIX_SERVER_NAME ARBOS_MATRIX_AS_TOKEN_USERS ARBOS_MATRIX_HS_TOKEN_USERS
  ARBOS_MATRIX_AS_TOKEN_AGENTS ARBOS_MATRIX_HS_TOKEN_AGENTS ARBOS_MATRIX_AS_TOKEN_BRIDGES ARBOS_MATRIX_HS_TOKEN_BRIDGES)
# GIF search (#2235, crates/arbos-relay/src/gifs.rs): the KLIPY key is pulled from the vault item
# "KLIPY — Arbos GIF API key" (never a device secret); filter and budget have the relay's documented defaults.
# Hand-added KLIPY lines were lost on the 2026-10-29 00:11 deploy because the env file is rewritten from these
# lists — anything the relay reads must be listed here.
GIF_KEYS=(KLIPY_API_KEY KLIPY_API_BASE KLIPY_CONTENT_FILTER KLIPY_HOURLY_BUDGET)
# Remote feature flags the apps read from the relay (no client release to flip): operator switches,
# kept from the env file, never read from the vault. Unset = the relay's default (off).
#   ARBOS_FEATURE_VAULT_CONNECT=1  shows "Connect a vault…" (1Password, D83 Phase 2) in the apps again.
FEATURE_KEYS=(ARBOS_FEATURE_VAULT_CONNECT)
# Box upgrade carry (ARB-339): operator knobs, kept from the env file, never read from the vault.
# Unset = the relay's defaults (16 GiB ceiling, 256 MiB parts, 4 attempts, rebuildable trees carried).
#   ARBOS_UPGRADE_MAX_WORK_MIB / ARBOS_UPGRADE_PART_MIB / ARBOS_UPGRADE_ATTEMPTS / ARBOS_UPGRADE_SKIP_REBUILDABLE=1
UPGRADE_KEYS=(ARBOS_UPGRADE_MAX_WORK_MIB ARBOS_UPGRADE_PART_MIB ARBOS_UPGRADE_ATTEMPTS ARBOS_UPGRADE_SKIP_REBUILDABLE ARBOS_UPGRADE_CONCURRENT_CARRIES)
# Agent-hosted sites (#2060): the relay pushes published snapshots to the Cloudflare R2 bucket
# the edge Worker serves from. Endpoint from the vault config item, S3 keys from the account-wide
# R2 key pair ("Cloudflare R2 — S3 access key ID / secret key, account-wide, no IP lock").
# ARBOS_SITES_SCHEME (subdomain|path|two-level), ARBOS_SITES_DOMAIN, ARBOS_SITES_PUBLISH and
# ARBOS_SITES_ABUSE_EMAIL are operator switches kept from the env file. Without the R2 keys the
# relay accepts publishes but reports them as not live.
SITES_KEYS=(ARBOS_SITES_R2_BUCKET ARBOS_SITES_R2_ENDPOINT ARBOS_SITES_R2_ACCESS_KEY_ID ARBOS_SITES_R2_SECRET_ACCESS_KEY
  ARBOS_SITES_SCHEME ARBOS_SITES_DOMAIN ARBOS_SITES_PUBLISH ARBOS_SITES_ABUSE_EMAIL)
if command -v op >/dev/null 2>&1 && [[ -n "${OP_SERVICE_ACCOUNT_TOKEN:-}" ]]; then
  for k in "${MATRIX_KEYS[@]:1}"; do
    [[ -n "${!k:-}" ]] || printf -v "$k" '%s' "$(opread "op://Arbos/Arbos Matrix/$k")"
  done
  : "${ARBOS_SITES_R2_ENDPOINT:=$(opread 'op://Arbos/skikpa5qypq44rdm7tsm74dfea/credential' | tr -d '[:space:]')}"
  : "${ARBOS_SITES_R2_ACCESS_KEY_ID:=$(opread 'op://Arbos/5zos64ftkkhow3yd2ualigad44/credential' | tr -d '[:space:]')}"
  : "${ARBOS_SITES_R2_SECRET_ACCESS_KEY:=$(opread 'op://Arbos/s6gnhchuq3xhzji34tn5aukhsm/credential' | tr -d '[:space:]')}"
  : "${ARBOS_SITES_R2_BUCKET:=arbos-sites}"
fi
# keep values already in the env file when nothing new was supplied
if [[ -f "$ENV_FILE" ]]; then
  keep() { local k=$1; [[ -n "${!k:-}" ]] || printf -v "$k" '%s' "$(sed -n "s/^$k=//p" "$ENV_FILE" | head -1)"; }
  for k in OPENROUTER_API_KEY OPENAI_API_KEY DAYTONA_API_KEY ARBOS_RELAY_ADMIN_TOKEN ARBOS_RELAY_KEY ARBOS_JWT_KEY ARBOS_GITHUB_CLIENT_ID ARBOS_GITHUB_CLIENT_SECRET ARBOS_DAYTONA_SNAPSHOT LIVEKIT_URL LIVEKIT_API_KEY LIVEKIT_API_SECRET ARBOS_AUTH_REDIRECTS "${MATRIX_KEYS[@]}" "${FLEET_KEYS[@]}" "${FEATURE_KEYS[@]}" "${UPGRADE_KEYS[@]}" "${SITES_KEYS[@]}" "${APNS_KEYS[@]}" "${GIF_KEYS[@]}"; do keep "$k"; done
fi
# A kept APNS_KEY_P8 path whose file is gone is no key at all; the relay would log an error and run with push off.
if [[ -n "${APNS_KEY_P8:-}" && "$APNS_KEY_P8" != *"-----BEGIN"* && ! -r "$APNS_KEY_P8" ]]; then
  echo "WARNING: APNS_KEY_P8=$APNS_KEY_P8 is not readable; dropping it (push stays off)"
  APNS_KEY_P8=""
fi
if [[ -n "${APNS_KEY_P8:-}" ]]; then
  : "${APNS_TEAM_ID:=25SCF3Q2AK}"
  [[ -n "${APNS_KEY_ID:-}" ]] || echo "WARNING: APNS_KEY_ID is empty — set it in the vault item \"$APNS_ITEM\"; push stays off"
fi
# Where /auth/github/start may send the browser besides the apps' arbos:// scheme, loopback and the
# relay's own origin (comma-separated prefixes). The site's "Sign in with GitHub" lands on
# $SITE_URL/welcome, so the site is allowed by default; without it the relay answers 400 bad_redirect.
: "${ARBOS_AUTH_REDIRECTS:=$SITE_URL}"
[[ -n "${DAYTONA_API_KEY:-}" ]] || echo "WARNING: DAYTONA_API_KEY is empty — new projects will get process boxes"
[[ -n "${OPENROUTER_API_KEY:-}${OPENAI_API_KEY:-}" ]] || echo "WARNING: no model provider key"
: "${ARBOS_RELAY_ADMIN_TOKEN:=adm_$(openssl rand -hex 32)}"
: "${ARBOS_RELAY_KEY:=$(openssl rand -hex 32)}"
: "${ARBOS_JWT_KEY:=$(openssl rand -hex 32)}"
umask 077
cat > "$ENV_FILE" <<EOF
# Arbos relay — written by scripts/deploy/relay-server.sh on $(date -u +%FT%TZ). root only.
ARBOS_RELAY_LISTEN=$LISTEN
ARBOS_RELAY_PUBLIC_URL=$PUBLIC_URL
ARBOS_PUBLIC_URL=$PUBLIC_URL
ARBOS_SITE_URL=$SITE_URL
ARBOS_RELAY_DATA=$DATA_DIR
ARBOS_RELAY_ADMIN_TOKEN=$ARBOS_RELAY_ADMIN_TOKEN
ARBOS_RELAY_KEY=$ARBOS_RELAY_KEY
ARBOS_JWT_KEY=$ARBOS_JWT_KEY
ARBOS_AUTH_MODE=${ARBOS_AUTH_MODE:-github}
ARBOS_AUTH_REDIRECTS=$ARBOS_AUTH_REDIRECTS
OPENROUTER_API_KEY=${OPENROUTER_API_KEY:-}
OPENAI_API_KEY=${OPENAI_API_KEY:-}
DAYTONA_API_KEY=${DAYTONA_API_KEY:-}
ARBOS_GITHUB_CLIENT_ID=${ARBOS_GITHUB_CLIENT_ID:-}
ARBOS_GITHUB_CLIENT_SECRET=${ARBOS_GITHUB_CLIENT_SECRET:-}
ARBOS_GITHUB_OAUTH_CLIENT_ID=${ARBOS_GITHUB_CLIENT_ID:-}
ARBOS_GITHUB_OAUTH_CLIENT_SECRET=${ARBOS_GITHUB_CLIENT_SECRET:-}
ARBOS_OAUTH_REDIRECT_URL=$PUBLIC_URL/auth/github/callback
RUST_LOG=info,arbos_relay=info
EOF
# Daytona snapshot for NEW boxes (the relay's compiled-in default otherwise). Never written empty:
# clap takes an empty env value literally. Export ARBOS_DAYTONA_SNAPSHOT for one run to set it; it
# is kept on later runs.
[[ -z "${ARBOS_DAYTONA_SNAPSHOT:-}" ]] || echo "ARBOS_DAYTONA_SNAPSHOT=$ARBOS_DAYTONA_SNAPSHOT" >> "$ENV_FILE"
# LiveKit (calls): only when configured, so a relay without an SFU keeps calls disabled.
if [[ -n "${LIVEKIT_API_KEY:-}" && -n "${LIVEKIT_API_SECRET:-}" ]]; then
  printf 'LIVEKIT_URL=%s\nLIVEKIT_API_KEY=%s\nLIVEKIT_API_SECRET=%s\n' "${LIVEKIT_URL:-wss://livekit.arbos.life}" "$LIVEKIT_API_KEY" "$LIVEKIT_API_SECRET" >> "$ENV_FILE"
fi
# Host fleet (D89): only the keys that have a value; ARBOS_DEFAULT_PROVIDER unset means Daytona.
for k in "${FLEET_KEYS[@]}"; do
  [[ -z "${!k:-}" ]] || printf '%s=%s\n' "$k" "${!k}" >> "$ENV_FILE"
done
# Matrix (D88): only the keys that have a value; ARBOS_MATRIX unset means off.
for k in "${MATRIX_KEYS[@]}"; do
  [[ -z "${!k:-}" ]] || printf '%s=%s\n' "$k" "${!k}" >> "$ENV_FILE"
done
# GIF search: filter/budget defaults only travel with a key; no key means /gif/* answers 503 gif_unavailable.
if [[ -n "${KLIPY_API_KEY:-}" ]]; then
  : "${KLIPY_CONTENT_FILTER:=low}"
  : "${KLIPY_HOURLY_BUDGET:=90}"
  for k in "${GIF_KEYS[@]}"; do
    [[ -z "${!k:-}" ]] || printf '%s=%s\n' "$k" "${!k}" >> "$ENV_FILE"
  done
else
  echo "WARNING: KLIPY_API_KEY is empty — GIF search stays off (vault item \"KLIPY — Arbos GIF API key\")"
fi
# Remote feature flags: only the keys that have a value; unset means the relay default.
for k in "${FEATURE_KEYS[@]}"; do
  [[ -z "${!k:-}" ]] || printf '%s=%s\n' "$k" "${!k}" >> "$ENV_FILE"
done
# Box upgrade carry knobs: only the keys that have a value; unset means the relay default.
for k in "${UPGRADE_KEYS[@]}"; do
  [[ -z "${!k:-}" ]] || printf '%s=%s\n' "$k" "${!k}" >> "$ENV_FILE"
done
# Agent sites: only the keys that have a value.
for k in "${SITES_KEYS[@]}"; do
  [[ -z "${!k:-}" ]] || printf '%s=%s\n' "$k" "${!k}" >> "$ENV_FILE"
done
# APNs: only when the key exists (all three required by the relay; APNS_TOPIC optional).
for k in "${APNS_KEYS[@]}"; do
  [[ -z "${!k:-}" ]] || printf '%s=%s\n' "$k" "${!k}" >> "$ENV_FILE"
done
chmod 0600 "$ENV_FILE"; chown root:root "$ENV_FILE"
umask 022

# ---- 4. systemd unit -------------------------------------------------------------------------------
say "systemd"
cat > /etc/systemd/system/arbos-relay.service <<EOF
[Unit]
Description=Arbos relay (registry, pairing, sync rooms, model proxy, Daytona)
After=network-online.target
Wants=network-online.target

[Service]
User=arbos
Group=arbos
EnvironmentFile=$ENV_FILE
WorkingDirectory=$DATA_DIR
ExecStart=/usr/local/bin/arbos-relay serve
Restart=always
RestartSec=2
# Every client room websocket is one Caddy -> relay loopback socket. systemd's default soft limit is 1024;
# the relay hit it on 2026-10-25 02:00 UTC (975 ESTAB + db + listener) and accept() failed with EMFILE for
# 12 min — new sessions 502, chats empty, healthz still 200 on kept-alive connections. 65536 soft+hard.
LimitNOFILE=65536
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=$DATA_DIR
PrivateTmp=true

[Install]
WantedBy=multi-user.target
EOF
systemctl daemon-reload
systemctl enable --now arbos-relay.service >/dev/null
# Planned restart (#1347): the running relay first sends every room socket a `restart_soon` notice, then
# closes them with 1012 Service Restart and drains, so the apps redial at once instead of backing off
# (the Oct 22 "Reconnecting…" banners). Answers when ready; a relay too old for the endpoint (404) is
# simply restarted. RELAY_RESTART_NOTICE_S=0 skips the notice (hotfixes).
# A relay with a box upgrade or create in flight answers 409 box_work_in_flight and changes nothing
# (#2262: a restart at 01:03 on Oct 29 cut an upgrade between its new sandbox and its row). Wait for
# it, up to RELAY_BOX_WORK_WAIT_MAX_S, then restart anyway with force.
if systemctl is-active --quiet arbos-relay.service; then
  waited=0
  force=false
  while :; do
    code=$(curl -s -o /tmp/arbos-relay-restart.json -w '%{http_code}' --max-time 60 -X POST \
      -H "Authorization: Bearer $ARBOS_RELAY_ADMIN_TOKEN" -H 'Content-Type: application/json' \
      -d "{\"notice_s\":${RELAY_RESTART_NOTICE_S:-10},\"drain_s\":5,\"force\":$force}" "http://$LISTEN/admin/restart" || echo 000)
    [[ "$code" == 409 ]] || break
    if (( waited >= ${RELAY_BOX_WORK_WAIT_MAX_S:-1800} )); then
      echo "relay: box work still in flight after ${waited}s; restarting with force: $(cat /tmp/arbos-relay-restart.json)"
      force=true
      continue
    fi
    (( waited == 0 )) && echo "relay: restart waits for box work in flight: $(cat /tmp/arbos-relay-restart.json)"
    sleep 15; waited=$((waited + 15))
  done
  echo "relay: planned-restart handshake answered HTTP $code (waited ${waited}s for box work)"
fi
systemctl restart arbos-relay.service
sleep 2
systemctl is-active arbos-relay.service
curl -fsS "http://$LISTEN/healthz" >/dev/null && echo "relay healthz ok on $LISTEN"

# ---- 5. site + Caddy -------------------------------------------------------------------------------
say "site + caddy"
install -d -o "$WWW_USER" -g "$WWW_USER" "$WWW_DIR"
if [[ -d "$WWW_DIR/site" && ! -e "$WWW_DIR/site.pre-selfserve" && ! -f /var/lib/arbos-site/deployed-sha ]]; then
  mv "$WWW_DIR/site" "$WWW_DIR/site.pre-selfserve"; echo "kept previous site at $WWW_DIR/site.pre-selfserve"
fi
# The site itself is deployed from the repo's main by arbos-site-sync (timer below); seed it once
# from this checkout so a fresh server has a page before the first sync.
if [[ ! -d "$WWW_DIR/site" ]]; then
  cp -a "$REPO_DIR/site" "$WWW_DIR/site"; chown -R "$WWW_USER:$WWW_USER" "$WWW_DIR/site"
fi
install -m 0755 "$REPO_DIR/scripts/deploy/site-sync.sh" /usr/local/bin/arbos-site-sync
cat > /etc/systemd/system/arbos-site-sync.service <<'UNIT'
[Unit]
Description=Deploy site/ from github.com/unarbos/arbos main into the arbos.life web root
[Service]
Type=oneshot
ExecStart=/usr/local/bin/arbos-site-sync
UNIT
cat > /etc/systemd/system/arbos-site-sync.timer <<'UNIT'
[Unit]
Description=Sync the arbos.life site from the repo every 5 minutes
[Timer]
OnBootSec=1min
OnUnitActiveSec=5min
[Install]
WantedBy=timers.target
UNIT
systemctl daemon-reload
systemctl enable --now arbos-site-sync.timer >/dev/null
/usr/local/bin/arbos-site-sync || echo "WARNING: site sync failed; the seeded site stays"
install -m 0755 "$REPO_DIR/scripts/deploy/refresh-mac-download.sh" /usr/local/bin/arbos-refresh-mac-download
/usr/local/bin/arbos-refresh-mac-download --no-reload || true
if [[ -f "$WWW_DIR/Caddyfile" ]] && ! cmp -s "$REPO_DIR/scripts/deploy/Caddyfile" "$WWW_DIR/Caddyfile"; then
  cp -a "$WWW_DIR/Caddyfile" "$WWW_DIR/Caddyfile.bak-$(date -u +%Y%m%dT%H%M%SZ)"
fi
install -d -m 0755 /etc/arbos/sites.d   # per-service vhosts (scripts/deploy/livekit); the glob import tolerates an empty dir
# DNS-01 for the arbos.life vhost (#2060): the zone DNS token goes to /etc/arbos/www.env (root:www
# 0640) and the arbos-www.service drop-in points at it. Kept from the existing file when the vault
# is not reachable; the Caddyfile refers to {env.CLOUDFLARE_DNS_API_TOKEN}, so an empty value only
# means renewals of arbos.life fail (Caddy keeps serving the stored certificate meanwhile).
WWW_ENV=/etc/arbos/www.env
if command -v op >/dev/null 2>&1 && [[ -n "${OP_SERVICE_ACCOUNT_TOKEN:-}" ]]; then
  : "${CLOUDFLARE_DNS_API_TOKEN:=$(opread 'op://Arbos/azzmcdsl6wfcirvusnm557choy/credential' | tr -d '[:space:]')}"
fi
if [[ -z "${CLOUDFLARE_DNS_API_TOKEN:-}" && -f "$WWW_ENV" ]]; then
  CLOUDFLARE_DNS_API_TOKEN=$(sed -n 's/^CLOUDFLARE_DNS_API_TOKEN=//p' "$WWW_ENV" | head -1)
fi
if [[ -n "${CLOUDFLARE_DNS_API_TOKEN:-}" ]]; then
  (umask 077; printf 'CLOUDFLARE_DNS_API_TOKEN=%s\n' "$CLOUDFLARE_DNS_API_TOKEN" > "$WWW_ENV")
  chown "root:$WWW_USER" "$WWW_ENV"; chmod 0640 "$WWW_ENV"
else
  echo "WARNING: no CLOUDFLARE_DNS_API_TOKEN; arbos.life certificate renewals (DNS-01) will fail"
fi
install -d -m 0755 /etc/systemd/system/arbos-www.service.d
cat > /etc/systemd/system/arbos-www.service.d/env.conf <<EOF
[Service]
EnvironmentFile=-$WWW_ENV
EOF
systemctl daemon-reload
sed -e "s#@WWW_DIR@#$WWW_DIR#g" -e "s#@RELAY_LISTEN@#$LISTEN#g" "$REPO_DIR/scripts/deploy/Caddyfile" > "$WWW_DIR/Caddyfile"
chown "$WWW_USER:$WWW_USER" "$WWW_DIR/Caddyfile"
CADDY_BIN="$WWW_DIR/bin/caddy"; [[ -x "$CADDY_BIN" ]] || CADDY_BIN=$(command -v caddy)
# The cloudflare DNS module refuses an empty token at provision time, so validation needs the
# value in its environment (the service gets it from the drop-in); a placeholder keeps a tokenless
# validate meaningful for everything else in the file.
CLOUDFLARE_DNS_API_TOKEN="${CLOUDFLARE_DNS_API_TOKEN:-validate-only-placeholder}" \
  "$CADDY_BIN" validate --config "$WWW_DIR/Caddyfile" --adapter caddyfile >/dev/null
if systemctl is-active --quiet arbos-www.service; then systemctl reload arbos-www.service || systemctl restart arbos-www.service; else systemctl restart arbos-www.service; fi
# refresh timer for the Mac download redirect
cat > /etc/systemd/system/arbos-refresh-mac-download.service <<'EOF'
[Unit]
Description=Refresh the arbos.life /download/mac redirect from the newest edge release
[Service]
Type=oneshot
ExecStart=/usr/local/bin/arbos-refresh-mac-download
EOF
cat > /etc/systemd/system/arbos-refresh-mac-download.timer <<'EOF'
[Unit]
Description=Refresh /download/mac every 15 minutes
[Timer]
OnBootSec=2min
OnUnitActiveSec=15min
[Install]
WantedBy=timers.target
EOF
systemctl daemon-reload
systemctl enable --now arbos-refresh-mac-download.timer >/dev/null

# ---- 6. GitHub OAuth secret watcher (vault -> relay.env -> restart) --------------------------------
say "oauth secret watcher"
install -m 0755 "$REPO_DIR/scripts/deploy/oauth-secret-watch.sh" /usr/local/bin/arbos-oauth-secret-watch
if [[ -n "${OP_SERVICE_ACCOUNT_TOKEN:-}" ]]; then
  umask 077; printf 'OP_SERVICE_ACCOUNT_TOKEN=%s\n' "$OP_SERVICE_ACCOUNT_TOKEN" > /etc/arbos/op.env; chmod 0600 /etc/arbos/op.env; umask 022
fi
# Each tick reads the vault, whose account budget every lane shares; an operator who disabled (or
# masked) the timer meant it, so a redeploy leaves it that way. Read before the units are rewritten.
OAUTH_WATCH_STATE=$(systemctl is-enabled arbos-oauth-secret-watch.timer 2>/dev/null || true)
cat > /etc/systemd/system/arbos-oauth-secret-watch.service <<'UNIT'
[Unit]
Description=Pull the GitHub OAuth client id/secret from the Arbos vault into relay.env
[Service]
Type=oneshot
ExecStart=/usr/local/bin/arbos-oauth-secret-watch
UNIT
cat > /etc/systemd/system/arbos-oauth-secret-watch.timer <<'UNIT'
[Unit]
Description=Check the vault for the GitHub OAuth secret every 2 minutes
[Timer]
OnBootSec=1min
OnUnitActiveSec=2min
[Install]
WantedBy=timers.target
UNIT
systemctl daemon-reload
case "$OAUTH_WATCH_STATE" in
  disabled|masked|masked-runtime)
    echo "oauth secret watcher: timer is $OAUTH_WATCH_STATE by an operator; left as it is (restore: systemctl unmask arbos-oauth-secret-watch.timer; systemctl enable --now arbos-oauth-secret-watch.timer)";;
  *)
    systemctl enable --now arbos-oauth-secret-watch.timer >/dev/null
    /usr/local/bin/arbos-oauth-secret-watch || true;;
esac

say "done"
echo "relay: $PUBLIC_URL (listening on $LISTEN, data $DATA_DIR, env $ENV_FILE)"
echo "site:  https://arbos.life  (files $WWW_DIR/site, synced from origin/main site/ by arbos-site-sync.timer)"
echo "mac download: $PUBLIC_URL/download/mac  →  $(sed -n 's/.*redir [^ ]* \(https[^ ]*\).*/\1/p' /etc/arbos/download.caddy 2>/dev/null | head -1)"
