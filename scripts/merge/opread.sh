# shellcheck shell=bash
# Sourced by merge.sh / merge-window.sh. opread <op-ref>: prints the secret to stdout for capture by the caller.
# op runs in a subshell holding only PATH, HOME and the service-account token (no secret ever on argv), and with
# no session bus, so it cannot autolaunch a dbus-daemon that keeps a copy of the environment.
# Cache off (coordinator 20:48Z): every read must reach 1Password so the account's 1,000/day budget is counted
# honestly; with --cache=false op neither starts a daemon nor uses one that is running. op's stderr (never a
# secret) goes to $MERGE_KIT/op-last.err (default /tmp/k) so a refusal can be reported verbatim.
opread() {
  ( for v in $(compgen -e); do case $v in PATH|HOME|OP_SERVICE_ACCOUNT_TOKEN) ;; *) unset "$v" ;; esac; done
    export DBUS_SESSION_BUS_ADDRESS=disabled: OP_CACHE=false
    exec op --cache=false read "$1" 2>"${MERGE_KIT:-/tmp/k}/op-last.err" )
}
# oprl: prints the account read_write USED count, or "?" if the probe failed. The ratelimit query is free.
# The counter is account-wide, so a before/after difference also includes any other lane's requests in between.
oprl() {
  ( for v in $(compgen -e); do case $v in PATH|HOME|OP_SERVICE_ACCOUNT_TOKEN) ;; *) unset "$v" ;; esac; done
    export DBUS_SESSION_BUS_ADDRESS=disabled: OP_CACHE=false
    op --cache=false service-account ratelimit 2>/dev/null | awk '$1=="account"&&$2=="read_write"{u=$4} END{print (u==""?"?":u)}' )
}
opdoc() {
  ( for v in $(compgen -e); do case $v in PATH|HOME|OP_SERVICE_ACCOUNT_TOKEN) ;; *) unset "$v" ;; esac; done
    export DBUS_SESSION_BUS_ADDRESS=disabled: OP_CACHE=false
    exec op --cache=false document get "$1" --vault Arbos 2>"${MERGE_KIT:-/tmp/k}/op-last.err" )
}
