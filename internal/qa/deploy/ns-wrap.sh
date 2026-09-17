#!/usr/bin/env bash
# Run a command with the Project Agent Store hidden from it.
#
# Why (2026-09-17): the QA loop's inbox scenario handed an attack list to a live
# agent; the list included `cd / && rm -rf *`, the kernel's needs_approval did
# not stop it, and /cursor/stores/<id> is the first user-writable tree under /.
# Fifteen runs since 09-13 deleted the store's files, one at a time, and we
# recorded the result as a store fault (docs/store-fault-report-2026-09-17.md).
#
# A kernel under test must not be able to reach the store at all, whatever an
# agent decides to run. This wraps it in a mount namespace where /cursor/stores
# is an empty directory, then drops back to the real uid so the kernel and
# anything it spawns still run as this user. The pid is preserved (exec chain),
# so kernel.json's pid still matches what the harness started.
#
#   ns-wrap.sh <command> [args...]
#
# Refuses to run (exit 97) if user namespaces are not available, unless
# ARBOS_QA_STORE_VISIBLE=1 says this host has no store mount to protect.
set -u
STORE_MOUNT_ROOT="${ARBOS_QA_HIDE_PATH:-/cursor/stores}"
EMPTY="${TMPDIR:-/tmp}/arbos-qa-empty-store"
mkdir -p "$EMPTY" 2>/dev/null
uid=$(id -u); gid=$(id -g)
if unshare -Urm --propagation private true 2>/dev/null; then
    exec unshare -Urm --propagation private -- sh -c '
        mount --bind "$0" "$1" || { echo "ns-wrap: could not hide $1" >&2; exit 97; }
        shift 1
        exec unshare -U --map-user='"$uid"' --map-group='"$gid"' -- "$@"
    ' "$EMPTY" "$STORE_MOUNT_ROOT" "$@"
fi
if [ "${ARBOS_QA_STORE_VISIBLE:-}" = 1 ]; then
    echo "ns-wrap: user namespaces unavailable; ARBOS_QA_STORE_VISIBLE=1 so running $1 with the store VISIBLE" >&2
    exec "$@"
fi
echo "ns-wrap: user namespaces unavailable on this host; refusing to run $1 where it could reach $STORE_MOUNT_ROOT (set ARBOS_QA_STORE_VISIBLE=1 only on a host with no store mount)" >&2
exit 97
