#!/bin/sh
# Arbos connect: Claude Code / Codex on your own computer, in your Arbos chats.
#   curl -fsSL https://relay.arbos.life/acp/install.sh | sh -s -- --cwd ~/your-project
#
# One command.  It finds or builds `arbos-acp`, asks for the agent key with the terminal echo
# off, and starts the bridge in the project you name.  Nothing is written to
# ~/.claude/settings.json, nothing is installed globally from npm, and the key never appears in
# argv, in your shell history or in `ps`.
#
# Options (all optional):
#   --cwd DIR     the project the agent works in (default: the current directory)
#   --relay URL   relay to talk to (default https://relay.arbos.life)
#   --key-stdin   read the key from stdin instead of the terminal (for scripts)
#   --dry-run     print what would run, and exit without starting anything
#   --            everything after this goes to arbos-acp, e.g.  -- --permissions reject
set -eu

RELAY="https://relay.arbos.life"
REPO="https://github.com/unarbos/arbos"
CWD="$PWD"
KEY_STDIN=0
DRY_RUN=0

say() { printf '%s\n' "$*" >&2; }

while [ $# -gt 0 ]; do
  case "$1" in
    --cwd) CWD="$2"; shift 2 ;;
    --relay) RELAY="$2"; shift 2 ;;
    --key-stdin) KEY_STDIN=1; shift ;;
    --dry-run) DRY_RUN=1; shift ;;
    --) shift; break ;;
    -h|--help) sed -n '2,15p' "$0" 2>/dev/null || say "See $RELAY/acp/install.sh"; exit 0 ;;
    --key|--key=*)
      say "Arbos: give the key when asked, not on the command line. argv is readable by every"
      say "process on this machine through ps, and your shell will remember it."
      exit 64 ;;
    *) say "Arbos: unknown option $1  (options for the agent go after --)"; exit 64 ;;
  esac
done

[ -d "$CWD" ] || {
  say "Arbos: --cwd $CWD does not exist. Point it at the project you want worked on."
  exit 1
}
CWD=$(cd "$CWD" && pwd)

os="$(uname -s)"
arch="$(uname -m)"
case "$os" in
  Darwin) target_os="macos" ;;
  Linux) target_os="linux" ;;
  *) say "Arbos: $os is not supported yet (macOS and Linux are)."; exit 1 ;;
esac
case "$arch" in
  arm64|aarch64) target_arch="arm64" ;;
  x86_64|amd64) target_arch="x86_64" ;;
  *) say "Arbos: unsupported CPU $arch."; exit 1 ;;
esac

BIN_DIR="$HOME/.local/bin"
mkdir -p "$BIN_DIR"
bin=""

# 1. A binary you already have wins, so running this again does not rebuild anything.
for cand in "$(command -v arbos-acp 2>/dev/null || true)" "$HOME/.cargo/bin/arbos-acp" "$BIN_DIR/arbos-acp"; do
  if [ -n "$cand" ] && [ -x "$cand" ]; then bin="$cand"; break; fi
done

# 2. A prebuilt binary from the newest release, when one exists for this machine.
if [ -z "$bin" ]; then
  asset="arbos-acp-${target_os}-${target_arch}"
  if curl -fsSL -o "$BIN_DIR/arbos-acp.tmp" "$REPO/releases/latest/download/$asset" 2>/dev/null; then
    chmod 755 "$BIN_DIR/arbos-acp.tmp"
    mv "$BIN_DIR/arbos-acp.tmp" "$BIN_DIR/arbos-acp"
    bin="$BIN_DIR/arbos-acp"
    say "Arbos: downloaded $asset to $bin"
  else
    rm -f "$BIN_DIR/arbos-acp.tmp"
  fi
fi

# 3. Build from source. The Rust toolchain is never installed for you.
if [ -z "$bin" ]; then
  if command -v cargo >/dev/null 2>&1; then
    say "Arbos: building arbos-acp from source with cargo. A few minutes, once."
    cargo install --quiet --git "$REPO" arbos-acp
    bin="$HOME/.cargo/bin/arbos-acp"
  else
    say "Arbos: there is no prebuilt arbos-acp for $target_os/$target_arch and no cargo here."
    say "Install Rust from https://rustup.rs and run this again."
    exit 1
  fi
fi

command -v node >/dev/null 2>&1 || say \
  "Arbos: node was not found. arbos-acp starts the Claude adapter with npx and will need it."

case ":$PATH:" in
  *":$(dirname "$bin"):"*) ;;
  *) say "Arbos: add $(dirname "$bin") to your PATH to call arbos-acp directly later." ;;
esac

# The key: already in the environment, piped in, or typed at the terminal with echo off. It is
# passed to the bridge in the environment and never as an argument, because arguments are public.
if [ -n "${ARBOS_AGENT_KEY:-}" ]; then
  say "Arbos: using the key in ARBOS_AGENT_KEY."
elif [ "$KEY_STDIN" = 1 ]; then
  IFS= read -r ARBOS_AGENT_KEY || true
elif [ -r /dev/tty ]; then
  printf 'Arbos agent key (arbk_…, it will not be shown): ' >&2
  saved=$(stty -g < /dev/tty 2>/dev/null || true)
  stty -echo < /dev/tty 2>/dev/null || true
  IFS= read -r ARBOS_AGENT_KEY < /dev/tty || true
  if [ -n "$saved" ]; then
    stty "$saved" < /dev/tty 2>/dev/null || true
  else
    stty echo < /dev/tty 2>/dev/null || true
  fi
  printf '\n' >&2
else
  say "Arbos: no terminal here to ask for the key. Set ARBOS_AGENT_KEY, or use --key-stdin."
  exit 1
fi
export ARBOS_AGENT_KEY
case "${ARBOS_AGENT_KEY:-}" in
  arbk_*) ;;
  "") say "Arbos: no key given."; exit 1 ;;
  *) say "Arbos: that does not look like an agent key. They start with arbk_."; exit 1 ;;
esac

say ""
say "Arbos: starting the bridge."
say "  bridge:   $bin"
say "  relay:    $RELAY"
say "  project:  $CWD"
say "  asks you: every tool call raises an approval card in your own chat with the agent,"
say "            and anything other than you approving it is a refusal."
say "  may act:  you, and nobody else. Widen it yourself with --allow."
say ""

if [ "$DRY_RUN" = 1 ]; then
  say "Would run:  $bin --relay $RELAY --cwd $CWD $*"
  say "The key stays in the environment and is not printed."
  exit 0
fi

exec "$bin" --relay "$RELAY" --cwd "$CWD" "$@"
