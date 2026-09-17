#!/usr/bin/env bash
# Run a command with the Project Agent Store hidden and this user's own trees read-only.
#
# Why (2026-09-17): the QA loop's inbox scenario handed an attack list to a live
# agent; the list included `cd / && rm -rf *`, the kernel's needs_approval did
# not stop it, and as this user the walk succeeds on every user-writable tree
# under /. Between 09-16 09:02 and 09-17 07:33 that was /cursor/stores (the
# store, seven times); at 08:13 the same command, with the store hidden, took
# ~/arbos-qa/{repo,deploy,logs,state,...} and half of ~ before the reaper got it.
#
# A kernel under test must not be able to reach anything of ours, whatever an
# agent decides to run. Inside a mount namespace:
#   - /cursor/stores is an empty directory
#   - the user's home and /workspace are read-only (binaries there still run)
#   - everything under /tmp stays writable: that is where scenario places live
# then back to the real uid, pid preserved (exec chain), so kernel.json's pid
# still matches what the harness started.
#
#   ns-wrap.sh <command> [args...]
#
# Refuses to run (exit 97) if user namespaces are not available, unless
# ARBOS_QA_STORE_VISIBLE=1 says this host has nothing of ours to protect.
set -u
HIDE="${ARBOS_QA_HIDE_PATH:-/cursor/stores}"
REAL_HOME="$(getent passwd "$(id -u)" | cut -d: -f6)"
RO="${ARBOS_QA_PROTECT_RO:-$REAL_HOME /workspace}"
EMPTY="/tmp/arbos-qa-empty-store"
mkdir -p "$EMPTY" 2>/dev/null
uid=$(id -u); gid=$(id -g)
if unshare -Urm --propagation private true 2>/dev/null; then
    exec unshare -Urm --propagation private -- sh -c '
        empty="$1"; hide="$2"; ro="$3"; shift 3
        [ -d "$hide" ] && { mount --bind "$empty" "$hide" || { echo "ns-wrap: could not hide $hide" >&2; exit 97; }; }
        for d in $ro; do
            [ -d "$d" ] || continue
            mount --bind "$d" "$d" && mount -o remount,bind,ro "$d" || { echo "ns-wrap: could not make $d read-only" >&2; exit 97; }
        done
        exec unshare -U --map-user='"$uid"' --map-group='"$gid"' -- "$@"
    ' sh "$EMPTY" "$HIDE" "$RO" "$@"
fi
if [ "${ARBOS_QA_STORE_VISIBLE:-}" = 1 ]; then
    echo "ns-wrap: user namespaces unavailable; ARBOS_QA_STORE_VISIBLE=1 so running $1 unprotected" >&2
    exec "$@"
fi
echo "ns-wrap: user namespaces unavailable on this host; refusing to run $1 where it could reach $HIDE or write $RO (set ARBOS_QA_STORE_VISIBLE=1 only on a host with nothing of ours on it)" >&2
exit 97
