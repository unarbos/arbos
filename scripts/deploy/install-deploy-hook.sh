#!/usr/bin/env bash
# Install the continuous-delivery hook + worker on the relay host (run as root from a checkout):
#   sudo scripts/deploy/install-deploy-hook.sh
# Puts arbos-deploy-hook and arbos-deploy-worker in /usr/local/bin, the queue directory under the
# poller's state dir (writable by arbos-feedback), a sudoers line so `sudo -n arbos-deploy-hook`
# also works, /etc/arbos/deploy.env (knobs), and enables arbos-deploy.timer. Idempotent.
set -euo pipefail
[[ $EUID -eq 0 ]] || { echo "run as root (sudo)" >&2; exit 1; }
HERE="$(cd "$(dirname "$0")" && pwd)"
install -m 0755 "$HERE/arbos-deploy-hook" /usr/local/bin/arbos-deploy-hook
bash -n "$HERE/arbos-deploy-worker"
# A worker that cannot complete a tick is not installed (Oct 23 17:55-18:40; Oct 26 04:40-07:10, #1879:
# selftest alone passed while the real tick died on a missing queue file). Three checks: shellcheck,
# the pure-helper selftest, and a dry tick against a copy of the live state and queue.
if command -v shellcheck >/dev/null 2>&1; then shellcheck -S warning "$HERE/arbos-deploy-worker"; else echo "warning: shellcheck not installed; skipping" >&2; fi
bash "$HERE/arbos-deploy-worker" selftest
bash "$HERE/arbos-deploy-worker" dry-tick
install -m 0755 "$HERE/arbos-deploy-worker" /usr/local/bin/arbos-deploy-worker
install -m 0644 "$HERE/arbos-deploy.service" /etc/systemd/system/arbos-deploy.service
install -m 0644 "$HERE/arbos-deploy.timer" /etc/systemd/system/arbos-deploy.timer
install -d -m 0755 /var/lib/arbos-deploy /var/lib/arbos-deploy/failed /var/lib/arbos-deploy/runs
if id arbos-feedback >/dev/null 2>&1; then
  install -d -m 0755 -o arbos-feedback -g arbos-feedback /var/lib/arbos-feedback/deploy-requests
  printf 'arbos-feedback ALL=(root) NOPASSWD: /usr/local/bin/arbos-deploy-hook\n' > /etc/sudoers.d/arbos-deploy-hook
  chmod 0440 /etc/sudoers.d/arbos-deploy-hook
  visudo -cf /etc/sudoers.d/arbos-deploy-hook >/dev/null
else
  install -d -m 0755 /var/lib/arbos-feedback/deploy-requests
fi
if [[ ! -f /etc/arbos/deploy.env ]]; then
  cat > /etc/arbos/deploy.env <<'ENV'
# Arbos continuous delivery knobs (read by arbos-deploy-worker). 1 = deploy main automatically.
AUTO_DEPLOY_RELAY=1
AUTO_DEPLOY_AGENTD=1
AUTO_DEPLOY_VOICE=1
# Debounce: deploy SETTLE_S after the last request, at most MAX_WAIT_S after the first.
SETTLE_S=120
MAX_WAIT_S=300
ENV
  chmod 0644 /etc/arbos/deploy.env
fi
# The image the boxes run today, so the worker knows what "deployed" means for agentd.
if [[ ! -f /var/lib/arbos-deploy/agentd.deployed-sha && -n "${AGENTD_DEPLOYED_SHA:-}" ]]; then
  echo "$AGENTD_DEPLOYED_SHA" > /var/lib/arbos-deploy/agentd.deployed-sha
fi
# /download/mac flips the moment the relay hears about a stable Mac release (the 15-min timer stays).
install -m 0644 "$HERE/arbos-refresh-mac-download.path" /etc/systemd/system/arbos-refresh-mac-download.path
systemctl daemon-reload
systemctl enable --now arbos-deploy.timer
if systemctl list-unit-files arbos-refresh-mac-download.service >/dev/null 2>&1; then
  systemctl enable --now arbos-refresh-mac-download.path
fi
echo "installed: arbos-deploy-hook, arbos-deploy-worker, arbos-deploy.timer ($(systemctl is-active arbos-deploy.timer))"
echo "kill switch: touch /etc/arbos/deploy.disabled"
