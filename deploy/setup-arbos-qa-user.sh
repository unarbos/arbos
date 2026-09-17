#!/usr/bin/env bash
# Move the QA loop on ArbosLife to its own Unix user, `arbos-qa`, so a kernel
# bug (qa-020: kill(-1)) can only ever reach the loop's own processes.
#
# Run as `const` (needs sudo), once:
#   ~/arbos-qa/deploy/setup-arbos-qa-user.sh
#
# What it does:
#   1. creates user arbos-qa (no password, own home, bash)
#   2. copies /home/const/arbos-qa -> /home/arbos-qa/arbos-qa (repo, toolchain,
#      loop, deploy, results, secrets.env; not staging or logs), fixes the
#      absolute linker path in the cargo config, chowns everything
#   3. checks the tracked branches carry the killpg fix (#73 / #74); if not,
#      says so and continues: cycle.sh installs the `kill` shim either way
#   4. enables linger for arbos-qa and installs the timer under that user
#   5. makes sure nothing is left enabled under const
# Idempotent: safe to run again.
set -euo pipefail
SRC="${ARBOS_QA_SRC:-/home/const/arbos-qa}"
USER_NAME=arbos-qa
DST_HOME="/home/$USER_NAME"
DST="$DST_HOME/arbos-qa"
TRACK="${ARBOS_QA_TRACK_BRANCHES:-cursor/release-integration-52cd}"

[ -d "$SRC" ] || { echo "no $SRC"; exit 1; }
sudo -n true 2>/dev/null || { echo "needs sudo"; exit 1; }

# 1. user
if ! id "$USER_NAME" >/dev/null 2>&1; then
  sudo useradd --create-home --shell /bin/bash --comment "Arbos QA loop" "$USER_NAME"
  echo "-- created user $USER_NAME"
fi
UID_QA=$(id -u "$USER_NAME")

# 2. copy the tree (rsync keeps it idempotent; caches come along so the first
#    cycle does not rebuild from scratch)
sudo mkdir -p "$DST"
sudo rsync -a --delete \
  --exclude staging --exclude logs --exclude 'repo-inbox/*/target' \
  "$SRC/" "$DST/"
sudo mkdir -p "$DST/logs" "$DST/staging" "$DST/state"
# the zig linker path was written for const's home
if [ -f "$DST/toolchain/cargo/config.toml" ]; then
  sudo sed -i "s#/home/const/arbos-qa#$DST#g" "$DST/toolchain/cargo/config.toml"
fi
sudo chown -R "$USER_NAME:$USER_NAME" "$DST"
sudo chmod 700 "$DST"
sudo chmod 600 "$DST/secrets.env"
echo "-- tree copied to $DST"

# 3. is the killpg fix on the branches under test?
cd "$DST/repo"
sudo -u "$USER_NAME" git fetch -q origin rust $TRACK || true
for b in rust $TRACK; do
  if sudo -u "$USER_NAME" git log "origin/$b" --oneline 2>/dev/null | grep -q "kill a job's process group with killpg"; then
    echo "-- $b: killpg fix present"
  else
    echo "!! $b: killpg fix (#73/#74) NOT present; the kill shim in cycle.sh is the only guard, and it only protects processes of $USER_NAME"
  fi
done

# 4. units under arbos-qa
sudo loginctl enable-linger "$USER_NAME"
RUN="/run/user/$UID_QA"
# the user manager starts with linger; give it a moment
for _ in 1 2 3 4 5 6 7 8 9 10; do [ -S "$RUN/bus" ] && break; sleep 1; done
asqa() { sudo -u "$USER_NAME" XDG_RUNTIME_DIR="$RUN" DBUS_SESSION_BUS_ADDRESS="unix:path=$RUN/bus" "$@"; }
asqa systemctl --user daemon-reload
asqa systemctl --user link "$DST/deploy/arbos-qa.service" "$DST/deploy/arbos-qa.timer" 2>/dev/null || true
asqa systemctl --user daemon-reload
asqa systemctl --user enable --now arbos-qa.timer
asqa systemctl --user list-timers arbos-qa.timer --no-pager
echo "-- timer enabled under $USER_NAME (tracks: rust + $TRACK)"

# 5. nothing left under const
for u in arbos-qa.timer arbos-qa.service; do
  systemctl --user disable --now "$u" 2>/dev/null || true
done
rm -f ~/.config/systemd/user/arbos-qa.timer ~/.config/systemd/user/arbos-qa.service \
      ~/.config/systemd/user/timers.target.wants/arbos-qa.timer 2>/dev/null || true
systemctl --user daemon-reload 2>/dev/null || true
echo "-- const: no arbos-qa units"
echo "== done. Logs: $DST/logs; results: branch qa-results; stop with: sudo -u $USER_NAME XDG_RUNTIME_DIR=$RUN systemctl --user disable --now arbos-qa.timer"
