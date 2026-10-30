#!/usr/bin/env bash
# Install (or upgrade) the Arbos feedback loop on the ArbosLife server (Ubuntu, systemd).
#
#   sudo -E scripts/feedback/install-feedback-loop.sh                 # install/upgrade, keep the cursor
#   sudo -E scripts/feedback/install-feedback-loop.sh --since 2026-10-15T19:50:00Z   # (re)start from a date
#
# What it does
#   /opt/arbos-feedback/            feedback_poller.py + agent-prompt.md (copied from this directory)
#   /etc/arbos/feedback.env         secrets, root:root 0600 — App Store Connect key, Cursor API key,
#                                   GitHub token. Pulled from the 1Password vault "Arbos" with `op` when
#                                   OP_SERVICE_ACCOUNT_TOKEN is in the environment (or in /etc/arbos/op.env),
#                                   else taken from the caller's environment, else kept from the old file.
#   /etc/arbos/feedback-priority.txt   emails / @handles that go first and skip the per-tester cap (Jacob)
#   /etc/arbos/feedback-blocklist.txt  emails / @handles whose feedback is dropped (abuse)
#   /var/lib/arbos-feedback/        state: cursor.json, launches.json, log.jsonl, <id>/ per item, inbox/
#   arbos-feedback-poller.{service,timer}   one tick every 2 minutes as user arbos-feedback
#   /etc/arbos/feedback.disabled    kill file: create it to stop the loop, delete it to resume
#
# Environment knobs (written into feedback.env; defaults in brackets):
#   ASC_KEY_ID [ZV82D3ZWRT]  ASC_ISSUER_ID [4623d589-1653-444a-b497-93116044e272]  ASC_APP_ID [6812503407]
#   ASC_KEY_P8_B64 (base64 of the .p8; vault: item "Appe Cert Apple Developer", file AuthKey_<id>.p8)
#   CURSOR_API_KEY (vault: "New New Cursor")   GITHUB_TOKEN (vault: "Arbos GitHub PAT (workflow)")
#   ARBOS_RELAY_URL [https://relay.arbos.life] + ARBOS_RELAY_ADMIN_TOKEN (vault: relay admin token) — Mac sheet intake
#   OPENROUTER_API_KEY (vault: OpenRouter) + FEEDBACK_SCREEN_MODEL [openai/gpt-4o-mini] — malicious-intent screen
#   GITHUB_REPO [unarbos/arbos]  FEEDBACK_PRIORITY [jake@bittensor.com,@unconst]  FEEDBACK_AGENT_MODEL []
#   FEEDBACK_MIN_INTERVAL_S [90]  FEEDBACK_MAX_PER_DAY [120]  FEEDBACK_TESTER_MAX_PER_DAY [20]  FEEDBACK_DEDUPE_DAYS [7]
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
INSTALL_DIR=/opt/arbos-feedback
ENV_FILE=/etc/arbos/feedback.env
PRIORITY_FILE=/etc/arbos/feedback-priority.txt
BLOCK_FILE=/etc/arbos/feedback-blocklist.txt
STATE_DIR=/var/lib/arbos-feedback
SVC_USER=arbos-feedback
SINCE=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --since) SINCE="$2"; shift 2;;
    *) echo "unknown arg $1" >&2; exit 2;;
  esac
done
[[ $EUID -eq 0 ]] || { echo "run as root (sudo -E)" >&2; exit 1; }

# Only committed, pushed code is installed (2026-10-23: an operator copy carried never-committed code once
# and lagged the committed head once). The tree under scripts/feedback must be clean and HEAD must exist on
# the remote. ALLOW_DIRTY=1 overrides for a bench. On a host without a checkout use install-poller.sh.
if [[ "${ALLOW_DIRTY:-0}" != 1 ]] && git -C "$HERE" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  dirty=$(git -C "$HERE" status --porcelain -- . | head -n 5)
  [[ -z "$dirty" ]] || { printf 'scripts/feedback has uncommitted changes; commit and push first (ALLOW_DIRTY=1 to override):\n%s\n' "$dirty" >&2; exit 1; }
  head=$(git -C "$HERE" rev-parse HEAD)
  git -C "$HERE" fetch -q origin 2>/dev/null || true
  if ! git -C "$HERE" branch -r --contains "$head" 2>/dev/null | grep -q .; then
    echo "HEAD ${head:0:9} is not on any remote branch; push first (ALLOW_DIRTY=1 to override)" >&2; exit 1
  fi
fi

say() { printf '\n==> %s\n' "$*"; }

say "python"
python3 -c 'import cryptography' 2>/dev/null || { apt-get update -q && apt-get install -y -q python3-cryptography >/dev/null; }

say "files"
install -d -m 0755 "$INSTALL_DIR"
install -m 0755 "$HERE/feedback_poller.py" "$INSTALL_DIR/feedback_poller.py"
install -m 0644 "$HERE/agent-prompt.md" "$INSTALL_DIR/agent-prompt.md"
install -m 0644 "$HERE/arbos-feedback-poller.service" /etc/systemd/system/arbos-feedback-poller.service
install -m 0644 "$HERE/arbos-feedback-poller.timer" /etc/systemd/system/arbos-feedback-poller.timer

say "user + state dir"
id "$SVC_USER" >/dev/null 2>&1 || useradd --system --home "$STATE_DIR" --shell /usr/sbin/nologin "$SVC_USER"
install -d -m 0750 -o "$SVC_USER" -g "$SVC_USER" "$STATE_DIR" "$STATE_DIR/inbox"
mkdir -p /etc/arbos

say "secrets -> $ENV_FILE"
if [[ -z "${OP_SERVICE_ACCOUNT_TOKEN:-}" && -f /etc/arbos/op.env ]]; then
  set -a; . /etc/arbos/op.env; set +a
fi
export PATH=/usr/local/bin:$PATH
opread() { op read "$1" 2>/dev/null || true; }
: "${ASC_KEY_ID:=ZV82D3ZWRT}"
: "${ASC_ISSUER_ID:=4623d589-1653-444a-b497-93116044e272}"
: "${ASC_APP_ID:=6812503407}"
: "${GITHUB_REPO:=unarbos/arbos}"
: "${ARBOS_RELAY_URL:=https://relay.arbos.life}"
if command -v op >/dev/null 2>&1 && [[ -n "${OP_SERVICE_ACCOUNT_TOKEN:-}" ]]; then
  if [[ -z "${ASC_KEY_P8_B64:-}" ]]; then
    p8=$(opread "op://Arbos/phsrnmu3qfpqx6lbumafjo3uom/AuthKey_${ASC_KEY_ID}.p8")
    [[ -n "$p8" ]] && ASC_KEY_P8_B64=$(printf '%s\n' "$p8" | base64 -w0)
  fi
  : "${CURSOR_API_KEY:=$(opread 'op://Arbos/eagoyh7fpynfmzx4xm2agtqaim/notesPlain' | tr -d '[:space:]')}"
  : "${GITHUB_TOKEN:=$(opread 'op://Arbos/gefompyaklwbv2iuyyb4z5rvbm/credential' | tr -d '[:space:]')}"
  : "${ARBOS_RELAY_ADMIN_TOKEN:=$(opread 'op://Arbos/pjwsl7tjrhcbzbqos2masde2u4/credential' | tr -d '[:space:]')}"
  : "${OPENROUTER_API_KEY:=$(opread 'op://Arbos/xbirrctuljw2m6aieoway2szom/credential' | tr -d '[:space:]')}"
  # iOS pre-merge gate: the EC2 Mac's ssh key (vault document "SSH — EC2 Mac arbos-mobile … ec2-user private key").
  if [[ ! -s /etc/arbos/ec2-mac.pem ]]; then
    if op document get l6zmhcj7thvr6bi7xdfr36akc4 --vault Arbos > /etc/arbos/ec2-mac.pem.tmp 2>/dev/null && [[ -s /etc/arbos/ec2-mac.pem.tmp ]]; then
      mv /etc/arbos/ec2-mac.pem.tmp /etc/arbos/ec2-mac.pem
    else
      rm -f /etc/arbos/ec2-mac.pem.tmp
    fi
  fi
fi
if [[ -s /etc/arbos/ec2-mac.pem ]]; then
  chown root:"$SVC_USER" /etc/arbos/ec2-mac.pem; chmod 0640 /etc/arbos/ec2-mac.pem
  : "${IOS_GATE_SSH:=ec2-user@13.217.52.163}"
fi
if [[ -f "$ENV_FILE" ]]; then
  keep() { local k=$1; [[ -n "${!k:-}" ]] || printf -v "$k" '%s' "$(sed -n "s/^$k=//p" "$ENV_FILE" | head -1)"; }
  for k in ASC_KEY_P8_B64 CURSOR_API_KEY GITHUB_TOKEN ARBOS_RELAY_ADMIN_TOKEN OPENROUTER_API_KEY FEEDBACK_PRIORITY FEEDBACK_AGENT_MODEL FEEDBACK_SCREEN_MODEL FEEDBACK_MIN_INTERVAL_S FEEDBACK_MAX_PER_DAY FEEDBACK_TESTER_MAX_PER_DAY FEEDBACK_DEDUPE_DAYS IOS_GATE_SSH IOS_GATE_TIMEOUT_S RUST_GATE_SSH RUST_GATE_TIMEOUT_S TESTFLIGHT_PUBLIC_GROUP_ID FEEDBACK_INTERNAL_MAX_PER_DAY; do keep "$k"; done
fi
missing=()
[[ -n "${ASC_KEY_P8_B64:-}" ]] || missing+=(ASC_KEY_P8_B64)
[[ -n "${CURSOR_API_KEY:-}" ]] || missing+=(CURSOR_API_KEY)
[[ -n "${GITHUB_TOKEN:-}" ]] || missing+=(GITHUB_TOKEN)
if [[ ${#missing[@]} -gt 0 ]]; then
  echo "missing: ${missing[*]} (export them, or give me OP_SERVICE_ACCOUNT_TOKEN for the Arbos vault)" >&2
  exit 1
fi
umask 077
cat > "$ENV_FILE" <<EOF
# Arbos feedback loop — written by scripts/feedback/install-feedback-loop.sh on $(date -u +%FT%TZ). root only.
ASC_KEY_ID=$ASC_KEY_ID
ASC_ISSUER_ID=$ASC_ISSUER_ID
ASC_APP_ID=$ASC_APP_ID
ASC_KEY_P8_B64=$ASC_KEY_P8_B64
CURSOR_API_KEY=$CURSOR_API_KEY
FEEDBACK_AGENT_MODEL=${FEEDBACK_AGENT_MODEL:-}
GITHUB_TOKEN=$GITHUB_TOKEN
GITHUB_REPO=$GITHUB_REPO
ARBOS_RELAY_URL=$ARBOS_RELAY_URL
ARBOS_RELAY_ADMIN_TOKEN=${ARBOS_RELAY_ADMIN_TOKEN:-}
OPENROUTER_API_KEY=${OPENROUTER_API_KEY:-}
FEEDBACK_SCREEN_MODEL=${FEEDBACK_SCREEN_MODEL:-openai/gpt-4o-mini}
FEEDBACK_PRIORITY=${FEEDBACK_PRIORITY:-jake@bittensor.com,@unconst}
FEEDBACK_PRIORITY_FILE=$PRIORITY_FILE
FEEDBACK_BLOCKLIST_FILE=$BLOCK_FILE
FEEDBACK_STATE_DIR=$STATE_DIR
FEEDBACK_KILL_FILE=/etc/arbos/feedback.disabled
FEEDBACK_MIN_INTERVAL_S=${FEEDBACK_MIN_INTERVAL_S:-90}
FEEDBACK_MAX_PER_DAY=${FEEDBACK_MAX_PER_DAY:-120}
FEEDBACK_TESTER_MAX_PER_DAY=${FEEDBACK_TESTER_MAX_PER_DAY:-20}
FEEDBACK_DEDUPE_DAYS=${FEEDBACK_DEDUPE_DAYS:-7}
TESTFLIGHT_PUBLIC_GROUP_ID=${TESTFLIGHT_PUBLIC_GROUP_ID:-}
# iOS pre-merge gate (EC2 Mac over ssh; empty = merge without compiling, CI catches it later)
IOS_GATE_SSH=${IOS_GATE_SSH:-}
IOS_GATE_KEY=/etc/arbos/ec2-mac.pem
IOS_GATE_TIMEOUT_S=${IOS_GATE_TIMEOUT_S:-1500}
RUST_GATE_SSH=${RUST_GATE_SSH:-$IOS_GATE_SSH}
RUST_GATE_TIMEOUT_S=${RUST_GATE_TIMEOUT_S:-3600}
FEEDBACK_INTERNAL_FILE=/etc/arbos/feedback-internal.txt
FEEDBACK_INTERNAL_MAX_PER_DAY=${FEEDBACK_INTERNAL_MAX_PER_DAY:-40}
EOF
chmod 0600 "$ENV_FILE"
umask 022
if [[ -f /etc/arbos/feedback-allowlist.txt && ! -f "$PRIORITY_FILE" ]]; then
  mv /etc/arbos/feedback-allowlist.txt "$PRIORITY_FILE"
fi
if [[ ! -f "$PRIORITY_FILE" ]]; then
  printf '# Priority testers (first in the queue, no per-tester cap): one email or @github-handle per line.\njake@bittensor.com\n@unconst\n' > "$PRIORITY_FILE"
fi
if [[ ! -f "$BLOCK_FILE" ]]; then
  printf '# Blocked testers (feedback dropped, logged): one email or @github-handle per line.\n' > "$BLOCK_FILE"
fi
chmod 0644 "$PRIORITY_FILE" "$BLOCK_FILE"

say "cursor"
if [[ -n "$SINCE" ]]; then
  sudo -u "$SVC_USER" env FEEDBACK_STATE_DIR="$STATE_DIR" python3 "$INSTALL_DIR/feedback_poller.py" set-cursor "$SINCE"
elif [[ ! -f "$STATE_DIR/cursor.json" ]]; then
  sudo -u "$SVC_USER" env FEEDBACK_STATE_DIR="$STATE_DIR" python3 "$INSTALL_DIR/feedback_poller.py" set-cursor "$(date -u +%FT%TZ)"
else
  echo "keeping $(sudo -u "$SVC_USER" env FEEDBACK_STATE_DIR="$STATE_DIR" python3 "$INSTALL_DIR/feedback_poller.py" status | head -1)"
fi

say "cli wrapper"
cat > /usr/local/bin/arbos-feedback <<'EOF2'
#!/usr/bin/env bash
# arbos-feedback status | inject <image> "<comment>" [ios|mac] | set-cursor <ISO> | requeue <id>… | prioritize <id>… | confirm <id> [note] | run  — as root.
set -euo pipefail
set -a; . /etc/arbos/feedback.env; set +a
exec sudo -E -u arbos-feedback /usr/bin/python3 /opt/arbos-feedback/feedback_poller.py "$@"
EOF2
chmod 0755 /usr/local/bin/arbos-feedback

say "systemd"
systemctl daemon-reload
systemctl enable --now arbos-feedback-poller.timer >/dev/null
systemctl start arbos-feedback-poller.service || true
sleep 1
journalctl -u arbos-feedback-poller.service -n 15 --no-pager -o cat || true
systemctl list-timers arbos-feedback-poller.timer --no-pager | head -3
echo
echo "installed. timer: sudo systemctl status arbos-feedback-poller.timer; items: sudo arbos-feedback status"
echo "stop the loop: sudo touch /etc/arbos/feedback.disabled    resume: sudo rm /etc/arbos/feedback.disabled"
