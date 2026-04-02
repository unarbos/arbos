#!/usr/bin/env bash
#
# Arbos — one-command bootstrap
#
# Usage:
#   ./run.sh [discord_token] [guild_id] [openrouter_key]
#
# All three arguments are optional if they were previously saved to the vault
#

set -e
set -o pipefail

# ── Colors ───────────────────────────────────────────────────────────────────

if [ -t 1 ] && [ "${TERM:-dumb}" != "dumb" ]; then
    GREEN=$'\033[0;32m' RED=$'\033[0;31m' CYAN=$'\033[0;36m'
    YELLOW=$'\033[0;33m' BOLD=$'\033[1m' DIM=$'\033[2m' NC=$'\033[0m'
else
    GREEN='' RED='' CYAN='' YELLOW='' BOLD='' DIM='' NC=''
fi

ok()   { printf "  ${GREEN}+${NC} %s\n" "$1"; }
warn() { printf "  ${YELLOW}!${NC} %s\n" "$1"; }
err()  { printf "  ${RED}x${NC} %s\n" "$1"; }
die()  { err "$1"; exit 1; }

command_exists() { command -v "$1" >/dev/null 2>&1; }

# ── Spinner ──────────────────────────────────────────────────────────────────

spin() {
    local pid=$1 msg="$2" i=0 chars='|/-\'
    printf "\033[?25l" 2>/dev/null || true
    while kill -0 "$pid" 2>/dev/null; do
        printf "\r  ${CYAN}%s${NC} %s" "${chars:$((i%4)):1}" "$msg"
        sleep 0.1 2>/dev/null || sleep 1
        i=$((i+1))
    done
    printf "\033[?25h" 2>/dev/null || true
    wait "$pid" 2>/dev/null; local code=$?
    if [ $code -eq 0 ]; then
        printf "\r  ${GREEN}+${NC} %s\n" "$msg"
    else
        printf "\r  ${RED}x${NC} %s\n" "$msg"
    fi
    return $code
}

run() {
    local msg="$1"; shift
    local tmp_out=$(mktemp) tmp_err=$(mktemp)
    "$@" >"$tmp_out" 2>"$tmp_err" &
    local pid=$!
    if ! spin $pid "$msg"; then
        if [ -s "$tmp_err" ]; then
            printf "\n    ${RED}${BOLD}stderr:${NC}\n"
            while IFS= read -r l; do printf "    ${DIM}%s${NC}\n" "$l"; done < "$tmp_err"
        elif [ -s "$tmp_out" ]; then
            printf "\n    ${RED}${BOLD}output:${NC}\n"
            tail -20 "$tmp_out" | while IFS= read -r l; do printf "    ${DIM}%s${NC}\n" "$l"; done
        fi
        printf "\n"
        rm -f "$tmp_out" "$tmp_err"
        return 1
    fi
    rm -f "$tmp_out" "$tmp_err"
}

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

# ── Banner ───────────────────────────────────────────────────────────────────

printf "\n${CYAN}${BOLD}"
printf "      _         _               \n"
printf "     / \\   _ __| |__   ___  ___ \n"
printf "    / _ \\ | '__| '_ \\ / _ \\/ __|\n"
printf "   / ___ \\| |  | |_) | (_) \\__ \\\\\n"
printf "  /_/   \\_\\_|  |_.__/ \\___/|___/\n"
printf "${NC}\n"

# ── 1. Install missing tools ────────────────────────────────────────────────

printf "  ${BOLD}Checking & installing prerequisites${NC}\n\n"

# ── Node.js ──────────────────────────────────────────────────────────────────

if command_exists node; then
    ok "node $(node -v) already installed"
else
    warn "node not found — installing via nvm"

    export NVM_DIR="${NVM_DIR:-$HOME/.nvm}"

    if [ ! -s "$NVM_DIR/nvm.sh" ]; then
        run "Installing nvm" bash -c \
            'curl -fsSL https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.1/install.sh | bash'
    fi

    # shellcheck disable=SC1091
    . "$NVM_DIR/nvm.sh"

    run "Installing Node.js LTS" nvm install --lts
    nvm use --lts >/dev/null 2>&1

    command_exists node || die "node installation failed"
    ok "node $(node -v) installed"
fi

command_exists npm || die "npm not found (should come with node)"

# ── pm2 ──────────────────────────────────────────────────────────────────────

if command_exists pm2; then
    ok "pm2 $(pm2 -v 2>/dev/null || echo '?') already installed"
else
    warn "pm2 not found — installing globally"
    run "Installing pm2" sudo npm install -g pm2
    command_exists pm2 || die "pm2 installation failed"
    ok "pm2 $(pm2 -v 2>/dev/null) installed"
fi

# ── Claude CLI ───────────────────────────────────────────────────────────────

if command_exists claude; then
    ok "claude $(claude -v 2>/dev/null || echo '?') already installed"
else
    warn "claude not found — installing Claude CLI"
    run "Installing Claude CLI" bash -c \
        'curl -fsSL https://claude.ai/install.sh | bash'

    # The installer puts it in ~/.local/bin — make sure it's on PATH
    if ! command_exists claude; then
        export PATH="$HOME/.local/bin:$PATH"
    fi
    command_exists claude || die "claude installation failed — add ~/.local/bin to your PATH and retry"
    ok "claude $(claude -v 2>/dev/null || echo '?') installed"
fi

printf "\n"

# ── 2. Environment ──────────────────────────────────────────────────────────

printf "  ${BOLD}Configuring environment${NC}\n\n"

mask() { local v="$1"; printf "%s…%s" "${v:0:4}" "${v: -4}"; }

env_val() { grep "^$1=" .env 2>/dev/null | head -1 | cut -d= -f2- || true; }

if [ -f .env ]; then
    EXISTING_VAULT_KEY=$(env_val VAULT_KEY)
fi

VAULT_KEY="${EXISTING_VAULT_KEY:-$(openssl rand -base64 32)}"

# Collect secrets (args > existing vault > prompt)
# On re-runs the vault already holds these, so we only prompt when truly missing.
NEED_SEED=false

vault_get() {
    if [ -f dist/vault.js ]; then
        VAULT_KEY="$VAULT_KEY" node -e "
            import('./dist/vault.js').then(v => v.getAllValues()).then(m => {
                process.stdout.write(m['$1'] || '');
            }).catch(() => {});
        " 2>/dev/null || true
    fi
}

DISCORD_TOKEN="${1:-}"
GUILD_ID="${2:-}"
OPENROUTER_KEY="${3:-}"

if [ -z "$DISCORD_TOKEN" ]; then DISCORD_TOKEN=$(vault_get DISCORD_TOKEN); fi
if [ -z "$GUILD_ID" ];       then GUILD_ID=$(vault_get GUILD_ID); fi
if [ -z "$OPENROUTER_KEY" ]; then OPENROUTER_KEY=$(vault_get OPENROUTER_KEY); fi

if [ -z "$DISCORD_TOKEN" ]; then
    printf "  ${BOLD}Discord bot token:${NC} " && read -rs DISCORD_TOKEN && printf "\n"
    [ -n "$DISCORD_TOKEN" ] || die "Discord token is required"
    NEED_SEED=true
fi
if [ -z "$GUILD_ID" ]; then
    printf "  ${BOLD}Discord guild (server) ID:${NC} " && read -r GUILD_ID
    [ -n "$GUILD_ID" ] || die "Guild ID is required"
    NEED_SEED=true
fi
if [ -z "$OPENROUTER_KEY" ]; then
    printf "  ${BOLD}OpenRouter API key:${NC} " && read -rs OPENROUTER_KEY && printf "\n"
    [ -n "$OPENROUTER_KEY" ] || die "OpenRouter key is required"
    NEED_SEED=true
fi

if [ -n "$1" ] || [ -n "$2" ] || [ -n "$3" ]; then
    NEED_SEED=true
fi

if [ "$NEED_SEED" = true ]; then
    ok "Credentials collected"
else
    ok "Credentials loaded from vault"
fi

cat > .env <<EOF
VAULT_KEY=${VAULT_KEY}
WORKSPACE_ROOT=./workspace
EOF
ok ".env written (vault key only — secrets stored in encrypted vault)"

printf "\n"

# ── 3. Build ─────────────────────────────────────────────────────────────────

printf "  ${BOLD}Installing & building${NC}\n\n"

run "Installing dependencies" npm install --silent
run "Compiling TypeScript" npx tsc

# ── 3b. Seed vault ───────────────────────────────────────────────────────────

if [ "$NEED_SEED" = true ]; then
    export VAULT_KEY
    run "Seeding secrets into encrypted vault" \
        node dist/seed-vault.js \
            "DISCORD_TOKEN=${DISCORD_TOKEN}" \
            "GUILD_ID=${GUILD_ID}" \
            "OPENROUTER_KEY=${OPENROUTER_KEY}"
    ok "Credentials stored in ~/.arbos/vault.enc"
fi

printf "\n"

# ── 4. Workspace ─────────────────────────────────────────────────────────────

printf "  ${BOLD}Preparing workspace${NC}\n\n"

mkdir -p workspace/general/chat
ok "Workspace initialized"
mkdir -p logs
ok "Log directory ready"

printf "\n"

# ── 5. Launch ────────────────────────────────────────────────────────────────

printf "  ${BOLD}Starting Arbos${NC}\n\n"

pm2 delete arbos >/dev/null 2>&1 || true
run "Starting PM2 process" pm2 start ecosystem.config.cjs
pm2 save --force >/dev/null 2>&1
ok "PM2 state saved"

TSC_LOG="$(pwd)/tsc-watch.log"
npx tsc --watch > "$TSC_LOG" 2>&1 &
TSC_PID=$!
ok "tsc --watch started (pid $TSC_PID)"

printf "\n"

# ── Done ─────────────────────────────────────────────────────────────────────

printf "  ${GREEN}${BOLD}Arbos is running${NC}\n"
printf "\n"
printf "  ${BOLD}Logs${NC}\n"
printf "    pm2 logs arbos         %s bot output\n" "${DIM}—${NC}"
printf "    tail -f %s  %s tsc watcher\n" "$TSC_LOG" "${DIM}—${NC}"
printf "\n"
printf "  ${BOLD}Manage${NC}\n"
printf "    pm2 stop arbos         %s stop bot\n" "${DIM}—${NC}"
printf "    pm2 restart arbos      %s restart bot\n" "${DIM}—${NC}"
printf "    kill %s                 %s stop tsc watcher\n" "$TSC_PID" "${DIM}—${NC}"
printf "\n"
