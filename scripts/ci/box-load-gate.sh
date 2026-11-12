#!/usr/bin/env bash
# box-load-gate.sh
#
# The Phase 4 gate of docs/box-scaling.md §5: 1,000 users with a box each, 150 of the boxes in
# use, 30 sleeping boxes woken a minute, 2,500 client sockets, for ten minutes, and at the end
#
#   waitlist == 0, p95 Asleep-Clean wake < 1.0 s, p95 Asleep-Checkpointed wake < 2.0 s,
#   relay SQLite write lock wait p99 < 10 ms.
#
# It runs anywhere with cargo, on one machine: the relay is the real one (its database, its room
# sockets, its wake jobs, placement, the cap, the idle supervisor, the cold-tier pass) and every
# daemon is a fake inside the test that answers after as long as a real one takes. See
# crates/arbos-relay/src/load_gate.rs for what is modelled and how it is measured.
#
# Every number of the model is an ARBOS_LOAD_* variable (the run prints them all first):
#
#   ARBOS_LOAD_BOXES=1000  ARBOS_LOAD_SOCKETS=2500  ARBOS_LOAD_NODES=2  ARBOS_LOAD_NODE_CAP=160
#   ARBOS_LOAD_WARM=150  ARBOS_LOAD_WAKES_PER_MIN=30  ARBOS_LOAD_MINUTES=10
#   ARBOS_LOAD_CLEAN_WAKE_MS=200  ARBOS_LOAD_RESTORE_MS=220    a daemon's wake, measured
#   ARBOS_LOAD_SLEEP_MS=500  ARBOS_LOAD_CKPT_MS=1200  ARBOS_LOAD_FETCH_MS=8000   assumed
#   ARBOS_LOAD_THINK_MS=10000  ARBOS_LOAD_ROWS_PER_MIN=60  ARBOS_LOAD_IDLE_SPEEDUP=30  ARBOS_LOAD_SEED=1
#
# A quick look, a minute and a half:   ARBOS_LOAD_BOXES=60 ARBOS_LOAD_SOCKETS=150 \
#   ARBOS_LOAD_NODE_CAP=40 ARBOS_LOAD_WARM=12 ARBOS_LOAD_MINUTES=1 scripts/ci/box-load-gate.sh
#
# With real daemons the wake times are not a model. Measure them on the Linux host
# (scripts/ci/boxd-linux-test.sh prints a clean wake and a restore; on a host that runs the
# relay, `boxd_start_ms` in GET /projects/{id}/timings is the same wake seen from the relay),
# then run this with ARBOS_LOAD_CLEAN_WAKE_MS and ARBOS_LOAD_RESTORE_MS set to what was measured.
# The exit status is the gate's.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/../.."
# Both ends of every socket are in this process, and so are the fake daemons' connections.
want=$(( ${ARBOS_LOAD_SOCKETS:-2500} * 2 + 2048 ))
if (( $(ulimit -n) < want )); then
  ulimit -n "$want" || { echo "raise the open-file limit to $want (ulimit -n)" >&2; exit 2; }
fi
exec cargo test --release --locked -p arbos-relay the_thousand_user_load_gate -- --ignored --nocapture
