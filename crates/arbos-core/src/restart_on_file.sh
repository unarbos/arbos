#!/bin/sh
# Restart every process of this user running a given kernel binary.
#
# Run on the machine whose binary was just replaced. It is the half of an
# install that installers usually get wrong: replacing the file and restarting
# "the service I was asked about" leaves every other process on that file
# running a deleted image. On ArbosLife that was two kernels for 5 h 34 m; on
# Jacob's box it was `subnet120`, three times.
#
#   restart_on_file.sh <binary> [--dry-run]
#
# Nothing here matches a process by name, and nothing signals a pid it did not
# find through /proc as this user — an unprivileged reader gets EACCES on
# another user's exe link, so the confinement is the kernel's permission check
# rather than a filter that could be wrong.
#
# ── how a process is matched ─────────────────────────────────────────────────
# By identity, and by every name the file has had. The inode now at the path
# finds processes started since the last install; the `(deleted)` link finds
# the ones an *earlier* install stranded, which is the class that broke
# ArbosLife — its worker daemon sat on an inode two installs old while the path
# already held a newer build, and a pass matching only the current inode would
# have walked straight past the one process refusing every spawn.
#
# ── how a process is restarted ───────────────────────────────────────────────
# By waiting to see what happens, not by deciding what it is. `ppid` cannot
# tell a systemd unit (parent 1, supervised) from a detached kernel (parent 1,
# not), nor a tmux pane (parent not 1, nothing restarts it) from a `while true`
# loop (parent not 1, restarts it). Six shapes exist on these machines and any
# classifier misreads two of them. So: stop it, watch the place for a
# replacement, and relaunch it only if none arrives. That is right for all six
# without knowing which one it is, and it removes the place-lock race — the
# relaunch happens only after the old process is gone and nothing else has
# taken the lock.
set -u

BIN="${1:?usage: restart_on_file.sh <binary> [--dry-run]}"
DRY="${2:-}"
WAIT_EXIT=15        # seconds to wait for a stopped kernel to go
WAIT_REPLACEMENT=10 # seconds to wait for a supervisor to bring one back
say() { printf '%s\n' "$*"; }

case "$BIN" in /*) ;; *) BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")" ;; esac
[ -f "$BIN" ] || { say "no such binary: $BIN"; exit 1; }

# Every name this file has had, so a process stranded by an earlier install is
# found too. `install_file` keeps the build it replaced as `.previous`; the
# older updater moved it aside as `.<name>.arbos-old`.
dir="$(dirname "$BIN")"; base="$(basename "$BIN")"
NAMES="$BIN
$BIN.previous
$dir/.$base.arbos-old"

new_inode="$(stat -L -c '%d:%i' "$BIN" 2>/dev/null)"

# ── 1. record, before anything is stopped ───────────────────────────────────
# argv, cwd and the fds a relaunch needs. argv[0] is often relative (the parity
# loop runs `bin/arbos-kernel serve …`), so cwd is not optional.
found=""
for p in /proc/[0-9]*; do
    pid="${p#/proc/}"
    link="$(readlink "$p/exe" 2>/dev/null)" || continue   # EACCES: not ours
    ino="$(stat -L -c '%d:%i' "$p/exe" 2>/dev/null)"
    hit=""
    [ -n "$ino" ] && [ "$ino" = "$new_inode" ] && hit=1
    if [ -z "$hit" ]; then
        # A deleted file keeps its name in the magic link, with a suffix.
        stripped="${link% (deleted)}"
        for n in $NAMES; do [ "$stripped" = "$n" ] && hit=1 && break; done
    fi
    [ -n "$hit" ] || continue
    found="$found $pid"
done

[ -n "$found" ] || { say "no process of this user is running $BIN"; exit 0; }

record() { # record <pid> <what>
    case "$2" in
        cmdline) tr '\0' '\n' < "/proc/$1/cmdline" 2>/dev/null ;;
        cwd)     readlink "/proc/$1/cwd" 2>/dev/null ;;
        out)     readlink "/proc/$1/fd/1" 2>/dev/null ;;
        err)     readlink "/proc/$1/fd/2" 2>/dev/null ;;
        place)   tr '\0' '\n' < "/proc/$1/cmdline" 2>/dev/null | awk 'p{print;exit} /^serve$/{p=1}' ;;
    esac
}

# ── 2. busy? a stale kernel beats interrupted work ──────────────────────────
busy_reason() { # busy_reason <pid> <place>
    pid="$1"; place="$2"
    # Children of the kernel are tools and jobs it is running.
    kids="$(pgrep -P "$pid" 2>/dev/null | tr '\n' ' ')"
    [ -n "$kids" ] && { printf 'it has child processes (%s)\n' "${kids% }"; return; }
    # A turn whose folder has no `ended` is a turn in flight.
    if [ -n "$place" ] && [ -d "$place/.arbos/agents" ]; then
        open="$(grep -rLs '^ended' "$place"/.arbos/agents/*/turns/*/meta.toml 2>/dev/null | head -1)"
        [ -n "$open" ] && { printf 'a turn is in flight (%s)\n' "$open"; return; }
    fi
    printf ''
}

# ── 3. worktree children first ──────────────────────────────────────────────
# A kernel serving `.arbos/worktrees/<claim>` is another machine's spawn. The
# daemon never restarts one and starts fresh ones on demand, so an idle one is
# stopped and left stopped. Children go before the daemon: stopping the daemon
# first reparents them to PID 1, where they would look like the detached shape
# and be relaunched as lingering kernels.
order=""
for pid in $found; do
    case "$(record "$pid" place)" in *"/.arbos/worktrees/"*) order="$pid $order" ;; *) order="$order $pid" ;; esac
done

# ── 4. stop, watch, and relaunch only if nothing came back ──────────────────
alive() { kill -0 "$1" 2>/dev/null; }

for pid in $order; do
    place="$(record "$pid" place)"; cwd="$(record "$pid" cwd)"
    out="$(record "$pid" out)"; err="$(record "$pid" err)"
    cmd="$(record "$pid" cmdline)"
    worktree=""; case "$place" in *"/.arbos/worktrees/"*) worktree=1 ;; esac
    why="$(busy_reason "$pid" "$place")"
    if [ -n "$why" ]; then
        say "leave  pid $pid ${place:-?} — $why"
        continue
    fi
    if [ -n "$DRY" ]; then
        say "would  pid $pid ${place:-?}${worktree:+ (worktree child, stop only)}"
        continue
    fi

    kill -TERM "$pid" 2>/dev/null
    i=0; while alive "$pid" && [ "$i" -lt "$((WAIT_EXIT * 10))" ]; do sleep 0.1; i=$((i + 1)); done
    alive "$pid" && { say "stuck  pid $pid ${place:-?} — did not stop; left alone"; continue; }

    if [ -n "$worktree" ]; then
        say "stop   pid $pid $place — worktree child, not relaunched"
        continue
    fi

    # Did something bring it back? A new pid on this place whose exe is the
    # new file is a supervisor doing its job, whatever shape it is.
    back=""; i=0
    while [ "$i" -lt "$((WAIT_REPLACEMENT * 2))" ]; do
        npid="$(sed -n 's/.*"pid":[[:space:]]*\([0-9]*\).*/\1/p' \
            "$place/.arbos/runtime/kernel.json" 2>/dev/null | head -1)"
        if [ -n "$npid" ] && [ "$npid" != "$pid" ] && alive "$npid" \
           && [ "$(stat -L -c '%d:%i' "/proc/$npid/exe" 2>/dev/null)" = "$new_inode" ]; then
            back="$npid"; break
        fi
        sleep 0.5; i=$((i + 1))
    done
    if [ -n "$back" ]; then
        say "back   pid $pid -> $back $place — its supervisor restarted it"
        continue
    fi

    # Nothing did. Relaunch from what was recorded, detached, so a parent that
    # is not a supervisor cannot take it down with it.
    ( cd "${cwd:-/}" 2>/dev/null || exit 1
      # shellcheck disable=SC2086
      setsid sh -c 'exec "$@" >>"$0" 2>>"$1"' "${out:-/dev/null}" "${err:-/dev/null}" $cmd \
        < /dev/null & ) 2>/dev/null
    sleep 1
    say "start  pid $pid $place — nothing restarted it, relaunched from its own command line"
done

say "done. voice gateway: $(curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:8765/healthz 2>/dev/null || echo 'not checked')"
