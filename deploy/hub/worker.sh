#!/bin/bash
# Offer this machine to the mesh. Everything Arbos touches stays under <dir>:
#   <dir>/bin/arbos-kernel            the binary (build: cargo build --release -p arbos-kernel)
#   <dir>/config/arbos/config.toml    model + key for kernels started here (0600)
#   <dir>/config/arbos/hub.toml       url, machine, token (0600)
#   <dir>/projects/<name>/            checkouts a remote `spawn host=<machine>` may use
#   <dir>/cache, <dir>/logs
# Kernels the worker starts run as this user with this config, in a git
# worktree of the checkout (.arbos/worktrees/<claim>, branch arbos/<claim>).
dir=${ARBOS_WORKER_DIR:-$HOME/arbos-hub}
export XDG_CONFIG_HOME="$dir/config" XDG_CACHE_HOME="$dir/cache"
mkdir -p "$dir/projects" "$dir/logs"
chmod 600 "$dir"/config/arbos/*.toml 2>/dev/null
while true; do
  "$dir/bin/arbos-kernel" worker --dir "$dir/projects" "$@" >> "$dir/logs/worker.log" 2>&1
  sleep 2
done
