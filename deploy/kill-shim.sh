#!/usr/bin/env bash
# `kill` shim (qa-020 stopgap). procps `kill -9 -12345` without `--` is read
# as kill(-1): every process of the user. Any `kill` whose arguments hold a
# negative number after a signal gets `--` inserted in front of it, so the
# number is a process group. Everything else passes through untouched.
#
#   kill-shim.sh install <dir>   write <dir>/kill (idempotent)
set -euo pipefail
if [ "${1:-}" = "install" ]; then
  dir="${2:?dir}"
  mkdir -p "$dir"
  cat > "$dir/kill" <<'SHIM'
#!/usr/bin/env bash
# qa-020 stopgap: a negative number after the signal is a process group,
# never "everyone". `--` is inserted once, before the first such argument.
real=/bin/kill
[ -x "$real" ] || real=/usr/bin/kill
args=()
i=0
done_dd=0
for a in "$@"; do
  if [ "$a" = "--" ]; then done_dd=1; fi
  if [ $done_dd -eq 0 ] && [ $i -gt 0 ] && [[ "$a" =~ ^-[0-9]+$ ]]; then
    args+=("--"); done_dd=1
  fi
  args+=("$a"); i=$((i+1))
done
exec "$real" "${args[@]}"
SHIM
  chmod 755 "$dir/kill"
  exit 0
fi
echo "usage: kill-shim.sh install <dir>" >&2; exit 2
