# shellcheck shell=bash
# Sourced by merge.sh / merge-window.sh. opread <op-ref>: prints the secret to stdout for capture by the caller.
# op runs in a subshell holding only PATH, HOME and the service-account token (no secret ever on argv), and with
# no session bus, so it cannot autolaunch a dbus-daemon that keeps a copy of the environment.
# Cache off (coordinator 20:48Z): every read must reach 1Password so the account's 1,000/day budget is counted
# honestly; with --cache=false op neither starts a daemon nor uses one that is running. op's stderr (never a
# secret) goes to the file op_err_file names, so a refusal can be reported verbatim.

# op_err_file: where op's stderr goes. op writes the secret to STDOUT and only diagnostics to stderr, so this
# file never receives a secret — asserted in selftest.sh, which checks a stdout sentinel does not reach it.
#
# Resolved in the CALLER's shell, and that is the whole point. The subshells below unset every exported
# variable but PATH, HOME and the token, so a ${MERGE_KIT} expanded INSIDE one is already gone: the redirect
# was unconditionally /tmp/k/op-last.err for every kit however MERGE_KIT was set. On a host with no /tmp/k the
# redirect itself then failed, op never ran, and merge.sh reported a PAT read failure that was indistinguishable
# from a 1Password refusal — a setup gap wearing a credential refusal's face. A lane unblocked itself with
# `mkdir /tmp/k` on 2026-11-05.
#
# The answer is a plain shell variable: `compgen -e` lists exported names only, so the scrub cannot reach it.
# The fallback is a directory that always exists, so the redirect can never again be the thing that fails.
op_err_file() {
  local d=${MERGE_KIT:-/tmp/k}
  [ -d "$d" ] && [ -w "$d" ] || d=${TMPDIR:-/tmp}
  printf '%s/op-last.err' "$d"
}

# op_preflight: the credential prerequisites, to be checked BEFORE anything is judged. Prints what is missing
# and how to supply it; returns 1 if anything is. It says nothing about the token but whether it is present —
# never its value, never its length, never any part of it.
#
# This exists because an absent `op` BINARY used to surface as "PAT read failed ... exit 4", after a clean GO:
# on a fresh worker VM op is simply not installed while OP_SERVICE_ACCOUNT_TOKEN is present, so the kit ran
# every gate, reached GO, and then died with `exec: op: not found`. Nothing had been read, refused or
# rate-limited, and exit 4 is the code for a credential refusal. Reporting a missing tool that way is what
# sends a lane looking for a vault problem, or around the script altogether.
#
# It installs nothing, on purpose. Installing a credential tool is an operator's decision, not a merge
# script's, and a script that silently acquires its own credential tooling is a worse thing to audit.
op_preflight() {
  local miss=0
  if ! command -v op >/dev/null 2>&1; then
    echo "SETUP: the 1Password CLI (op) is not on PATH, so no credential can be read on this host."
    echo "  This is a MISSING TOOL, not a refusal: nothing has been read, denied or rate-limited, and the"
    echo "  service account is fine. Install op yourself — the kit deliberately does not — then re-run:"
    echo "    curl -sS -o /tmp/op.zip https://cache.agilebits.com/dist/1P/op2/pkg/v2.30.3/op_linux_amd64_v2.30.3.zip"
    echo "    unzip -o -q /tmp/op.zip -d /tmp/op.d op && sudo install -m 755 /tmp/op.d/op /usr/local/bin/op"
    echo "  Check it with 'op --version'. The other lanes are on 2.30.x."
    miss=1
  fi
  if [ -z "${OP_SERVICE_ACCOUNT_TOKEN:-}" ]; then
    echo "SETUP: OP_SERVICE_ACCOUNT_TOKEN is not in this environment, so op has no service account to act as."
    echo "  This is a MISSING ENVIRONMENT VARIABLE, not a refusal by 1Password."
    echo "  Export it into this shell from wherever the session keeps it. Do not pass it on a command line and"
    echo "  do not write it to a file: the kit hands it to op through the environment and nowhere else."
    miss=1
  fi
  return $miss
}

opread() {
  # Not exported, so the scrub below cannot see it. See op_err_file.
  local _operr; _operr=$(op_err_file)
  ( for v in $(compgen -e); do case $v in PATH|HOME|OP_SERVICE_ACCOUNT_TOKEN) ;; *) unset "$v" ;; esac; done
    export DBUS_SESSION_BUS_ADDRESS=disabled: OP_CACHE=false
    exec op --cache=false read "$1" 2>"$_operr" )
}
# oprl: prints the account read_write USED count, or "?" if the probe failed. The ratelimit query is free.
# The counter is account-wide, so a before/after difference also includes any other lane's requests in between.
oprl() {
  ( for v in $(compgen -e); do case $v in PATH|HOME|OP_SERVICE_ACCOUNT_TOKEN) ;; *) unset "$v" ;; esac; done
    export DBUS_SESSION_BUS_ADDRESS=disabled: OP_CACHE=false
    op --cache=false service-account ratelimit 2>/dev/null | awk '$1=="account"&&$2=="read_write"{u=$4} END{print (u==""?"?":u)}' )
}
opdoc() {
  local _operr; _operr=$(op_err_file)
  ( for v in $(compgen -e); do case $v in PATH|HOME|OP_SERVICE_ACCOUNT_TOKEN) ;; *) unset "$v" ;; esac; done
    export DBUS_SESSION_BUS_ADDRESS=disabled: OP_CACHE=false
    exec op --cache=false document get "$1" --vault Arbos 2>"$_operr" )
}
