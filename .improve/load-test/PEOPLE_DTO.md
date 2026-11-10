# Whole chat-list profile and query work

The request-local roster read model loads only the public project projection,
requested user cards/device presence and the viewer's directed blocks. Live/banned
membership ordering and visibility stay with their existing ACL checks and one
shared DTO assembler. There is no cross-request profile cache. Legacy device-name
fallback, id-first/name-fallback project resolution, duplicate JSON-key semantics,
nullable fields and independent profile/presence decode failures are preserved.
Models retain their existing serde parse policy; no SQL JSON-path reinterpretation
or shared schema application occurs. The new live-device user-id index excludes
revoked devices, preserves tied-device row order, and leaves the frequently
updated presence timestamp outside the index key.

An8-room real handler regression originally used186/714 statements for2/24humans;
now both use69. The device lookup originally scanned5002 unrelated rows in a local
2500-device fixture; its bounded regression passes with the index and retains the
first tied legacy device name. A reviewer caught two candidate partial-row error
regressions; both fail before the narrow decode amendment and pass after. These
are candidate preservation defects, not additional original app defects.
Nine native cases and all-target Clippy with warnings denied pass for this
component; see PEOPLE_DTO_GATES.json. The later room-activity index adds a tenth
case, recorded separately in ACTIVITY_NATIVE_GATES.json. The portable suite
passed 21 randomized pairs at each scale with complete JSON byte equality.

| Groups | Whole handler p50 before → after | SQLite statements | Logical visits |
| --- | ---: | ---: | ---: |
| 1 | 0.945 → 0.749 ms | 96 → 13 | 868 → 195 |
| 10 | 6.360 → 3.645 ms | 942 → 85 | 8,672 → 970 |
| 100 | 72.531 → 31.978 ms | 9,382 → 805 | 604,715 → 9,498 |
| 1,000 | 1,832.479 → 321.528 ms | 93,822 → 8,005 | 57,962,963 → 94,778 |

All four paired saving intervals are positive; exact sign-test p=9.54e-7 per
scale (four-comparison Bonferroni p≤3.82e-6). The self-contained chart is
[people-dto.html](people-dto.html); full raw samples, hashes and plans are in
PEOPLE_DTO_RESULTS.json. These published whole-handler timings pin d499090b →
b96ef1f90, before the additional history/activity index. That index is measured
separately in ACTIVITY_RESULTS.json and is not folded into these numbers.

```sh
python3 .improve/load-test/people_dto.py --after b96ef1f90 --output /tmp/arbos-people-profile-1
```

Python3, Rust, a C compiler, cached locked Cargo dependencies and macOS Seatbelt or
Linux unprivileged network namespaces are required. Network isolation fails closed.
The output directory must be fresh. No DSN, account or service URL is accepted.
Source snapshots include only regular tracked blobs, excluding environment files.
Each arm has its own target, fresh HOME and generated relational SQLite files.
The optional dependency seed must be an inactive owned target; every workspace
package is cleaned in each clone. The driver records compilation and source hashes,
executes exactly one explicit profiling case per arm, verifies complete JSON output
hashes and runs21 randomized paired samples at1/10/100/1000rooms. Instrumentation
is disabled for timings. A separate read-only run uses cached root-lock SQLite with
scanstatus to capture real NVISIT and query plans from actual expanded SQL captures.

Timing covers the actual native handler and JSON body, with workspace opt0 and
dependencies opt2. It excludes transport, UI and production scheduling. Persistent
workers are warm; there is no CPU affinity or exclusive-host claim. Complete
responses grow with room count; this preserves the current full-list API. The
separate per-room metadata work is visible in counters, not claimed eliminated by
profile batching. No production migration or deployment is performed.
