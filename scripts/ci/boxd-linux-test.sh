#!/usr/bin/env bash
# boxd-linux-test.sh [platform]
#
# The Phase 1 gate of docs/box-scaling.md §5 against the real thing: loop devices, ext4,
# network namespaces and runsc. arbos-boxd's unit tests run on a fake host; this is the half
# they cannot reach.
#
# Run from anywhere with Docker. It starts one privileged rust container with the repo mounted
# read-only and runs itself inside it. Inside, it installs e2fsprogs and the official gVisor
# release (sha512 checked), builds arbos-boxd, and drives the daemon over HTTP through:
#
#   ensure -> fresh image under 35 MB on disk -> wake -> grow to 32 and 128 GiB while the box
#   writes -> a busy box refuses sleep -> sleep clean (no checkpoint) -> wake -> sleep with a
#   checkpoint -> wake restores it with the background process still counting -> two boxes
#   whose ids share a prefix stay two boxes.
#
# A busybox rootfs stands in for the arbos-agent image: its init answers prepare-sleep the way
# agentd does and keeps one background process counting on /work. So this proves the
# supervisor, not agentd; and the timings it prints are this machine's, not production's.
#
# `platform` is runsc's (default systrap; ptrace where systrap is refused). The build is cached
# in the Docker volume `arbos-boxd-test-cache`; remove it when done.
set -euo pipefail

if [[ "${BOXD_TEST_INSIDE:-}" != 1 ]]; then
  repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
  exec docker run --rm --privileged \
    -e BOXD_TEST_INSIDE=1 -e BOXD_PLATFORM="${1:-systrap}" \
    -v "$repo":/src:ro -v arbos-boxd-test-cache:/cache \
    rust:1-slim-bookworm bash /src/scripts/ci/boxd-linux-test.sh
fi

log() { echo "boxd-test: $*"; }
fail() { echo "boxd-test: FAILED: $*" >&2; exit 1; }
ms() { echo $(( ($(date +%s%N) - $1) / 1000000 )); }

# ---- what the daemon needs on a host ----------------------------------------------------------
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq --no-install-recommends \
  ca-certificates curl bzip2 jq e2fsprogs iproute2 busybox-static >/dev/null

# The release is one tarball: runsc and, beside it, the gvisor-bin/ it starts its sandboxes from.
release="https://storage.googleapis.com/gvisor/releases/release/latest/$(uname -m)"
mkdir -p /opt/gvisor && cd /opt/gvisor
curl -fsSLO "$release/gvisor.tar.bz2" -O "$release/gvisor.tar.bz2.sha512"
sha512sum -c gvisor.tar.bz2.sha512 >/dev/null || fail "gVisor does not match its published sha512"
tar -xjf gvisor.tar.bz2 && rm gvisor.tar.bz2
export PATH="/opt/gvisor:$PATH"
log "runsc $(runsc --version | sed -n 's/^runsc version //p'), platform $BOXD_PLATFORM"

# A container's /dev is a snapshot: the nodes of loop devices made later are not in it.
for i in $(seq 0 31); do [[ -e /dev/loop$i ]] || mknod /dev/loop$i b 7 "$i"; done
# cgroup v2 hands controllers to children only from a cgroup with no process of its own.
if [[ -f /sys/fs/cgroup/cgroup.controllers ]]; then
  mkdir -p /sys/fs/cgroup/init
  for pid in $(cat /sys/fs/cgroup/cgroup.procs); do
    echo "$pid" > /sys/fs/cgroup/init/cgroup.procs 2>/dev/null || true
  done
  sed 's/\</+/g' /sys/fs/cgroup/cgroup.controllers > /sys/fs/cgroup/cgroup.subtree_control
fi

cd /src
export CARGO_HOME=/cache/cargo CARGO_TARGET_DIR=/cache/target
cargo build --locked -p arbos-boxd 2>&1 | tail -2

# ---- the stand-in image -----------------------------------------------------------------------
root=/var/lib/arbos-boxd
image="$root/images/test"
rootfs="$(mktemp -d)"
mkdir -p "$image" "$rootfs"/{bin,etc,proc,dev,sys,tmp,run,mnt,work,home/daytona}
cp /bin/busybox "$rootfs/bin/busybox"
for applet in $(busybox --list | grep -vx busybox); do ln -s busybox "$rootfs/bin/$applet"; done
echo 'root:x:0:0:root:/root:/bin/sh' > "$rootfs/etc/passwd"
: > "$rootfs/etc/hosts"; : > "$rootfs/etc/resolv.conf"
# Counts on /work five times a second: the "user process" a checkpoint must keep.
cat > "$rootfs/tick" <<'EOF'
#!/bin/sh
i=0
while :; do i=$((i + 1)); echo "$$ $i" > /work/tick; sleep 0.2; done
EOF
# One prepare-sleep answer, shaped like agentd's. /work/busy and /work/pids steer it.
cat > "$rootfs/respond" <<'EOF'
#!/bin/sh
while IFS= read -r line; do [ "${#line}" -le 1 ] && break; done
if [ -e /work/busy ]; then
  code="409 Conflict"; body='{"ok":false,"reason":"test: a turn is running"}'
else
  code="200 OK"
  body="{\"ok\":true,\"active_user_pids\":$(cat /work/pids),\"files_bytes\":1,\"scratch_bytes\":2,\"trash_bytes\":3}"
fi
printf 'HTTP/1.1 %s\r\ncontent-type: application/json\r\ncontent-length: %s\r\nconnection: close\r\n\r\n%s' \
  "$code" "${#body}" "$body"
EOF
cat > "$rootfs/init" <<'EOF'
#!/bin/sh
trap 'echo term >> /work/signals; exit 0' TERM
trap 'echo usr1 >> /work/signals' USR1
[ -e /work/pids ] || echo 0 > /work/pids
/tick &
(while :; do nc -l -p 7788 -e /respond; done) &
# The box reaches itself on 127.0.0.1, as agentd's workspaces do: marked once it has.
(until wget -q -O /dev/null http://127.0.0.1:7788/; do sleep 0.2; done; echo ok > /work/loopback) &
while :; do wait; done
EOF
chmod +x "$rootfs"/{tick,respond,init}
tar -C "$rootfs" -cf "$image/rootfs.tar" .
echo '{"Entrypoint":["/init"],"Cmd":null,"Env":["PATH=/bin"],"WorkingDir":"/work"}' > "$image/image.json"

# ---- the daemon -------------------------------------------------------------------------------
mkdir -p /etc/arbos-boxd
(umask 077; head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n' > /etc/arbos-boxd/token)
ARBOS_BOXD_PLATFORM="$BOXD_PLATFORM" /cache/target/debug/arbos-boxd > /tmp/boxd.log 2>&1 &
trap 'echo "--- boxd.log"; tail -40 /tmp/boxd.log; tail -40 $root/boxes/*/box.log 2>/dev/null || true' ERR
for _ in $(seq 50); do curl -s -o /dev/null http://127.0.0.1:7790/ && break; sleep 0.1; done

user=test_01JBOXDTEST0000000000000000
box="$root/boxes/$user"
# api <want-status> <method> <path> [json]: the body on stdout.
api() {
  local out code
  out="$(curl -sS -w '\n%{http_code}' -X "$2" "http://127.0.0.1:7790$3" \
    -H "authorization: Bearer $(cat /etc/arbos-boxd/token)" \
    ${4:+-H 'content-type: application/json' -d "$4"})"
  code="${out##*$'\n'}"
  [[ "$code" == "$1" ]] || fail "$2 $3 answered $code, wanted $1: ${out%$'\n'*}"
  echo "${out%$'\n'*}"
}
# The box answers on its port: the stand-in's healthcheck.
healthy() {
  for _ in $(seq 200); do curl -s -m 1 -o /dev/null "$1/" && return 0; sleep 0.02; done
  fail "the box never answered on $1"
}
tick() { cat "$box/mnt/tick"; }
# attached [box-dir]: the box's disk is still on a loop device or still mounted.
attached() { [[ -n "$(losetup -j "${1:-$box}/work.img")" ]] || mountpoint -q "${1:-$box}/mnt"; }

[[ "$(curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:7790/v1/host/status)" == 401 ]] \
  || fail "status answered without a token"

log "ensure"
api 200 POST "/v1/boxes/$user/ensure" '{"version":"test","env":{"ARBOS_MCP_TOKEN":"box-token"}}' >/dev/null
kb="$(du -k "$box/work.img" | cut -f1)"
log "  fresh 8 GiB work.img: $((kb / 1024)) MB on disk"
(( kb < 35 * 1024 )) || fail "a fresh image takes $kb kB, over 35 MB"

log "wake (first boot: seeds /etc)"
t=$(date +%s%N)
url="$(api 200 POST "/v1/boxes/$user/wake" | jq -r .box_url)"
healthy "$url"; log "  healthy in $(ms "$t") ms"
[[ -f "$box/mnt/.etc/passwd" ]] || fail "/etc was not seeded"
for _ in $(seq 100); do [[ -f "$box/mnt/loopback" ]] && break; sleep 0.1; done
[[ -f "$box/mnt/loopback" ]] || fail "nothing in the box reaches 127.0.0.1: its loopback is down"
[[ "$(api 200 POST "/v1/boxes/$user/wake" | jq -r .restored)" == false ]] || fail "second wake"
cgroup="/sys/fs/cgroup/arbos-boxd/arbos-box-$user."
[[ "$(cat "$cgroup/memory.max")" == $((16 << 30)) && "$(cat "$cgroup/cpu.max")" == "800000 100000" ]] \
  || fail "the box does not run under its 16 GiB and 8 vCPU limits"

log "grow under write load"
dd if=/dev/urandom of="$box/mnt/load" bs=1M count=300 conv=fsync status=none &
load=$!
for want in 32 128; do
  before="$(tick)"
  t=$(date +%s%N)
  [[ "$(api 200 POST "/v1/boxes/$user/grow" | jq .size_gb)" == "$want" ]] || fail "grow to $want"
  log "  $want GiB in $(ms "$t") ms"
  gb="$(df -BG --output=size "$box/mnt" | tail -1 | tr -dc 0-9)"
  (( gb > want * 9 / 10 )) || fail "the filesystem is $gb GiB after growing to $want"
  sleep 0.5; [[ "$(tick)" != "$before" ]] || fail "the box stopped writing during the grow"
done
wait "$load"
api 409 POST "/v1/boxes/$user/grow" >/dev/null
log "  refused past 128 GiB; $(du -m "$box/work.img" | cut -f1) MB on disk"

log "a busy box is not put to sleep"
touch "$box/mnt/busy"
api 409 POST "/v1/boxes/$user/sleep" | jq -c .
before="$(tick)"; sleep 0.5; [[ "$(tick)" != "$before" ]] || fail "the busy box was stopped"
rm "$box/mnt/busy"

log "sleep clean"
read -r _ first_life < "$box/mnt/tick"
t=$(date +%s%N)
r="$(api 200 POST "/v1/boxes/$user/sleep")"; log "  $(ms "$t") ms: $r"
[[ "$(jq -r .state <<<"$r")" == asleep_clean ]] || fail "not a clean stop"
[[ ! -e "$box/ckpt" ]] || fail "a clean stop wrote a checkpoint"
attached && fail "the disk is still attached"
api 200 POST "/v1/boxes/$user/sleep" >/dev/null

log "wake from clean"
t=$(date +%s%N)
[[ "$(api 200 POST "/v1/boxes/$user/wake" | jq -r .restored)" == false ]] || fail "restored from nothing"
healthy "$url"; log "  healthy in $(ms "$t") ms"
grep -q term "$box/mnt/signals" || fail "the clean stop did not deliver SIGTERM"
sleep 1
read -r pid count < "$box/mnt/tick"
(( count < first_life )) || fail "the counter did not start over after a clean stop"

log "sleep with a user process"
echo 2 > "$box/mnt/pids"
t=$(date +%s%N)
r="$(api 200 POST "/v1/boxes/$user/sleep")"; log "  $(ms "$t") ms: $r"
[[ "$(jq -r .state <<<"$r")" == asleep_ckpt ]] || fail "not checkpointed"
(( $(jq .ckpt_bytes <<<"$r") > 0 )) || fail "the checkpoint is empty"
attached && fail "the disk is still attached"

log "wake restores it"
t=$(date +%s%N)
[[ "$(api 200 POST "/v1/boxes/$user/wake" | jq -r .restored)" == true ]] || fail "not restored"
healthy "$url"; log "  healthy in $(ms "$t") ms"
read -r pid2 count2 < "$box/mnt/tick"
sleep 1
read -r pid3 count3 < "$box/mnt/tick"
[[ "$pid2" == "$pid" && "$pid3" == "$pid" ]] || fail "the background pid changed: $pid -> $pid2"
(( count2 >= count && count3 > count2 )) || fail "the counter did not carry on: $count, $count2, $count3"
log "  background pid $pid intact, counter $count -> $count2 -> $count3"
grep -q usr1 "$box/mnt/signals" || fail "SIGUSR1 did not reach the box's init"
[[ ! -e "$box/ckpt" ]] || fail "the checkpoint was kept after the restore"

api 200 GET /v1/host/status | jq -c 'del(.boxes), .boxes[0]'
echo 0 > "$box/mnt/pids"
api 200 POST "/v1/boxes/$user/sleep" >/dev/null
[[ "$(api 200 GET /v1/host/status | jq .resident_boxes)" == 0 ]] || fail "a box is still resident"
# Asleep means nothing of the box is left on the host but its files.
attached && fail "the disk is still attached"
# Every box runs on the one unpacked rootfs, so a box must leave nothing in it.
diff <(tar -tf "$image/rootfs.tar" | sed 's|/$||' | sort) <(cd "$image/rootfs" && find . | sort) \
  || fail "a box changed the shared rootfs"

# runsc takes an id that is the start of exactly one sandbox's as that sandbox. Two users whose
# ids differ only by a tail must still be two boxes, whichever of them is running.
log "one user id is the start of another"
for u in pfx_a pfx_ab; do
  api 200 POST "/v1/boxes/$u/ensure" '{"version":"test","env":{"ARBOS_MCP_TOKEN":"box-token"}}' >/dev/null
done
resident() { api 200 GET /v1/host/status | jq .resident_boxes; }
url_ab="$(api 200 POST /v1/boxes/pfx_ab/wake | jq -r .box_url)"; healthy "$url_ab"
# Only the longer one runs: the shorter is not it, and waking it starts its own sandbox.
url_a="$(api 200 POST /v1/boxes/pfx_a/wake | jq -r .box_url)"; healthy "$url_a"
[[ "$url_a" != "$url_ab" && "$(resident)" == 2 ]] || fail "waking pfx_a did not start a second sandbox"
# Both run: sleeping the shorter stops that one and only that one.
[[ "$(api 200 POST /v1/boxes/pfx_a/sleep | jq -r .state)" == asleep_clean ]] || fail "pfx_a did not sleep"
attached "$root/boxes/pfx_a" && fail "pfx_a's disk is still attached"
[[ "$(resident)" == 1 ]] || fail "sleeping pfx_a left $(resident) boxes running"
curl -s -m 1 -o /dev/null "$url_a/" && fail "pfx_a still answers after its sleep"
healthy "$url_ab"
# Only the longer one runs again: a sleep of the shorter must not reach it.
api 200 POST /v1/boxes/pfx_a/sleep >/dev/null
healthy "$url_ab"
api 200 POST /v1/boxes/pfx_ab/sleep >/dev/null
[[ "$(resident)" == 0 ]] || fail "a box is still resident"
attached "$root/boxes/pfx_ab" && fail "pfx_ab's disk is still attached"

left="$(ip netns list; ip -br link | grep '^ab' || true)"
[[ -z "$left" ]] || fail "the box's network is still up: $left"
log "PASSED on $BOXD_PLATFORM"
