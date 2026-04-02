#!/usr/bin/env bash
#
# Arbos — one-command bootstrap
#
# Usage:
#   ./run.sh <discord_token> <guild_id> <openrouter_key>
#

set -e
set -o pipefail

# ── Colors ───────────────────────────────────────────────────────────────────

if [ -t 1 ] && [ "${TERM:-dumb}" != "dumb" ]; then
    GREEN=$'\033[0;32m' RED=$'\033[0;31m' CYAN=$'\033[0;36m'
    BOLD=$'\033[1m' DIM=$'\033[2m' NC=$'\033[0m'
else
    GREEN='' RED='' CYAN='' BOLD='' DIM='' NC=''
fi

ok()  { printf "  ${GREEN}+${NC} %s\n" "$1"; }
err() { printf "  ${RED}x${NC} %s\n" "$1"; }
die() { err "$1"; exit 1; }

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

# ── Args ─────────────────────────────────────────────────────────────────────

DISCORD_TOKEN="${1:-}"
GUILD_ID="${2:-}"
OPENROUTER_KEY="${3:-}"

[ -n "$DISCORD_TOKEN"  ] || die "Usage: ./run.sh <discord_token> <guild_id> <openrouter_key>"
[ -n "$GUILD_ID"       ] || die "Usage: ./run.sh <discord_token> <guild_id> <openrouter_key>"
[ -n "$OPENROUTER_KEY" ] || die "Usage: ./run.sh <discord_token> <guild_id> <openrouter_key>"

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

# ── 1. Preflight ─────────────────────────────────────────────────────────────

printf "  ${BOLD}Checking prerequisites${NC}\n\n"

for cmd in node claude pm2; do
    if command_exists "$cmd"; then
        case "$cmd" in
            node)   ok "node $(node -v)" ;;
            claude) ok "claude $(claude -v 2>/dev/null || echo '?')" ;;
            pm2)    ok "pm2 $(pm2 -v 2>/dev/null || echo '?')" ;;
        esac
    else
        die "$cmd is required"
    fi
done

printf "\n"

# ── 2. Environment ───────────────────────────────────────────────────────────

printf "  ${BOLD}Writing environment${NC}\n\n"

cat > .env <<EOF
DISCORD_TOKEN=${DISCORD_TOKEN}
GUILD_ID=${GUILD_ID}
OPENROUTER_KEY=${OPENROUTER_KEY}
WORKSPACE_ROOT=./workspace
EOF
ok ".env written"

printf "\n"

# ── 3. Build ─────────────────────────────────────────────────────────────────

printf "  ${BOLD}Installing & building${NC}\n\n"

run "Installing dependencies" npm install --silent
run "Compiling TypeScript" npx tsc

printf "\n"

# ── 4. Workspace ─────────────────────────────────────────────────────────────

printf "  ${BOLD}Preparing workspace${NC}\n\n"

mkdir -p workspace/general/chat
ok "Workspace initialized"
mkdir -p logs
ok "Log directory ready"

printf "\n"

# ── 5. Launch ─────────────────────────────────────────────────────────────────

printf "  ${BOLD}Starting Arbos${NC}\n\n"

pm2 delete arbos 2>/dev/null || true
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
