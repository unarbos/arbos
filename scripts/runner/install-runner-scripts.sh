#!/bin/bash
# Put the EC2 Mac's runner scripts where the box reads them, from this checkout, so one path is authoritative.
#
#   scripts/runner/install-runner-scripts.sh             # --check: report drift, write nothing (the default)
#   sudo scripts/runner/install-runner-scripts.sh --install
#   scripts/runner/install-runner-scripts.sh --only arbos-heavy-slot.sh
#   scripts/runner/install-runner-scripts.sh --check --dest-root /tmp/dryrun   # rehearse off the box
#
# Why this exists (2026-10-31): the box ran /usr/local/bin/arbos-heavy-slot.sh two commits behind main for
# over 24 hours, because installing was a hand action that nothing prompted, checked or recorded. A lane that
# wanted newer behaviour copied the script to /tmp instead, and a third copy went live. Every finding about
# "the script" then had to name which copy. Nothing in the repo closed that gap; this does.
#
# Rules it keeps:
#  - --check is the default and writes nothing anywhere. Safe on a busy box, and safe in CI.
#  - It never deletes. Stray copies are reported, never removed: one may be executing right now, and a
#    half-deleted lane is worse than a stale one.
#  - It does not load, unload or kickstart launchd jobs, and it does not restart a runner. Those have to be
#    timed against what the box is doing; it prints the commands and leaves them to a person.
#  - /bin/bash on the gate Mac is 3.2: no associative arrays, no mapfile, no ${var^^}.
#
# READ THIS BEFORE THE FIRST --install ON THE BOX. Unifying the copies is not only a tidy-up. On 2026-10-31
# a feature lane taking evidence shots queued 1322 s for a slot and was then granted one ONLY because its
# /tmp copy predates release_held/refuse_in_release; the installed copy would have refused it outright with
# exit 75. So the drift this script exists to remove is currently the one reason a real product lane got
# through. Closing the gap turns that twenty-minute wait into an immediate refusal. Decide first what a
# feature lane should get while a release holds the box, and write the answer into docs/process-rules.md.
# Discovering this after the install is the expensive order.
#
# Exit: 0 everything in step (or installed), 1 drift found by --check, 2 bad arguments, 3 an install failed.
set -uo pipefail

SRC_DIR=$(cd "$(dirname "$0")" && pwd)
MODE=check
DEST_ROOT=""
ONLY=""

while [ $# -gt 0 ]; do
  case "$1" in
    --check) MODE=check ;;
    --install) MODE=install ;;
    --only) ONLY="${2:-}"; [ -n "$ONLY" ] || { echo "--only needs a source file name" >&2; exit 2; }; shift ;;
    --dest-root) DEST_ROOT="${2:-}"; [ -n "$DEST_ROOT" ] || { echo "--dest-root needs a directory" >&2; exit 2; }; shift ;;
    -h|--help) sed -n '2,/^[^#]/p' "$0" | sed '$d;s/^# \{0,1\}//'; exit 0 ;;
    *) echo "usage: $0 [--check | --install] [--only <source file>] [--dest-root <dir>]" >&2; exit 2 ;;
  esac
  shift
done

# The manifest: src | dest | mode | root? | what reads it there.
# Every destination below is one the repo states for itself — an "Installed as"/"Installed copy" line in the
# script, a ProgramArguments path in a plist, a caller, or docs/process-rules.md. Add nothing here on a hunch:
# a wrong destination in an installer is worse than no installer, because it looks authoritative.
manifest() {
  cat <<'EOF'
arbos-heavy-slot.sh|/usr/local/bin/arbos-heavy-slot.sh|755|root|ios.yml, release.yml, every rig, sh.arbos.heavy-slot-reclaim
arbos-mac-clean.sh|/usr/local/bin/arbos-mac-clean.sh|755|root|sh.arbos.mac-clean and sh.arbos.mac-clean-low
xcrun-slot-guard.sh|/usr/local/bin/xcrun|755|root|ahead of /usr/bin on the rig users' PATH (#1957)
xcrun-slot-guard.sh|/Users/Shared/arbos-run/axbin/xcrun|755|user|the same guard on the shared runners' PATH
gui-lock.sh|/Users/Shared/arbos-run/gui-lock.sh|755|user|scripts/feedback/mac-enter-smoke.sh and the shot rigs
runner.sh|/Users/Shared/arbos-run/runner.sh|755|user|the Terminal-identity shared command runner
ax-runner.sh|/Users/Shared/arbos-run/ax-runner.sh|755|user|the Accessibility-identity shared command runner
ax|/Users/Shared/arbos-run/ax|755|user|ax-osascript-shim.sh execs it by this exact path
ax-osascript-shim.sh|/Users/Shared/arbos-run/axbin/osascript|755|user|osascript calls from runner.sh commands
sh.arbos.heavy-slot-reclaim.plist|/Library/LaunchDaemons/sh.arbos.heavy-slot-reclaim.plist|644|root|launchd, every 300 s
sh.arbos.mac-clean-low.plist|/Library/LaunchDaemons/sh.arbos.mac-clean-low.plist|644|root|launchd, every 1800 s
EOF
}

# Not installed anywhere, on purpose: install-self-hosted-runner.sh is run from a checkout with sudo and
# writes its own LaunchDaemon, and this file is run from a checkout too.

# Copies that exist on the box and are read by someone, but are nobody's destination. Reported, never touched.
strays() {
  cat <<'EOF'
/tmp/arbos-heavy-slot.sh|a lane ran this on 2026-10-31; four days old and missing the release-refusal rule
/tmp/new-arbos-heavy-slot.sh|predates the versioned series; no process was using it on 2026-10-31
EOF
}

SHA_CMD=""
for c in shasum sha256sum; do
  command -v "$c" >/dev/null 2>&1 && { SHA_CMD=$c; break; }
done
sha_of() {  # sha256 of $1, or nothing when it cannot be read
  [ -r "$1" ] || return 0
  case "$SHA_CMD" in
    shasum) shasum -a 256 "$1" 2>/dev/null | awk '{print $1}' ;;
    sha256sum) sha256sum "$1" 2>/dev/null | awk '{print $1}' ;;
    *) return 0 ;;
  esac
}

# Best effort, and the single most useful line in a drift report: name the commit whose blob the installed
# file is. This is how the 2026-10-31 read dated the box's three copies. Silent when the checkout cannot
# answer (no git, a shallow clone, or content that was never committed).
dates_from_git() {
  local blob commit
  command -v git >/dev/null 2>&1 || return 0
  git -C "$SRC_DIR" rev-parse --git-dir >/dev/null 2>&1 || return 0
  blob=$(git -C "$SRC_DIR" hash-object -- "$1" 2>/dev/null) || return 0
  [ -n "$blob" ] || return 0
  # --find-object lists every commit that added OR removed the blob, newest first. The oldest of them is the
  # one that introduced this content; the newest is usually the commit that replaced it, which would name the
  # wrong version. Hence tail, not head.
  commit=$(git -C "$SRC_DIR" log --all --oneline --find-object="$blob" 2>/dev/null | tail -1)
  [ -n "$commit" ] && printf ', committed in %s' "$commit"
  return 0
}

# BSD stat on the Mac, GNU stat elsewhere. These cannot be tried in sequence: GNU `stat -f` means "stat the
# filesystem" and exits 0 with unrelated numbers, so a BSD-first fallback prints garbage on Linux instead of
# falling through. Switch on the kernel.
if [ "$(uname -s)" = Darwin ]; then
  mtime_of() { [ -e "$1" ] && stat -f '%Sm' -t '%Y-%m-%d %H:%M' "$1" 2>/dev/null; return 0; }
else
  mtime_of() { [ -e "$1" ] && stat -c '%y' "$1" 2>/dev/null | cut -c1-16; return 0; }
fi

[ -n "$SHA_CMD" ] || { echo "neither shasum nor sha256sum is on PATH; cannot compare" >&2; exit 2; }

needs_root=$(manifest | awk -F'|' -v only="$ONLY" '(only == "" || $1 == only) && $4 == "root" { n++ } END { print n + 0 }')

if [ "$MODE" = install ] && [ -z "$DEST_ROOT" ] && [ "$needs_root" != 0 ] && [ "$(id -u)" != 0 ]; then
  echo "--install needs root for /usr/local/bin and /Library/LaunchDaemons: re-run with sudo," >&2
  echo "or select a user-owned entry with --only (and see --dest-root to rehearse)." >&2
  exit 2
fi

drift=0
failed=0
installed=0
printf '%s from %s\n\n' "$MODE" "$SRC_DIR"

while IFS='|' read -r src dest fmode _ why; do   # field 4 (root?) was consumed in the pre-flight above
  [ -n "${src:-}" ] || continue
  [ -z "$ONLY" ] || [ "$ONLY" = "$src" ] || continue
  full_dest="$DEST_ROOT$dest"
  src_path="$SRC_DIR/$src"

  if [ ! -r "$src_path" ]; then
    printf 'MISSING SOURCE  %s (manifest says it belongs at %s)\n' "$src" "$dest"
    failed=1
    continue
  fi

  src_sha=$(sha_of "$src_path")
  dest_sha=$(sha_of "$full_dest")

  if [ -z "$dest_sha" ]; then
    state=absent
  elif [ "$src_sha" = "$dest_sha" ]; then
    state=same
  else
    state=differs
  fi

  if [ "$MODE" = check ]; then
    case "$state" in
      same)
        printf 'in step        %s\n' "$full_dest"
        ;;
      differs)
        drift=1
        printf 'DRIFT          %s\n' "$full_dest"
        printf '                 on the box  %s%s\n' "$(mtime_of "$full_dest")" "$(dates_from_git "$full_dest")"
        printf '                 this tree   %s%s\n' "$(mtime_of "$src_path")" "$(dates_from_git "$src_path")"
        printf '                 read by   %s\n' "$why"
        ;;
      absent)
        drift=1
        printf 'NOT INSTALLED  %s (read by %s)\n' "$full_dest" "$why"
        ;;
      *) ;;
    esac
    continue
  fi

  if [ "$state" = same ]; then
    printf 'in step        %s\n' "$full_dest"
    continue
  fi

  dest_dir=$(dirname "$full_dest")
  if [ ! -d "$dest_dir" ]; then
    install -d "$dest_dir" || { printf 'FAILED         %s (cannot create %s)\n' "$full_dest" "$dest_dir"; failed=1; continue; }
  fi
  if install -m "$fmode" "$src_path" "$full_dest"; then
    printf 'installed      %s (mode %s, was %s)\n' "$full_dest" "$fmode" "$state"
    installed=$((installed + 1))
  else
    printf 'FAILED         %s\n' "$full_dest"
    failed=1
  fi
done <<EOF
$(manifest)
EOF

printf '\nstray copies (reported, never removed — one may be running):\n'
found_stray=0
while IFS='|' read -r path note; do
  [ -n "${path:-}" ] || continue
  [ -e "$DEST_ROOT$path" ] || continue
  found_stray=1
  printf '  %s  %s%s\n' "$DEST_ROOT$path" "$(mtime_of "$DEST_ROOT$path")" "$(dates_from_git "$DEST_ROOT$path")"
  printf '    %s\n' "$note"
done <<EOF
$(strays)
EOF
[ "$found_stray" = 1 ] || printf '  none of the known paths exist here\n'

if [ "$MODE" = install ] && [ "$installed" != 0 ]; then
  cat <<'EOF'

Follow-ups this script deliberately does not do, because they have to be timed against what the box is
running. Do them by hand, and not while a release or a gate half holds a heavy slot:
  - a changed plist needs launchd told:   sudo launchctl bootout system/<label> 2>/dev/null; sudo launchctl bootstrap system /Library/LaunchDaemons/<label>.plist
  - a changed runner.sh or ax-runner.sh is picked up on its next command, not by the process already running
EOF
fi

if [ "$failed" != 0 ]; then exit 3; fi
if [ "$MODE" = check ] && [ "$drift" != 0 ]; then exit 1; fi
exit 0
