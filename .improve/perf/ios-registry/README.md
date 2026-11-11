# Bounded iOS registry hydration

`RelayAPI.registry()` fetched every project detail serially. It now keeps up to
four detail GETs in flight and folds typed outcomes in input order. It preserves
the list-only archive flag, malformed-row/skipped/incomplete distinction, duplicate
rows, 403/404 omission, actual Project/Chat mapping, and the first fatal error by
input position. A detail gets its own JSONDecoder. Cancellation stops admission,
cancels children and returns no registry result. Already-admitted GETs finish;
there can be a bounded number of speculative GETs beyond an error. A request
already dispatched to the relay cannot be recalled.

From the repository root, on macOS with Apple Swift and Python 3:

```sh
python3 -B .improve/perf/ios-registry/run.py --out /tmp/arbos-registry-check --check-only
python3 -B .improve/perf/ios-registry/run.py --out /tmp/arbos-registry-profile
```

Output directories must be new. `--tree PATH` checks another source tree. A full
run compiles both variants with `-O -swift-version 5`, runs nine source-derived
correctness cases for each, verifies exact complete typed output dumps, then runs
21 seeded randomized pairs. Run timing without competing compiles/tests. `--samples`
can select 21, 41 or 101; this does not turn the fixture into a production workload.
The old registry implementation is retained verbatim in `registry-before.swift`,
with its original full-source hash. The public wrapper reproduced both fixture
hashes exactly: nine before cases pass in 0.202 seconds and nine after cases pass
in 0.264 seconds (compile 9.737/9.530 seconds). Earlier direct-source baseline and
candidate gates also passed all nine. An initial synthetic MachineDto omitted
required fields; that setup error was corrected before the candidate was edited.

The fixture compiles actual generated RelayTypes and actual Project/Chat DTO
mappers, including defaults, picture URL, machine state, talking model/effort,
archive and chat parts. Only the asynchronous JSON transport edge is synthetic.
Cases cover ordered outputs/request multiset, typed mapped fields, list-only
archive, malformed list/detail parsing, 403/404 omission, duplicate rows, a slower
lower-index fatal error, pre-cancellation, and cancellation without an active
request leak. Executables run with all network denied. No server, account, FFI
binary, app, device or database is used. The fresh compilation prevents reuse of
an executable from another source snapshot.

## Retained local profile

Each fixture has twelve detail requests, a requested 8 ms asynchronous delay per
detail, an immediate list response and the same synthetic payload. OS timer
coalescing/scheduling affects the absolute duration. Both versions execute thirteen
GETs, preserve byte-identical normal/mixed typed output dumps, and finish with
zero active work. All samples show peak detail concurrency 1 → 4.

| Measurement | Serial | Bounded four |
|---|---:|---:|
| p50 elapsed | 129.691 ms | 33.651 ms |
| sample p95 | 135.021 ms | 37.482 ms |
| sample p99 | 138.943 ms | 39.395 ms |
| median process maximum RSS | 10,223,616 bytes | 10,403,840 bytes |

The p50 reduction is 74.05% (3.854×). Mean paired savings are 96.312 ms;
a 10,000-resample seeded bootstrap gives 94.642–98.029 ms for its 95% interval.
The two-sided exact paired sign test is 9.54e-7 (21 positive nonzero differences).
The entire timing batch took 4.134 seconds. These describe this controlled async
fixture, **not** app-open time, real HTTP latency, production percentiles or a
performance guarantee. Twenty-one samples give only a coarse sample tail.

RSS is the macOS `time -l` whole-process maximum, not allocation attribution or a
real-device memory profile. Median RSS rose by 180,224 bytes (176 KiB) here. Four
simultaneous real detail responses can require more transient payload/decoder
memory than one; this small synthetic payload cannot establish that ceiling.
No assumption is made that Foundation avoids copies. Response/output collections
remain proportional to the registry, while network/decoder work is capped at four.

## Scope and composition

`RESULTS.json` retains all pairs, source/fixture/output hashes, timing units,
randomization/statistics seeds and RSS counters. `BEFORE.json` / `AFTER.json`
retain the public correctness cases and mapped snapshots. In the composed ownership/performance tree, JSON methods use the account-bound
request admission supplied by the separate ownership fix. This independent
performance branch retains upstream authorization behavior.
Only registry types/methods change in this performance patch; composing it must
preserve that authorization helper and the raw upload/read/transcription fence.
A full real-SDK app compile is separate from these SDK-free method checks.

The directory fallback is deliberately unchanged: actual `loadDirectory` loops
the literal `abcdefghijklmnopqrstuvwxyz0123456789`, exactly 36 concurrent
queries after `/users` fails. It is a hard product bound, not fixture-only or a
dynamic project count. A four-slot queue would add up to nine request rounds;
no measured latency benefit justifies that change. Server result sizes still
need their existing server contract; 36 requests does not mean 36 bounded rows.
