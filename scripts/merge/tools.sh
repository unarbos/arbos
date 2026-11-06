#!/bin/bash
# tools.sh — the merge kit's own tools, checked BY VERSION before anything is judged.
#
#   tools.sh check      name every tool gap and version drift; exit 1 if any tool is unusable
#   tools.sh repair     install the pinned op, after an operator asked for it in so many words
#   tools.sh baseline   print what a successful check recorded, and nothing else
#
# Sourced by merge.sh, which calls tools_preflight in the setup preflight, before the first gate.
#
# WHY VERSIONS AND NOT PRESENCE. op_preflight in opread.sh already answers "is op on PATH", and that
# question was not enough on 2026-11-06. A lane merged four pull requests with op resolving to
# /tmp/opbin/op at 2.30.0, an ad-hoc directory a previous shell had put on PATH by hand. The next
# shell did not inherit that PATH entry, `command -v op` found nothing, and the kit reported a
# missing tool — correctly, for the host it could see. The binary was never deleted and the host was
# never reimaged; the resolved PATH had changed under a running session. A presence check cannot
# tell those three apart, and they call for three different actions, so this one records the
# RESOLVED PATH and the VERSION of every tool it checks and keeps the last good reading on disk.
#
# WHAT THIS DOES NOT DO: it does not install anything by itself, and `check` never repairs. opread.sh
# states the reason for op and it holds for all three — "installing a credential tool is an
# operator's decision, not a merge script's, and a script that silently acquires its own credential
# tooling is a worse thing to audit". `repair` is a separate word an operator types, which is a
# different thing from a merge run quietly re-arming itself mid-queue.
set -u

K_T=${MERGE_KIT:-/tmp/k}
# Resolved from BASH_SOURCE and not from $0, because when merge.sh sources this file $0 is merge.sh
# and the repair hint would name the wrong script.
TOOLS_SELF=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/tools.sh

# The pinned op, and the honest provenance of its checksum. This sha256 is of the archive this kit
# actually installed from on 2026-11-06; it is NOT a hash published and attested by 1Password. It
# therefore proves only that a later download matches the one we ran, which is what catches a
# substituted or truncated archive, and is not a supply-chain attestation. Say so rather than let a
# reader assume the stronger claim.
OP_PIN_VERSION=2.30.3
OP_PIN_URL="https://cache.agilebits.com/dist/1P/op2/pkg/v${OP_PIN_VERSION}/op_linux_amd64_v${OP_PIN_VERSION}.zip"
OP_PIN_SHA256=a16307ebcecb40fd091d7a6ff4f0c380c3c0897c4f4616de2c5d285e57d5ee28

# Floors, and what a floor means here. Each is a version this kit has been RUN with and is known to
# work on, not a measured minimum: finding the true oldest workable release would mean testing old
# releases, which nobody has done. Below a floor is therefore UNKNOWN rather than proven broken, and
# is reported as drift to look at, not as a refusal. Above a floor is fine and is not reported.
OP_FLOOR=2.30.0
GH_FLOOR=2.103.0
GIT_FLOOR=2.43.0

# tool_version <tool>: the bare version string, or empty when it cannot be read.
tool_version() {
  case $1 in
    op)  op --version 2>/dev/null | tr -d '\r' ;;
    gh)  gh --version 2>/dev/null | sed -n 's/^gh version \([0-9.]*\).*/\1/p' | head -1 ;;
    git) git --version 2>/dev/null | sed -n 's/^git version \([0-9.]*\).*/\1/p' | head -1 ;;
  esac
}

# older_than <have> <floor>: true when <have> sorts below <floor>. sort -V, so 2.9 < 2.10.
older_than() { [ "$(printf '%s\n%s\n' "$1" "$2" | sort -V | head -1)" = "$1" ] && [ "$1" != "$2" ]; }

# host_facts: the readings that tell a reimage from a deletion from PATH drift. Read at the moment
# of the failure, BEFORE any repair, because a repair writes a new binary and a new mtime over the
# evidence. boot_id and uptime survive an install untouched; the state of the install directory does
# not, which is the part a late reading loses.
host_facts() {
  printf 'boot_id=%s\n' "$(cat /proc/sys/kernel/random/boot_id 2>/dev/null)"
  printf 'machine_id=%s\n' "$(cat /etc/machine-id 2>/dev/null)"
  printf 'uptime_s=%s\n' "$(cut -d' ' -f1 /proc/uptime 2>/dev/null)"
  printf 'container=%s\n' "$(sed -n '1s/.*/&/p' /proc/1/cgroup 2>/dev/null)"
}

baseline_file() {
  local d=$K_T
  [ -d "$d" ] && [ -w "$d" ] || d=${TMPDIR:-/tmp}
  printf '%s/tools-baseline' "$d"
}

# bf_field <file> <key>: the last value recorded for <key>, or empty. awk and not `sed | tail`, because the
# sandbox PATH the selftest runs this under carries a deliberately small tool set and `tail` is not in it —
# the selftest caught exactly that, with every diagnosis below collapsing into "unrecorded". substr rather
# than a field split, so a value that itself contains '=' survives.
bf_field() { awk -v k="$2" -F= '$1==k{v=substr($0, length(k)+2)} END{print v}' "$1" 2>/dev/null; }

# diagnose <tool>: why a tool that a previous run could use is unusable now. The three answers are
# distinct because they call for three different actions, and guessing between them is what sent a
# lane looking for a vault problem on 2026-11-06.
diagnose() {
  local tool=$1 bf had_path had_boot now_boot
  bf=$(baseline_file)
  [ -r "$bf" ] || { echo "      no baseline on this host, so there is nothing to compare against: this may be the kit's first run here."; return; }
  had_path=$(bf_field "$bf" "${tool}_path")
  had_boot=$(bf_field "$bf" boot_id)
  now_boot=$(cat /proc/sys/kernel/random/boot_id 2>/dev/null)
  echo "      last good reading: ${tool} ${had_path:-unrecorded} ($(bf_field "$bf" "${tool}_version")) at $(bf_field "$bf" recorded)"
  if [ -n "$had_boot" ] && [ "$had_boot" != "$now_boot" ]; then
    echo "      REIMAGE OR REBOOT: boot_id changed since that reading, so this is not the same running host."
    echo "      The tool was not deleted out from under anybody; the machine it lived on is gone. Reinstall and carry on."
  elif [ -n "$had_path" ] && [ -x "$had_path" ]; then
    echo "      PATH DRIFT: $had_path is still on disk and executable, but PATH no longer reaches it."
    echo "      Nothing was deleted and the host did not restart — this shell simply has a different PATH"
    echo "      from the one that recorded the baseline. Put the directory back on PATH, or install the"
    echo "      pinned version to a directory that is on PATH by default, which is what 'repair' does."
  elif [ -n "$had_path" ]; then
    echo "      DELETED: boot_id is unchanged, so this is the same running host, and $had_path is gone from it."
    echo "      Something removed it while the host stayed up. Worth knowing WHAT before reinstalling blindly."
  fi
  echo "      host now: $(host_facts | tr '\n' ' ')"
}

# tools_preflight: every gap and every drift in ONE pass, so one run names the whole problem instead
# of one layer of it per run. Returns 1 only when a tool is UNUSABLE; version drift below a floor is
# printed and does not refuse, because a floor here is an exercised version and not a proven minimum.
tools_preflight() {
  local miss=0 t v p rec="" bf
  for t in op gh git; do
    p=$(command -v "$t" 2>/dev/null)
    if [ -z "$p" ]; then
      echo "SETUP: $t is not on PATH, so the kit cannot run it on this host."
      echo "  This is a MISSING TOOL, not a refusal and not a verdict: nothing has been read or judged."
      diagnose "$t"
      [ "$t" = op ] && echo "      Install the pinned version with: $TOOLS_SELF repair"
      miss=1
      continue
    fi
    v=$(tool_version "$t")
    if [ -z "$v" ]; then
      echo "SETUP: $t is at $p but will not report a version, so the kit cannot tell what it is talking to."
      miss=1
      continue
    fi
    rec="$rec${t}_path=$p"$'\n'"${t}_version=$v"$'\n'
    local floor; case $t in op) floor=$OP_FLOOR ;; gh) floor=$GH_FLOOR ;; git) floor=$GIT_FLOOR ;; esac
    if older_than "$v" "$floor"; then
      echo "NOTE: $t is $v at $p, below the $floor this kit has been run with."
      echo "  Not a refusal: below an exercised version is UNKNOWN, not proven broken. Worth knowing before"
      echo "  a surprise, and worth naming in the report if this merge behaves oddly."
    else
      echo "    $t $v at $p"
    fi
  done
  [ $miss -eq 0 ] || return 1
  # Only a clean pass writes the baseline. A run that could not read a tool must not overwrite the
  # last reading that worked, because that reading is the only thing the next failure can compare to.
  bf=$(baseline_file)
  { printf 'recorded=%s\n' "$(date -u +%FT%TZ)"; printf '%s' "$rec"; host_facts; } > "$bf" 2>/dev/null || true
  return 0
}

tools_repair() {
  command -v curl >/dev/null 2>&1 && command -v unzip >/dev/null 2>&1 \
    || { echo "SETUP: repair needs curl and unzip, and this host is missing one of them."; return 1; }
  local d; d=$(mktemp -d)
  echo "==> downloading op $OP_PIN_VERSION"
  curl -sSfL -o "$d/op.zip" "$OP_PIN_URL" || { echo "download failed"; rm -rf "$d"; return 1; }
  local got; got=$(sha256sum "$d/op.zip" | cut -d' ' -f1)
  if [ "$got" != "$OP_PIN_SHA256" ]; then
    echo "REFUSED to install: sha256 of the archive is $got, not the pinned $OP_PIN_SHA256."
    echo "  Nothing was installed. See the provenance note at the top of this file for what this"
    echo "  checksum does and does not prove before you consider overriding it."
    rm -rf "$d"; return 1
  fi
  unzip -o -q "$d/op.zip" -d "$d" op || { echo "unzip failed"; rm -rf "$d"; return 1; }
  sudo install -m 755 "$d/op" /usr/local/bin/op || { echo "install failed"; rm -rf "$d"; return 1; }
  rm -rf "$d"
  echo "installed: op $(op --version 2>/dev/null) at $(command -v op)"
}

# Sourced for tools_preflight, or run directly for the three words above.
if [ "${BASH_SOURCE[0]}" = "$0" ]; then
  case "${1:-check}" in
    check)    tools_preflight ;;
    repair)   tools_repair ;;
    baseline) bf=$(baseline_file); [ -r "$bf" ] && cat "$bf" || { echo "no baseline at $bf"; exit 1; } ;;
    *) echo "usage: tools.sh <check|repair|baseline>"; exit 2 ;;
  esac
fi
