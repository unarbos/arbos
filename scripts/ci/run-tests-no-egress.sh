#!/usr/bin/env bash
# CI guard for this project's existing tests: build first, then permit loopback only.
# This is a network/privilege fence, not a container for arbitrary hostile programs.
set -euo pipefail

fail() { printf '%s\n' "no-egress test runner: $*" >&2; exit 1; }
binary() {
  local path
  path=$(type -P "$1") || fail "missing required tool: $1"
  [[ $path == /* && -x $path ]] || fail "tool must be an absolute executable: $1"
  printf '%s\n' "$path"
}

[[ $# == 0 ]] || fail 'arguments are not accepted'
[[ $(uname -s) == Linux ]] || fail 'Linux is required'
runner_uid=$(id -u)
runner_gid=$(id -g)
[[ $runner_uid =~ ^[1-9][0-9]*$ && $runner_gid =~ ^[1-9][0-9]*$ ]] || fail 'a non-root runner UID and GID are required'
workdir=$(pwd -P)
[[ -f Cargo.toml && -f Cargo.lock ]] || fail 'run from the Cargo workspace root'

cargo_bin=$(binary cargo)
sudo_bin=$(binary sudo)
unshare_bin=$(binary unshare)
ip_bin=$(binary ip)
setpriv_bin=$(binary setpriv)
env_bin=$(binary env)
bash_bin=$(binary bash)
python_bin=$(binary python3)
script_path=${BASH_SOURCE[0]}
[[ $script_path == */* ]] || script_path=./$script_path
script_dir=$(cd -- "${script_path%/*}" && pwd -P)
smoke_path=$script_dir/netns-smoke.py
[[ -f $smoke_path ]] || fail 'missing namespace smoke check'
[[ $("$setpriv_bin" --help) == *--reset-env* ]] || fail 'setpriv --reset-env is required'

# Identical build/test environments avoid a rebuild caused by losing compiler settings.
# setpriv initializes identity variables from passwd; HOME is never assigned here.
allowed=("PATH=$PATH" "RUSTFLAGS=${RUSTFLAGS:-}" "CARGO_TERM_COLOR=${CARGO_TERM_COLOR:-always}")
for name in CARGO_HOME RUSTUP_HOME CC CXX LIBCLANG_PATH CARGO_INCREMENTAL; do
  if [[ ${!name+x} ]]; then
    allowed+=("$name=${!name}")
  fi
done

"$env_bin" -i "$setpriv_bin" --reset-env "$env_bin" "${allowed[@]}" \
  "$cargo_bin" test --workspace --locked --no-run

# Clear the root shell's environment as well: BASH_ENV must not run before setpriv.
exec "$sudo_bin" -n "$env_bin" -i "$unshare_bin" --net \
  "$bash_bin" --noprofile --norc -euo pipefail -c '
    runner_uid=$1 runner_gid=$2 workdir=$3 ip_bin=$4 setpriv_bin=$5 env_bin=$6 cargo_bin=$7 python_bin=$8 smoke_path=$9
    shift 9
    cd -- "$workdir"
    "$ip_bin" link set dev lo up
    exec "$setpriv_bin" --reuid="$runner_uid" --regid="$runner_gid" \
      --clear-groups --no-new-privs --inh-caps=-all --ambient-caps=-all --bounding-set=-all \
      --reset-env "$env_bin" "$@" "$python_bin" "$smoke_path" "$runner_uid" "$runner_gid" -- \
      "$cargo_bin" test --workspace --locked --offline
  ' test-netns "$runner_uid" "$runner_gid" "$workdir" "$ip_bin" "$setpriv_bin" \
  "$env_bin" "$cargo_bin" "$python_bin" "$smoke_path" "${allowed[@]}"
