#!/usr/bin/env bash
#
# Agent — one-command install
#
# Usage:
#   ./install.sh
#   curl -fsSL <url>/install.sh | bash   (interactive)
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

# ── Detect context ───────────────────────────────────────────────────────────

INSTALL_DIR=""
if [ -n "${BASH_SOURCE[0]:-}" ] && [ -f "${BASH_SOURCE[0]}" ]; then
    INSTALL_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
fi
[ -z "$INSTALL_DIR" ] && INSTALL_DIR="$(pwd)"

HAS_TTY=false
if [ -t 0 ] || { [ -e /dev/tty ] && (echo >/dev/tty) 2>/dev/null; }; then
    HAS_TTY=true
fi

# ── Banner ───────────────────────────────────────────────────────────────────

printf "\n${CYAN}${BOLD}"
printf "      _                    _   \n"
printf "     / \\   __ _  ___ _ __ | |_ \n"
printf "    / _ \\ / _\` |/ _ \\ '_ \\| __|\n"
printf "   / ___ \\ (_| |  __/ | | | |_ \n"
printf "  /_/   \\_\\__, |\\___|_| |_|\\__|\n"
printf "          |___/                \n"
printf "${NC}\n"

# ── 1. Detect package manager ────────────────────────────────────────────────

pkg_install() {
    if command_exists apt-get; then
        sudo apt-get update -qq && sudo apt-get install -y -qq "$@"
    elif command_exists dnf; then
        sudo dnf install -y -q "$@"
    elif command_exists yum; then
        sudo yum install -y -q "$@"
    elif command_exists pacman; then
        sudo pacman -S --noconfirm --needed "$@"
    elif command_exists brew; then
        brew install "$@"
    else
        die "No supported package manager found (apt/dnf/yum/pacman/brew)"
    fi
}

# ── 2. Install prerequisites ────────────────────────────────────────────────

printf "  ${BOLD}Installing prerequisites${NC}\n\n"

for cmd in git python3 curl vim; do
    if command_exists "$cmd"; then
        ok "$cmd"
    else
        run "Installing $cmd" pkg_install "$cmd"
        command_exists "$cmd" || die "Failed to install $cmd"
    fi
done

printf "\n"

# ── 3. Install tooling ──────────────────────────────────────────────────────

printf "  ${BOLD}Installing tooling${NC}\n\n"

export PATH="$HOME/.local/bin:$HOME/.cargo/bin:$PATH"

# uv
if command_exists uv; then
    ok "uv already installed"
else
    run "Installing uv" bash -c "curl -LsSf https://astral.sh/uv/install.sh | sh"
    export PATH="$HOME/.local/bin:$HOME/.cargo/bin:$PATH"
    command_exists uv || die "uv install failed"
fi

# Cursor CLI (agent command)
export PATH="$HOME/.cursor/bin:$HOME/.local/bin:$PATH"
if command_exists agent; then
    ok "Cursor CLI (agent) already installed"
else
    run "Installing Cursor CLI" bash -c "curl https://cursor.com/install -fsSL | bash"
    export PATH="$HOME/.cursor/bin:$HOME/.local/bin:$PATH"
    command_exists agent || die "'agent' command not found — install Cursor CLI from https://cursor.com/install"
fi

# PATH persistence
SHELL_RC="$HOME/.bashrc"
[[ -n "${ZSH_VERSION:-}" ]] && SHELL_RC="$HOME/.zshrc"
PATHS_TO_ADD="$HOME/.cursor/bin:$HOME/.local/bin"
if [[ ":$PATH:" != *":$HOME/.cursor/bin:"* ]] || [[ ":$PATH:" != *":$HOME/.local/bin:"* ]]; then
    if ! grep -q '.cursor/bin' "$SHELL_RC" 2>/dev/null; then
        echo 'export PATH="$HOME/.cursor/bin:$HOME/.local/bin:$PATH"' >> "$SHELL_RC"
        ok "Added ~/.cursor/bin and ~/.local/bin to PATH in $SHELL_RC"
    fi
fi

printf "\n"

# ── 4. Python environment ───────────────────────────────────────────────────

printf "  ${BOLD}Setting up project${NC}\n\n"

cd "$INSTALL_DIR"

if [ ! -d ".venv" ]; then
    run "Creating Python environment" uv venv .venv
else
    ok "Python environment exists"
fi

source .venv/bin/activate
run "Installing dependencies" uv pip install -e .

mkdir -p history scratch

printf "\n"

# ── 5. Edit PROMPT.md ───────────────────────────────────────────────────────

printf "  ${BOLD}Prompt${NC}\n\n"

printf "  ${CYAN}PROMPT.md${NC} is the system prompt fed to the agent at every step.\n"
printf "  It tells the agent who it is, where to find its goal, how to\n"
printf "  store history, and any persistent hints you want to pass along.\n\n"
printf "  ${DIM}You're about to open it in vim — edit it to your liking,${NC}\n"
printf "  ${DIM}then save and quit (:wq) to continue the install.${NC}\n\n"

if [ "$HAS_TTY" = true ]; then
    printf "  ${DIM}Press any key to open PROMPT.md in vim...${NC}"
    read -rsn1 </dev/tty 2>/dev/null || true
    printf "\n\n"

    vim "$INSTALL_DIR/PROMPT.md" </dev/tty >/dev/tty

    ok "PROMPT.md saved"
else
    [ -f "$INSTALL_DIR/PROMPT.md" ] || die "No TTY and no PROMPT.md — cannot configure"
    ok "PROMPT.md exists (no TTY, skipping editor)"
fi

printf "\n"

# ── 6. Authenticate Cursor agent ─────────────────────────────────────────────

printf "  ${BOLD}Authentication${NC}\n\n"

# Check .env for CURSOR_API_KEY first
if [ -z "${CURSOR_API_KEY:-}" ] && [ -f "$INSTALL_DIR/.env" ]; then
    _env_key=$(grep "^CURSOR_API_KEY=" "$INSTALL_DIR/.env" 2>/dev/null | cut -d'=' -f2-)
    if [ -n "$_env_key" ]; then
        export CURSOR_API_KEY="$_env_key"
    fi
fi

if [ -n "${CURSOR_API_KEY:-}" ]; then
    ok "Using CURSOR_API_KEY from environment"
elif agent status >/dev/null 2>&1; then
    ok "Cursor agent already authenticated"
else
    printf "  ${DIM}The agent CLI needs to be logged in to work.${NC}\n\n"

    # Detect if we're on a remote/headless machine (no DISPLAY, no macOS)
    IS_REMOTE=false
    if [ -z "${DISPLAY:-}" ] && [ -z "${WAYLAND_DISPLAY:-}" ] && [ "$(uname -s)" != "Darwin" ]; then
        IS_REMOTE=true
    fi
    # Also treat SSH sessions as remote
    if [ -n "${SSH_CONNECTION:-}" ] || [ -n "${SSH_TTY:-}" ]; then
        IS_REMOTE=true
    fi

    if [ "$IS_REMOTE" = true ]; then
        printf "  ${CYAN}Remote machine detected${NC}\n"
        printf "  ${DIM}A login URL will be printed below.${NC}\n"
        printf "  ${DIM}Copy and open it in your local browser to authenticate.${NC}\n\n"
    else
        printf "  ${DIM}This will open your browser to authenticate.${NC}\n\n"
    fi

    if [ "$HAS_TTY" = true ]; then
        # NO_OPEN_BROWSER prevents agent login from trying to launch a browser
        # (essential for remote/headless machines — it prints the URL instead)
        if [ "$IS_REMOTE" = true ]; then
            NO_OPEN_BROWSER=1 agent login </dev/tty 2>&1
        else
            agent login </dev/tty 2>&1
        fi

        if agent status >/dev/null 2>&1; then
            ok "Cursor agent authenticated"
        else
            printf "\n"
            err "Authentication did not complete"
            printf "  ${DIM}You can authenticate later:${NC}\n"
            printf "  ${DIM}  Remote:  NO_OPEN_BROWSER=1 agent login${NC}\n"
            printf "  ${DIM}  Local:   agent login${NC}\n"
            printf "  ${DIM}  API key: export CURSOR_API_KEY=your_key${NC}\n\n"
            die "Run 'agent login' manually and retry ./install.sh"
        fi
    else
        err "No TTY available for interactive login"
        printf "  ${DIM}Options:${NC}\n"
        printf "  ${DIM}  1. Run interactively:  NO_OPEN_BROWSER=1 agent login${NC}\n"
        printf "  ${DIM}  2. Use API key:        export CURSOR_API_KEY=your_key${NC}\n"
        printf "  ${DIM}  Then re-run ./install.sh${NC}\n"
        die "Authentication required — see above"
    fi
fi

printf "\n"

# ── 7. Collect .env (optional extra vars) ────────────────────────────────────

printf "  ${BOLD}Environment variables${NC}\n\n"

printf "  ${DIM}Add any extra env vars your agent needs (API keys, etc.)${NC}\n"
printf "  ${DIM}These will be available to the agent at runtime via .env${NC}\n\n"

if [ "$HAS_TTY" = true ]; then
    printf "  ${DIM}Paste KEY=VALUE lines, one per line.${NC}\n"
    printf "  ${DIM}Press Enter on an empty line when done (or just Enter to skip).${NC}\n\n"

    ENV_CONTENT=""
    while true; do
        printf "  ${CYAN}>${NC} "
        IFS= read -r line </dev/tty 2>/dev/null || break
        [ -z "$line" ] && break
        ENV_CONTENT+="$line"$'\n'
    done

    if [ -n "$ENV_CONTENT" ]; then
        printf '%s' "$ENV_CONTENT" > "$INSTALL_DIR/.env"
        ok ".env saved"
    else
        if [ -f "$INSTALL_DIR/.env" ]; then
            ok ".env unchanged (kept existing)"
        else
            ok "No extra env vars (skipped)"
        fi
    fi
else
    if [ -f "$INSTALL_DIR/.env" ]; then
        ok ".env exists"
    else
        ok "No .env file (skipped)"
    fi
fi

printf "\n"

# ── 8. Start agent ───────────────────────────────────────────────────────────

printf "  ${BOLD}Starting agent${NC}\n\n"

if ! command_exists agent; then
    die "'agent' command not found in PATH — Cursor CLI is required"
fi
ok "agent CLI found at $(which agent)"

LAUNCH_SCRIPT="$INSTALL_DIR/.agent-launch.sh"
cat > "$LAUNCH_SCRIPT" <<LAUNCH
#!/usr/bin/env bash
export PATH="\$HOME/.cursor/bin:\$HOME/.local/bin:\$HOME/.cargo/bin:\$PATH"
cd "$INSTALL_DIR"
set -a; [ -f .env ] && source .env; set +a
source .venv/bin/activate
exec python3 agent.py 2>&1
LAUNCH
chmod +x "$LAUNCH_SCRIPT"

PM2_NAME="agent"

# Install pm2 if needed
if ! command_exists pm2; then
    if ! command_exists npm && ! command_exists npx; then
        if command_exists brew; then
            run "Installing Node.js" brew install node
        elif command_exists apt-get; then
            run "Installing Node.js" bash -c "sudo apt-get update -qq && sudo apt-get install -y -qq nodejs npm"
        else
            die "npm/node required for pm2 — install Node.js first"
        fi
    fi
    run "Installing pm2" npm install -g pm2
    command_exists pm2 || die "pm2 install failed"
fi

# Stop existing instance if running
pm2 delete "$PM2_NAME" 2>/dev/null || true

pm2 start "$LAUNCH_SCRIPT" \
    --name "$PM2_NAME" \
    --cwd "$INSTALL_DIR" \
    --log "$INSTALL_DIR/logs/agent.log" \
    --time \
    --restart-delay 10000

pm2 save 2>/dev/null || true

sleep 2
if pm2 pid "$PM2_NAME" >/dev/null 2>&1 && [ -n "$(pm2 pid "$PM2_NAME")" ]; then
    ok "Agent running"
else
    err "Agent may not have started — check logs:"
    printf "    ${DIM}pm2 logs $PM2_NAME${NC}\n"
fi

# ── Done ─────────────────────────────────────────────────────────────────
printf "\n"
printf "  ${GREEN}${BOLD}Agent is live${NC}\n"
printf "\n"
printf "  ${DIM}logs${NC}     pm2 logs $PM2_NAME\n"
printf "  ${DIM}status${NC}   pm2 status\n"
printf "  ${DIM}stop${NC}     pm2 stop $PM2_NAME\n"
printf "  ${DIM}start${NC}    pm2 start $PM2_NAME\n"
printf "  ${DIM}restart${NC}  pm2 restart $PM2_NAME\n"
printf "\n"
