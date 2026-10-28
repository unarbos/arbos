#!/bin/sh
# Arbos sidecar installer.  curl -fsSL https://relay.arbos.life/sidecar/install.sh | sh -s -- [options]
#
# Installs the `arbos-sidecar` binary, asks for the agent key (echo off, never printed), writes the
# key file with mode 600 and config.toml, registers the waker, installs a user service (systemd
# --user or launchd) and starts it.  Idempotent: run it again to change the waker.
#
# Options (all optional):
#   --relay URL                    relay to talk to (default https://relay.arbos.life)
#   --waker webhook|command|file|none
#   --webhook-url-file PATH        file holding the webhook URL (mode 600)
#   --webhook-auth-file PATH       file holding the Authorization value (mode 600)
#   --command 'claude -p {prompt}' command waker; {prompt} {json} {chat} are filled in
#   --key-stdin                    read the key from stdin instead of the terminal (for scripts)
#   --no-service                   skip the user service (run `arbos-sidecar run` yourself)
#   --no-test                      skip the test wake
set -eu

RELAY="https://relay.arbos.life"
REPO="https://github.com/unarbos/arbos"
INIT_ARGS=""
SERVICE=1
TEST=1
KEY_STDIN=0

while [ $# -gt 0 ]; do
  case "$1" in
    --relay) RELAY="$2"; shift 2 ;;
    --waker) INIT_ARGS="$INIT_ARGS --waker $2"; shift 2 ;;
    --webhook-url-file) INIT_ARGS="$INIT_ARGS --webhook-url-file $2"; shift 2 ;;
    --webhook-auth-file) INIT_ARGS="$INIT_ARGS --webhook-auth-file $2"; shift 2 ;;
    --command) CMD="$2"; shift 2 ;;
    --key-stdin) KEY_STDIN=1; shift ;;
    --no-service) SERVICE=0; shift ;;
    --no-test) TEST=0; shift ;;
    -h|--help) sed -n '2,18p' "$0" 2>/dev/null || true; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 64 ;;
  esac
done

say() { printf '%s\n' "$*" >&2; }

os="$(uname -s)"
arch="$(uname -m)"
case "$os" in
  Darwin) target_os="macos" ;;
  Linux) target_os="linux" ;;
  *) say "Arbos sidecar: $os is not supported yet (macOS and Linux are)."; exit 1 ;;
esac
case "$arch" in
  arm64|aarch64) target_arch="arm64" ;;
  x86_64|amd64) target_arch="x86_64" ;;
  *) say "Arbos sidecar: unsupported CPU $arch."; exit 1 ;;
esac

BIN_DIR="${ARBOS_SIDECAR_BIN_DIR:-$HOME/.local/bin}"
mkdir -p "$BIN_DIR"
bin=""

# 1. An installed binary wins.
for cand in "$(command -v arbos-sidecar 2>/dev/null || true)" "$HOME/.cargo/bin/arbos-sidecar" "$BIN_DIR/arbos-sidecar"; do
  if [ -n "$cand" ] && [ -x "$cand" ]; then bin="$cand"; break; fi
done

# 2. A prebuilt binary from the newest release, when one exists.
if [ -z "$bin" ]; then
  asset="arbos-sidecar-${target_os}-${target_arch}"
  url="$REPO/releases/latest/download/$asset"
  if curl -fsSL -o "$BIN_DIR/arbos-sidecar.tmp" "$url" 2>/dev/null; then
    chmod 755 "$BIN_DIR/arbos-sidecar.tmp"
    mv "$BIN_DIR/arbos-sidecar.tmp" "$BIN_DIR/arbos-sidecar"
    bin="$BIN_DIR/arbos-sidecar"
    say "Arbos sidecar: downloaded $asset to $bin"
  else
    rm -f "$BIN_DIR/arbos-sidecar.tmp"
  fi
fi

# 3. Build from source with cargo (the toolchain is never installed unasked).
if [ -z "$bin" ]; then
  if command -v cargo >/dev/null 2>&1; then
    say "Arbos sidecar: building from source with cargo (a few minutes)…"
    cargo install --quiet --git "$REPO" arbos-sidecar
    bin="$HOME/.cargo/bin/arbos-sidecar"
  else
    say "Arbos sidecar: no prebuilt binary for $target_os/$target_arch and no cargo on this machine."
    say "Install Rust first (https://rustup.rs: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh), then run this installer again."
    exit 1
  fi
fi

case ":$PATH:" in
  *":$(dirname "$bin"):"*) ;;
  *) say "Note: add $(dirname "$bin") to your PATH to call arbos-sidecar directly." ;;
esac

# The key: typed at the terminal with echo off (the binary opens /dev/tty), or piped with --key-stdin.
if [ "$KEY_STDIN" = 1 ]; then
  INIT_ARGS="$INIT_ARGS --key-stdin"
fi
if [ -n "${CMD:-}" ]; then
  "$bin" init --relay "$RELAY" $INIT_ARGS --command "$CMD"
else
  "$bin" init --relay "$RELAY" $INIT_ARGS
fi

if [ "$TEST" = 1 ]; then
  "$bin" test-wake || say "Arbos sidecar: the test wake did not go through; fix the waker in ~/.config/arbos-sidecar/config.toml and run 'arbos-sidecar test-wake'."
fi

if [ "$SERVICE" = 1 ]; then
  "$bin" install-service
else
  say "Arbos sidecar: start it with '$bin run' (no service installed)."
fi

"$bin" status
say ""
say "Register the MCP server in your agent:  command: $bin   args: [\"mcp\"]"
say "  Claude Code:  claude mcp add arbos -- $bin mcp"
say "  Codex:        codex mcp add arbos -- $bin mcp   (then add default_tools_approval_mode = \"approve\" under [mcp_servers.arbos] in ~/.codex/config.toml)"
say "  Cursor:       .cursor/mcp.json → {\"mcpServers\":{\"arbos\":{\"command\":\"$bin\",\"args\":[\"mcp\"]}}}"
