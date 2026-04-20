#!/usr/bin/env bash
# arbos runner.
#
# Subcommands:
#   ./run.sh                  install (idempotent fast path) then start agent under PM2
#   ./run.sh install          install only
#   ./run.sh start | up       same as default
#   ./run.sh stop | down      pm2 stop arbos-<machine>
#   ./run.sh restart          pm2 reload arbos-<machine>
#   ./run.sh logs | tail      pm2 logs arbos-<machine>
#   ./run.sh status | ps      pm2 status arbos-<machine>
#
# Bring-up sequence on first run:
#   1. Install tailscale (macOS brew / Linux curl) and resolve this machine's name.
#   2. Install doppler, log in, set scope (defaults: arbos / dev_arbos).
#   3. Read 4 required ARBOS_TELEGRAM_* base secrets, plus per-machine token
#      (ARBOS_<MACHINE>_TELE_TOKEN) and shared chat_id/supergroup_id.
#   4. Set up Python venv via uv, install arbos.
#   5. Run `arbos install` under doppler run (idempotent fast path if already
#      complete, otherwise drives the BotFather + supergroup + topic flow).
#   6. Push doppler writeback values back to doppler.
#   7. Install Node + PM2 if missing.
#   8. pm2 start (or reload) arbos-<machine>, pm2 save, hint about pm2 startup.
set -euo pipefail

# When invoked via `curl … | bash`, BASH_SOURCE[0] is unset / empty and there
# is no local checkout to point REPO_ROOT at. Clone (or fast-forward) the
# arbos source into <cwd>/.arbos/src and re-exec ourselves from the clone.
if [[ -z "${BASH_SOURCE[0]:-}" || ! -f "${BASH_SOURCE[0]:-}" ]]; then
  ARBOS_REMOTE="${ARBOS_REMOTE:-https://github.com/unarbos/arbos.git}"
  ARBOS_BRANCH="${ARBOS_BRANCH:-main}"
  _src="$(pwd -P)/.arbos/src"

  command -v git >/dev/null 2>&1 || {
    case "$(uname -s)" in
      Linux)  command -v apt-get >/dev/null && sudo apt-get update -qq \
                && sudo apt-get install -y git ;;
      Darwin) command -v brew    >/dev/null && brew install git ;;
    esac
    command -v git >/dev/null 2>&1 || {
      echo "arbos bootstrap: git is required but not installed" >&2; exit 1; }
  }

  if [[ -d "$_src/.git" ]]; then
    git -C "$_src" fetch --quiet --depth 1 origin "$ARBOS_BRANCH"
    git -C "$_src" reset --hard --quiet "origin/$ARBOS_BRANCH"
  else
    [[ -e "$_src" ]] && {
      echo "arbos bootstrap: $_src exists and is not a git checkout" >&2; exit 1; }
    mkdir -p "$(dirname "$_src")"
    git clone --quiet --depth 1 --branch "$ARBOS_BRANCH" "$ARBOS_REMOTE" "$_src"
  fi

  # Reattach stdin to the controlling terminal: when piped via `curl | bash`,
  # stdin is the (now-exhausted) curl pipe, so any later input() prompt
  # (e.g. aiotdlib's "Enter SMS code:") would EOF-loop forever.
  if [[ -r /dev/tty ]]; then
    exec bash "$_src/run.sh" "$@" </dev/tty
  else
    exec bash "$_src/run.sh" "$@"
  fi
fi

# REPO_ROOT = where the arbos source / venv / run.sh live. Used only to
# locate the python virtualenv and the editable package source. The user's
# working directory at invocation time ("install root") is what owns
# .arbos/, the doppler scope, and the cursor-agent workdir -- so a single
# checkout of the arbos repo can drive any number of separate workspaces.
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" && pwd)"
INVOKE_DIR="$(pwd -P)"

STORAGE_DIR="${INVOKE_DIR}/.arbos"
WRITEBACK_PATH="${STORAGE_DIR}/doppler_writeback.json"
AGENT_LOG_PATH="${STORAGE_DIR}/agent.log"

DOPPLER_PROJECT="${DOPPLER_PROJECT:-arbos}"
DOPPLER_CONFIG="${DOPPLER_CONFIG:-dev_arbos}"

REQUIRED_VARS=(
  ARBOS_TELEGRAM_API_ID
  ARBOS_TELEGRAM_API_HASH
  ARBOS_TELEGRAM_PHONE
  ARBOS_TELEGRAM_NAME
)

# ── Colors ───────────────────────────────────────────────────────────────────

if [ -t 1 ] && [ "${TERM:-dumb}" != "dumb" ]; then
    GREEN=$'\033[0;32m' RED=$'\033[0;31m' CYAN=$'\033[0;36m'
    YELLOW=$'\033[0;33m' BOLD=$'\033[1m' DIM=$'\033[2m' NC=$'\033[0m'
else
    GREEN='' RED='' CYAN='' YELLOW='' BOLD='' DIM='' NC=''
fi

bold()    { printf "  ${BOLD}%s${NC}\n" "$*"; }
section() { printf "\n  ${BOLD}%s${NC}\n\n" "$*"; }
info()    { printf "  ${CYAN}>${NC} %s\n" "$*"; }
ok()      { printf "  ${GREEN}+${NC} %s\n" "$*"; }
warn()    { printf "  ${YELLOW}!${NC} %s\n" "$*"; }
err()     { printf "  ${RED}x${NC} %s\n" "$*" >&2; }
die()     { err "$*"; exit 1; }
fail()    { die "$*"; }   # legacy alias

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

# run "Doing thing" some-command --flags
# Spins while the command runs, swallows stdout, shows stderr (or last 20
# lines of stdout) on failure. Use only for non-interactive commands.
run() {
    local msg="$1"; shift
    local tmp_out tmp_err
    tmp_out="$(mktemp)" tmp_err="$(mktemp)"
    "$@" >"$tmp_out" 2>"$tmp_err" &
    local pid=$!
    if ! spin "$pid" "$msg"; then
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

# ── Banner ───────────────────────────────────────────────────────────────────

banner() {
    printf "\n${CYAN}${BOLD}"
    printf "      _         _               \n"
    printf "     / \\   _ __| |__   ___  ___ \n"
    printf "    / _ \\ | '__| '_ \\ / _ \\/ __|\n"
    printf "   / ___ \\| |  | |_) | (_) \\__ \\\\\n"
    printf "  /_/   \\_\\_|  |_.__/ \\___/|___/\n"
    printf "${NC}\n"
}

# ----------------------------------------------------------------------------
# 1a. tailscale binary
# ----------------------------------------------------------------------------
ensure_tailscale() {
  if command_exists tailscale; then
    ok "tailscale $(tailscale version 2>/dev/null | head -n1) already installed"
    return
  fi

  warn "tailscale not found — installing"
  case "$(uname -s)" in
    Darwin)
      command_exists brew || die "brew not found; install Homebrew (https://brew.sh) or grab Tailscale from the App Store"
      run "Installing tailscale (brew)" brew install tailscale
      run "Starting tailscaled" bash -c '
        sudo brew services start tailscale 2>/dev/null \
          || brew services start tailscale 2>/dev/null \
          || true
      '
      ;;
    Linux)
      run "Installing tailscale" bash -c 'curl -fsSL https://tailscale.com/install.sh | sh'
      ;;
    *)
      die "unsupported OS: $(uname -s); install tailscale manually from https://tailscale.com/download"
      ;;
  esac
  command_exists tailscale || die "tailscale install appears to have failed"
  ok "tailscale installed"
}

ensure_tailscale_up() {
  local state
  state="$(tailscale status --json 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin).get("BackendState",""))' 2>/dev/null || echo "")"
  case "$state" in
    Running)  return ;;
    NeedsLogin|NoState|Stopped|"")
      info "tailscale is not logged in; running 'tailscale up' (browser will open)..."
      tailscale up || warn "'tailscale up' did not exit cleanly; continuing on best-effort"
      ;;
  esac
}

# ----------------------------------------------------------------------------
# 1b. machine name
# ----------------------------------------------------------------------------
# Telegram bot usernames are capped at 32 chars and must end with "bot".
# We build the username as `arbos_<MACHINE>_bot`, so MACHINE must fit in
# 32 - len("arbos_") - len("_bot") = 22 chars. We also use the same name as
# the suffix on a doppler secret key (`ARBOS_<MACHINE_UPPER>_TELE_TOKEN`),
# which has no hard cap but should stay readable. 22 keeps both happy.
ARBOS_MACHINE_MAX_LEN=22

sanitise_machine() {
  local max="${2:-$ARBOS_MACHINE_MAX_LEN}"
  local cleaned
  cleaned="$(printf '%s' "$1" \
    | tr '[:upper:]' '[:lower:]' \
    | tr -- '-. \t' '____' \
    | tr -cd 'a-z0-9_' \
    | sed -E 's/_+/_/g; s/^_+//; s/_+$//')"
  # Hard length cap. If we have to truncate, append a short stable hash of
  # the original input so two long-but-distinct hostnames don't collide.
  if (( ${#cleaned} > max )); then
    local hash
    hash="$(printf '%s' "$1" | shasum 2>/dev/null | head -c 6 \
      || printf '%s' "$1" | sha1sum 2>/dev/null | head -c 6 \
      || printf '%s' "$1" | cksum | tr -d ' ' | head -c 6)"
    local keep=$(( max - 1 - ${#hash} ))
    (( keep < 1 )) && keep=1
    cleaned="${cleaned:0:keep}_${hash}"
    cleaned="$(printf '%s' "$cleaned" | sed -E 's/_+/_/g; s/^_+//; s/_+$//')"
  fi
  printf '%s' "$cleaned"
}

# Pull the user-assigned Tailnet machine name (the first DNS label, e.g.
# "templar" from "templar.tail9859e8.ts.net.") rather than the OS hostname,
# which on a lot of cloud images is a long random string.
_tailscale_dns_label() {
  tailscale status --self --json 2>/dev/null \
    | python3 -c '
import json, sys
try:
    d = json.load(sys.stdin)
except Exception:
    sys.exit(0)
dns = (d.get("Self") or {}).get("DNSName", "") or ""
print(dns.split(".", 1)[0] if dns else "")
' 2>/dev/null || true
}

_tailscale_hostname() {
  tailscale status --self --json 2>/dev/null \
    | python3 -c '
import json, sys
try:
    d = json.load(sys.stdin)
except Exception:
    sys.exit(0)
print((d.get("Self") or {}).get("HostName", "") or "")
' 2>/dev/null || true
}

resolve_machine() {
  # Allow an explicit override for users who want to pin the name regardless
  # of what tailscale / the OS report.
  local raw="${ARBOS_MACHINE_NAME:-}"
  local source="override"

  if [[ -z "$raw" ]] && command -v tailscale >/dev/null 2>&1; then
    raw="$(_tailscale_dns_label)"
    source="tailscale dns"
    if [[ -z "$raw" ]]; then
      raw="$(_tailscale_hostname)"
      source="tailscale hostname"
    fi
  fi
  if [[ -z "$raw" ]]; then
    raw="$(hostname -s 2>/dev/null || hostname || echo "unknown")"
    source="hostname"
  fi

  ARBOS_MACHINE="$(sanitise_machine "$raw")"
  [[ -n "$ARBOS_MACHINE" ]] || fail "could not resolve a usable machine name from '$raw'"
  ARBOS_MACHINE_UPPER="$(printf '%s' "$ARBOS_MACHINE" | tr '[:lower:]' '[:upper:]')"
  PM2_NAME="arbos-${ARBOS_MACHINE}"

  if [[ "$raw" != "$ARBOS_MACHINE" ]]; then
    ok "machine name: $ARBOS_MACHINE  ${DIM}(from $source: $raw)${NC}  (pm2: $PM2_NAME)"
  else
    ok "machine name: $ARBOS_MACHINE  ${DIM}(from $source)${NC}  (pm2: $PM2_NAME)"
  fi
}

# ----------------------------------------------------------------------------
# 2. doppler binary
# ----------------------------------------------------------------------------
ensure_doppler() {
  if command_exists doppler; then
    ok "doppler $(doppler --version 2>/dev/null | head -n1) already installed"
    return
  fi

  warn "doppler not found — installing"
  case "$(uname -s)" in
    Darwin)
      command_exists brew || die "brew not found; install Homebrew first (https://brew.sh) or install doppler manually"
      run "Installing doppler (brew)" brew install gnupg dopplerhq/cli/doppler
      ;;
    Linux)
      run "Installing doppler" bash -c \
        'curl -Ls --tlsv1.2 --proto "=https" --retry 3 https://cli.doppler.com/install.sh | sudo sh'
      ;;
    *)
      die "unsupported OS: $(uname -s); install doppler manually from https://docs.doppler.com/docs/install-cli"
      ;;
  esac
  command_exists doppler || die "doppler install appears to have failed"
  ok "doppler installed"
}

# ----------------------------------------------------------------------------
# 3. doppler auth + scope
# ----------------------------------------------------------------------------
ensure_doppler_auth() {
  if doppler me >/dev/null 2>&1; then
    ok "doppler authenticated as $(doppler me --json 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin).get("workplace",{}).get("name","?"))' 2>/dev/null || echo '?')"
    return
  fi
  warn "doppler not authenticated — opening login flow"
  doppler login
  doppler me >/dev/null 2>&1 || die "doppler login did not complete successfully"
  ok "doppler authenticated"
}

ensure_doppler_scope() {
  local current_project current_config
  current_project="$(doppler configure get project --plain 2>/dev/null || true)"
  current_config="$(doppler configure get config --plain 2>/dev/null || true)"

  if [[ -n "$current_project" && -n "$current_config" ]]; then
    ok "doppler scope: ${current_project} / ${current_config}"
    DOPPLER_PROJECT="$current_project"
    DOPPLER_CONFIG="$current_config"
    return
  fi

  run "Configuring doppler scope (${DOPPLER_PROJECT}/${DOPPLER_CONFIG})" \
    doppler setup --no-interactive \
      --project "$DOPPLER_PROJECT" \
      --config "$DOPPLER_CONFIG" \
      --scope "$INVOKE_DIR" \
    || die "doppler setup failed; try 'doppler setup' manually"
}

# ----------------------------------------------------------------------------
# 4. secrets
# ----------------------------------------------------------------------------
get_secret() {
  doppler secrets get "$1" --plain --no-exit-on-missing-secret 2>/dev/null || true
}

set_secret() {
  doppler secrets set "$1=$2" >/dev/null
}

prompt_secret() {
  local name="$1"
  local hidden="${2:-no}"
  local value=""
  if [[ "$hidden" == "yes" ]]; then
    read -r -s -p "  $name: " value
    echo
  else
    read -r -p "  $name: " value
  fi
  printf '%s' "$value"
}

ensure_required_secrets() {
  printf "  ${BOLD}Telegram secrets${NC} ${DIM}(${DOPPLER_PROJECT}/${DOPPLER_CONFIG})${NC}\n\n"
  local missing=0
  for k in "${REQUIRED_VARS[@]}"; do
    local v
    v="$(get_secret "$k")"
    if [[ -z "$v" ]]; then
      warn "$k = (missing)"
      missing=$((missing + 1))
    else
      local preview
      if [[ "$k" == "ARBOS_TELEGRAM_API_HASH" ]]; then
        preview="<hidden>"
      else
        preview="$v"
      fi
      ok "$k = $preview"
    fi
  done

  if [[ "$missing" -gt 0 ]]; then
    printf "\n  ${BOLD}Enter ${missing} missing secret(s)${NC} ${DIM}— will be saved to doppler${NC}\n\n"
    for k in "${REQUIRED_VARS[@]}"; do
      local v
      v="$(get_secret "$k")"
      if [[ -n "$v" ]]; then continue; fi

      local hidden="no"
      [[ "$k" == "ARBOS_TELEGRAM_API_HASH" ]] && hidden="yes"

      local entered=""
      while [[ -z "$entered" ]]; do
        entered="$(prompt_secret "$k" "$hidden")"
      done
      set_secret "$k" "$entered"
      ok "saved $k to doppler"
    done
  fi
}

report_machine_secrets() {
  local token_key="ARBOS_${ARBOS_MACHINE_UPPER}_TELE_TOKEN"
  local chat_id supergroup_id token
  chat_id="$(get_secret ARBOS_TELEGRAM_CHAT_ID)"
  supergroup_id="$(get_secret ARBOS_TELEGRAM_SUPERGROUP_ID)"
  token="$(get_secret "$token_key")"

  printf "\n  ${BOLD}Machine-scoped state${NC}\n\n"
  if [[ -n "$token" ]]; then
    ok "$token_key = <hidden>"
  else
    warn "$token_key = (missing) — will create or recover via BotFather"
  fi
  if [[ -n "$chat_id" ]]; then
    ok "ARBOS_TELEGRAM_CHAT_ID = $chat_id"
  else
    warn "ARBOS_TELEGRAM_CHAT_ID = (missing) — will create new supergroup"
  fi
  if [[ -n "$supergroup_id" ]]; then
    ok "ARBOS_TELEGRAM_SUPERGROUP_ID = $supergroup_id"
  fi

  EXPORT_BOT_TOKEN="$token"
  EXPORT_CHAT_ID="$chat_id"
  EXPORT_SUPERGROUP_ID="$supergroup_id"
  EXPORT_TOKEN_KEY="$token_key"
}

# ----------------------------------------------------------------------------
# 4b. system shared libraries needed by bundled tdjson (Linux only)
#     aiotdlib ships a tdjson built against LLVM libc++, which is not present
#     on a stock Debian/Ubuntu box. Without it, `import` is fine but the first
#     `Client(...)` call dies with:
#       OSError: libc++.so.1: cannot open shared object file
# ----------------------------------------------------------------------------
ensure_tdjson_runtime() {
  [[ "$(uname -s)" == "Linux" ]] || return 0

  # Quick probe via the dynamic linker -- if libc++.so.1 already resolves
  # somewhere on the loader path, we're done.
  if ldconfig -p 2>/dev/null | grep -q '\blibc++\.so\.1\b'; then
    ok "libc++ runtime already present"
    return 0
  fi

  warn "libc++ runtime not found — installing (needed by bundled tdjson)"
  if command_exists apt-get; then
    # `apt-get update` may fail on boxes with a broken third-party repo
    # (we've seen Doppler's apt source 404 in the wild). Don't let that
    # abort us -- libc++1 is almost always already in the apt cache.
    run "apt-get update (best effort)" bash -c 'sudo apt-get update || true'
    # libc++1 pulls in libc++abi1 / libunwind as needed on modern Ubuntu/Debian.
    if ! run "Installing libc++1 libc++abi1" sudo apt-get install -y libc++1 libc++abi1; then
      warn "install failed; retrying after a forced apt-get update"
      run "apt-get update (forced)" sudo apt-get update
      run "Installing libc++1 libc++abi1 (retry)" sudo apt-get install -y libc++1 libc++abi1
    fi
  elif command_exists dnf; then
    run "Installing libcxx" sudo dnf install -y libcxx libcxxabi
  elif command_exists yum; then
    run "Installing libcxx" sudo yum install -y libcxx libcxxabi
  elif command_exists pacman; then
    run "Installing libc++" sudo pacman -S --noconfirm libc++ libc++abi
  else
    die "no supported package manager (apt-get/dnf/yum/pacman) found; install libc++ manually"
  fi

  ldconfig -p 2>/dev/null | grep -q '\blibc++\.so\.1\b' \
    || die "libc++ install appears to have failed (libc++.so.1 still not on loader path)"
  ok "libc++ runtime installed"
}

# ----------------------------------------------------------------------------
# 5. python venv + package install
# ----------------------------------------------------------------------------
ensure_venv() {
  command_exists uv || die "uv not found; install with 'curl -LsSf https://astral.sh/uv/install.sh | sh'"
  if [[ ! -d "$REPO_ROOT/.venv" ]]; then
    run "Creating .venv (python 3.11)" uv venv --python 3.11 "$REPO_ROOT/.venv"
  fi
  # shellcheck disable=SC1091
  source "$REPO_ROOT/.venv/bin/activate"
  # Check for the console-script entry point, not `import arbos`. The latter
  # gives false positives when cwd contains an `arbos/` directory (the repo
  # itself, when run.sh is invoked from its parent), since Python 3 treats
  # any directory without __init__.py as a namespace package.
  if [[ ! -x "$REPO_ROOT/.venv/bin/arbos" ]]; then
    run "Installing arbos into .venv" uv pip install -e "$REPO_ROOT"
  fi
  [[ -x "$REPO_ROOT/.venv/bin/arbos" ]] \
    || die "arbos entry point still missing at $REPO_ROOT/.venv/bin/arbos after install"
  ok "venv ready ($(python --version 2>&1))"
}

# ----------------------------------------------------------------------------
# 6. installer
# ----------------------------------------------------------------------------
run_installer() {
  printf "\n  ${BOLD}Running arbos install${NC} ${DIM}(secrets via 'doppler run')${NC}\n\n"
  rm -f "$WRITEBACK_PATH" 2>/dev/null || true

  # arbos install is interactive (aiotdlib prompts for SMS code / 2FA via
  # sys.stdin.readline). When the parent shell was started by `curl | bash`,
  # stdin is the now-exhausted curl pipe, so readline() returns "" forever
  # and the auth flow infinite-loops. Force stdin to /dev/tty whenever one
  # exists; the sub-redirect in the subshell wins over whatever the parent
  # bash inherited.
  local stdin_src="/dev/stdin"
  [[ -r /dev/tty ]] && stdin_src="/dev/tty"

  (
    cd "$INVOKE_DIR"
    ARBOS_MACHINE="$ARBOS_MACHINE" \
    ARBOS_TELEGRAM_BOT_TOKEN="$EXPORT_BOT_TOKEN" \
    ARBOS_TELEGRAM_CHAT_ID="$EXPORT_CHAT_ID" \
    ARBOS_TELEGRAM_SUPERGROUP_ID="$EXPORT_SUPERGROUP_ID" \
    ARBOS_TELE_TOKEN_KEY="$EXPORT_TOKEN_KEY" \
    doppler run --silent --preserve-env -- "$REPO_ROOT/.venv/bin/arbos" install <"$stdin_src"
  )
}

writeback_doppler() {
  if [[ ! -f "$WRITEBACK_PATH" ]]; then
    return
  fi

  local pairs
  pairs="$(python3 -c '
import json, sys
data = json.load(open(sys.argv[1]))
for k, v in data.items():
    if v is None or v == "":
        continue
    print(f"{k}\t{v}")
' "$WRITEBACK_PATH")"

  if [[ -z "$pairs" ]]; then
    rm -f "$WRITEBACK_PATH"
    return
  fi

  printf "\n  ${BOLD}Pushing newly discovered values to doppler${NC}\n\n"
  while IFS=$'\t' read -r key value; do
    [[ -z "$key" ]] && continue
    set_secret "$key" "$value"
    if [[ "$key" == *TOKEN* || "$key" == *HASH* || "$key" == *2FA* ]]; then
      ok "$key = <hidden>"
    else
      ok "$key = $value"
    fi
  done <<< "$pairs"

  rm -f "$WRITEBACK_PATH"
}

# ----------------------------------------------------------------------------
# 6b. cursor-agent CLI + auth
# ----------------------------------------------------------------------------
ensure_path_local_bin() {
  # Make ~/.local/bin (where the cursor installer drops the binary) visible
  # to this script and to future interactive shells. Idempotent.
  export PATH="$HOME/.local/bin:$PATH"
  local line='export PATH="$HOME/.local/bin:$PATH"'
  local shellrc
  for shellrc in "$HOME/.zshrc" "$HOME/.bashrc"; do
    [[ -f "$shellrc" ]] || continue
    if ! grep -Fq "$line" "$shellrc" 2>/dev/null; then
      printf '\n# added by arbos run.sh: make cursor-agent visible\n%s\n' "$line" >> "$shellrc"
    fi
  done
}

ensure_cursor() {
  ensure_path_local_bin
  if command_exists cursor-agent; then
    ok "cursor-agent $(cursor-agent --version 2>/dev/null | head -n1) already installed"
    return
  fi
  warn "cursor-agent not found — installing"
  run "Installing cursor-agent" bash -c 'curl -fsS https://cursor.com/install | bash'
  ensure_path_local_bin
  command_exists cursor-agent \
    || die "cursor-agent install appears to have failed (try: source ~/.zshrc)"
  ok "cursor-agent $(cursor-agent --version 2>/dev/null | head -n1) installed"
}

# Resolve cursor auth in priority order. Populates EXPORT_CURSOR_API_KEY for
# the PM2 start block to forward; stays empty when relying on a local
# `cursor-agent login` session under ~/.cursor (PM2 inherits that for free
# since it runs as the same user).
ensure_cursor_auth() {
  EXPORT_CURSOR_API_KEY=""

  local key
  key="$(get_secret CURSOR_API_KEY)"

  # Path A: doppler already has a key.
  if [[ -n "$key" ]]; then
    ok "CURSOR_API_KEY found in doppler"
    EXPORT_CURSOR_API_KEY="$key"
    if ! CURSOR_API_KEY="$key" cursor-agent status >/dev/null 2>&1; then
      warn "CURSOR_API_KEY in doppler did not validate via 'cursor-agent status'; continuing anyway"
    fi
    return
  fi

  # Path B: this machine has a local cursor session.
  if cursor-agent status 2>/dev/null | grep -q "Logged in"; then
    local who
    who="$(cursor-agent status 2>/dev/null | grep "Logged in" | head -n1 | tr -d '\r')"
    ok "cursor-agent ${who#  }"
    info "no CURSOR_API_KEY in doppler; using local cursor session for this machine"
    return
  fi

  # Path C: nothing yet. Drive a one-shot login flow.
  printf "\n  ${BOLD}cursor-agent is not authenticated${NC}\n"
  printf "    [1] open browser and run 'cursor-agent login' on this machine ${DIM}(default)${NC}\n"
  printf "    [2] paste an existing CURSOR_API_KEY ${DIM}(saved to doppler so all machines reuse it)${NC}\n"
  local choice=""
  read -r -p "    choice [1/2]: " choice
  case "${choice:-1}" in
    2)
      local entered=""
      while [[ -z "$entered" ]]; do
        entered="$(prompt_secret CURSOR_API_KEY yes)"
      done
      if ! CURSOR_API_KEY="$entered" cursor-agent status >/dev/null 2>&1; then
        die "the supplied CURSOR_API_KEY did not validate via 'cursor-agent status'"
      fi
      set_secret CURSOR_API_KEY "$entered"
      ok "saved CURSOR_API_KEY to doppler"
      EXPORT_CURSOR_API_KEY="$entered"
      ;;
    *)
      warn "running 'cursor-agent login' — browser will open"
      cursor-agent login || die "cursor-agent login failed"
      cursor-agent status 2>/dev/null | grep -q "Logged in" \
        || die "cursor-agent login did not complete"
      ok "cursor-agent logged in on this machine"
      ;;
  esac
}

# ----------------------------------------------------------------------------
# 7. Node.js + PM2
# ----------------------------------------------------------------------------
ensure_node() {
  if command_exists node; then
    ok "node $(node --version) already installed"
    return
  fi
  warn "node not found — installing"
  case "$(uname -s)" in
    Darwin)
      command_exists brew || die "brew required to install node on macOS"
      run "Installing node (brew)" brew install node
      ;;
    Linux)
      if command_exists apt-get; then
        run "Adding nodesource repo" bash -c \
          'curl -fsSL https://deb.nodesource.com/setup_lts.x | sudo -E bash -'
        run "Installing nodejs (apt)" sudo apt-get install -y nodejs
      elif command_exists dnf; then
        run "Adding nodesource repo" bash -c \
          'curl -fsSL https://rpm.nodesource.com/setup_lts.x | sudo bash -'
        run "Installing nodejs (dnf)" sudo dnf install -y nodejs
      else
        die "no supported package manager (apt-get/dnf) found; install node manually"
      fi
      ;;
    *)
      die "unsupported OS: $(uname -s); install node manually"
      ;;
  esac
  command_exists node || die "node install appears to have failed"
  ok "node $(node --version) installed"
}

ensure_pm2() {
  if command_exists pm2; then
    ok "pm2 $(pm2 --version 2>/dev/null | head -n1) already installed"
    return
  fi
  warn "pm2 not found — installing globally"
  if ! run "Installing pm2 (npm)" npm install -g pm2; then
    run "Installing pm2 (sudo npm)" sudo npm install -g pm2
  fi
  command_exists pm2 || die "pm2 install appears to have failed"
  ok "pm2 $(pm2 --version) installed"
}

# ----------------------------------------------------------------------------
# 8. agent under PM2
# ----------------------------------------------------------------------------
start_agent() {
  local pybin="$REPO_ROOT/.venv/bin/arbos"
  [[ -x "$pybin" ]] || fail "arbos binary not found at $pybin (was the venv set up?)"

  mkdir -p "$STORAGE_DIR"

  # Forward CURSOR_API_KEY when we resolved one (Path A or Path C-paste).
  # When EXPORT_CURSOR_API_KEY is empty (Path B: local cursor-agent login),
  # PM2 inherits the on-disk session at ~/.cursor automatically since it
  # runs as the same user.
  if [[ -n "${EXPORT_CURSOR_API_KEY:-}" ]]; then
    export CURSOR_API_KEY="$EXPORT_CURSOR_API_KEY"
  fi

  # Forward OPENAI_API_KEY when present in doppler. Optional: voice-note
  # transcription in agent.py is disabled when this is empty, but the agent
  # still runs. Mirrors the CURSOR_API_KEY plumbing above.
  EXPORT_OPENAI_API_KEY="$(get_secret OPENAI_API_KEY)"
  if [[ -n "$EXPORT_OPENAI_API_KEY" ]]; then
    export OPENAI_API_KEY="$EXPORT_OPENAI_API_KEY"
    ok "OPENAI_API_KEY forwarded to pm2 (voice transcription enabled)"
  else
    info "no OPENAI_API_KEY in doppler — voice transcription disabled"
  fi

  # If an entry with this name already exists, only reuse it when its
  # script path + cwd match what we'd start now. An older project may
  # have grabbed the same pm2 name, in which case `pm2 reload` would
  # silently re-launch the wrong binary.
  local existing_path="" existing_cwd=""
  if pm2 describe "$PM2_NAME" >/dev/null 2>&1; then
    existing_path="$(pm2 jlist 2>/dev/null \
      | python3 -c "
import json, sys
for p in json.load(sys.stdin):
    if p.get('name') == '$PM2_NAME':
        print(p.get('pm2_env', {}).get('pm_exec_path', ''))
        break
" 2>/dev/null || true)"
    existing_cwd="$(pm2 jlist 2>/dev/null \
      | python3 -c "
import json, sys
for p in json.load(sys.stdin):
    if p.get('name') == '$PM2_NAME':
        print(p.get('pm2_env', {}).get('pm_cwd', ''))
        break
" 2>/dev/null || true)"
  fi

  if [[ -n "$existing_path" ]] \
     && { [[ "$existing_path" != "$pybin" ]] || [[ "$existing_cwd" != "$INVOKE_DIR" ]]; }; then
    warn "stale pm2 entry $PM2_NAME points at ${DIM}${existing_path}${NC} (cwd ${DIM}${existing_cwd}${NC})"
    info "deleting it so we can start the right binary"
    run "Deleting stale $PM2_NAME entry" pm2 delete "$PM2_NAME"
    existing_path=""
  fi

  if [[ -n "$existing_path" ]]; then
    run "Reloading $PM2_NAME (refreshed env)" \
      env ARBOS_MACHINE="$ARBOS_MACHINE" \
        ${EXPORT_CURSOR_API_KEY:+CURSOR_API_KEY="$EXPORT_CURSOR_API_KEY"} \
        pm2 reload "$PM2_NAME" --update-env
  else
    run "Starting $PM2_NAME under pm2" \
      env ARBOS_MACHINE="$ARBOS_MACHINE" \
        ${EXPORT_CURSOR_API_KEY:+CURSOR_API_KEY="$EXPORT_CURSOR_API_KEY"} \
        pm2 start "$pybin" \
          --name "$PM2_NAME" \
          --cwd "$INVOKE_DIR" \
          --log "$AGENT_LOG_PATH" \
          --time \
          --max-memory-restart 256M \
          --interpreter none \
          -- agent run
  fi

  pm2 save >/dev/null 2>&1 || true
  ok "agent online (pm2: $PM2_NAME, log: $AGENT_LOG_PATH)"
}

ensure_pm2_startup() {
  # Detect platform startup unit. macOS: launchctl list | grep pm2.
  # Linux: systemctl is-enabled pm2-$USER.
  local installed=0
  case "$(uname -s)" in
    Darwin)
      if launchctl list 2>/dev/null | grep -q "pm2\.${USER}"; then installed=1; fi
      ;;
    Linux)
      if command -v systemctl >/dev/null 2>&1 && \
         systemctl is-enabled "pm2-${USER}" >/dev/null 2>&1; then
        installed=1
      fi
      ;;
  esac

  if [[ "$installed" -eq 1 ]]; then
    ok "pm2 startup unit installed — agent will survive reboots"
    return
  fi

  warn "pm2 startup unit not installed — agent will NOT survive reboots"
  info "run this ONCE to enable boot persistence:"
  echo

  # Capture pm2's suggested sudo line. `pm2 startup` exits non-zero by design
  # (it's just printing instructions), and may not contain a sudo line at all
  # on every platform -- both cases must not abort the script.
  local pm2_out hint=""
  pm2_out="$(pm2 startup 2>&1 || true)"
  while IFS= read -r line; do
    if [[ "$line" == *"sudo "* ]]; then
      hint="$line"
      break
    fi
  done <<< "$pm2_out"

  if [[ -n "$hint" ]]; then
    bold "    $hint"
  else
    bold "    pm2 startup       # then copy/paste the printed sudo line"
  fi
  echo
  info "after running it, re-run: ./run.sh   (so we 'pm2 save' the snapshot)"
}

# ----------------------------------------------------------------------------
# subcommand bodies
# ----------------------------------------------------------------------------
cmd_install() {
  banner

  section "Network identity"
  ensure_tailscale
  ensure_tailscale_up
  resolve_machine

  section "Doppler"
  ensure_doppler
  ensure_doppler_auth
  ensure_doppler_scope

  section "Secrets"
  ensure_required_secrets
  report_machine_secrets

  section "System libraries"
  ensure_tdjson_runtime

  section "Python venv"
  ensure_venv

  section "Arbos installer"
  set +e
  run_installer
  local rc=$?
  set -e
  writeback_doppler

  section "Cursor agent"
  ensure_cursor
  ensure_cursor_auth
  return "$rc"
}

cmd_privacy_off() {
  # Idempotent: drive BotFather /setprivacy -> Disable so the bot can read
  # all messages in groups/topics. Required for the pong agent to actually
  # see plain text messages (the default privacy mode hides them).
  section "Bot privacy mode"
  info "ensuring BotFather /setprivacy -> Disable"
  local stdin_src="/dev/stdin"
  [[ -r /dev/tty ]] && stdin_src="/dev/tty"
  (
    cd "$INVOKE_DIR"
    ARBOS_MACHINE="$ARBOS_MACHINE" \
    ARBOS_TELEGRAM_BOT_TOKEN="$EXPORT_BOT_TOKEN" \
    ARBOS_TELEGRAM_CHAT_ID="$EXPORT_CHAT_ID" \
    ARBOS_TELEGRAM_SUPERGROUP_ID="$EXPORT_SUPERGROUP_ID" \
    ARBOS_TELE_TOKEN_KEY="$EXPORT_TOKEN_KEY" \
    doppler run --silent --preserve-env -- "$REPO_ROOT/.venv/bin/arbos" privacy-off <"$stdin_src"
  ) || warn "privacy-off step failed; bot may only see mentions/commands"
}

cmd_install_and_start() {
  cmd_install
  cmd_privacy_off

  section "Node + PM2"
  ensure_node
  ensure_pm2

  section "Launching agent"
  start_agent
  ensure_pm2_startup

  printf "\n  ${GREEN}${BOLD}Arbos is online${NC}    ${DIM}machine=${ARBOS_MACHINE}  bot=@arbos_${ARBOS_MACHINE}_bot${NC}\n"
  printf "\n  ${BOLD}Manage${NC}\n"
  printf "    ./run.sh logs          ${DIM}— tail bot output${NC}\n"
  printf "    ./run.sh status        ${DIM}— pm2 status${NC}\n"
  printf "    ./run.sh restart       ${DIM}— reload bot${NC}\n"
  printf "    ./run.sh stop          ${DIM}— stop bot${NC}\n\n"
}

cmd_stop() {
  resolve_machine
  command_exists pm2 || die "pm2 not installed; nothing to stop"
  if pm2 describe "$PM2_NAME" >/dev/null 2>&1; then
    run "Stopping $PM2_NAME" pm2 stop "$PM2_NAME"
    pm2 save >/dev/null 2>&1 || true
  else
    warn "no pm2 entry $PM2_NAME"
  fi
}

cmd_restart() {
  resolve_machine
  command_exists pm2 || die "pm2 not installed"
  if pm2 describe "$PM2_NAME" >/dev/null 2>&1; then
    run "Reloading $PM2_NAME" pm2 reload "$PM2_NAME" --update-env
    pm2 save >/dev/null 2>&1 || true
  else
    warn "no pm2 entry $PM2_NAME — run ./run.sh to start fresh"
    exit 1
  fi
}

cmd_logs() {
  resolve_machine
  command_exists pm2 || die "pm2 not installed"
  exec pm2 logs "$PM2_NAME" --lines 200
}

cmd_status() {
  resolve_machine
  command_exists pm2 || { warn "pm2 not installed"; exit 1; }
  if pm2 describe "$PM2_NAME" >/dev/null 2>&1; then
    pm2 status "$PM2_NAME"
  else
    warn "no pm2 entry $PM2_NAME"
    pm2 status
  fi
}

# ----------------------------------------------------------------------------
# dispatcher
# ----------------------------------------------------------------------------
sub="${1:-default}"
case "$sub" in
  install)               shift || true; cmd_install ;;
  default|start|up)      shift || true; cmd_install_and_start ;;
  stop|down)             shift || true; cmd_stop ;;
  restart|reload)        shift || true; cmd_restart ;;
  logs|tail)             shift || true; cmd_logs ;;
  status|ps)             shift || true; cmd_status ;;
  -h|--help|help)
    sed -n '1,20p' "$0" | sed 's/^# \{0,1\}//'
    ;;
  *)
    fail "unknown subcommand: $sub (try install | start | stop | restart | logs | status)" ;;
esac
