# Two uploads per send batch

The project and people attachment paths now share one small private batch helper.
It mints keys in input order, keeps at most **two PUT bodies per batch** in flight,
and returns tagged outcomes in input order. Concurrent sends have separate
batches; this is not a global application request limit. Callers retain their
existing final account checks, error classification and single commit. People
sends omit failed attachments; project sends keep their nil-blob placeholders.
Multiple/all failures, reply/forward/extra parts and the fixture path are preserved.
Errors are folded after the batch in input order, so a faster later failure does
not replace the original last-input error policy. Individual request cancellation
errors still count as per-item failures; cancellation of the batch task stops
admission, drains its children and returns no result. Keys for unscheduled items
can have been minted already; the opaque key format/generator are unchanged.

From the repository root on macOS, with Apple Swift and Python 3:

```sh
python3 -B .improve/perf/ios-uploads/run.py --out /tmp/arbos-upload-check --check-only
python3 -B .improve/perf/ios-uploads/run.py --out /tmp/arbos-upload-profile
```

The output directory must be new. `--tree PATH` checks another tree; `--samples`
selects 21 (default), 41 or 101 pairs per scenario. Keep timing free of competing
compiles/tests. The default command always runs eleven performance contracts for
each version, four new helper contracts, and two actual Foundation transport
smoke scenarios per version. The standalone main-based source reproduced this
complete `--check-only` command. `BEFORE.json`, `AFTER.json` and `CONTRACT.json`
retain those gates. Temporary executables use fresh compilation and all network
is denied at execution. No account, server, device, model, FFI library or database
is used. The before functions are verbatim historical serial implementations in
`serial-main.swift` and `serial-owned.swift`, with full-source provenance hashes.

For a tree composed with the separate Phase4 account fix, explicitly add the
ownership contracts:

```sh
python3 -B .improve/perf/ios-uploads/run.py --out /tmp/arbos-upload-owned --check-only --composition
```

This requires actual AccountWork/WorkLifetime sources and runs seventeen contracts
per version, including six cross-owner cases; it fails if that prerequisite is
absent. No tests are conditionally skipped. The default eleven-case performance
command makes no cross-account authorization claim. Context detection only supplies
the actual source helpers needed to compile each explicit scope. `COMPOSED.json`
retains seventeen passing source-controller checks against the guarded variant.
The original serial seventeen-case composition gate passed before edits (1.659 s);
the final direct-source seventeen-case gate passed in 1.084 s. A new helper check
caught an interim missing pre-cancel guard (one failure, three controls); the
corrected helper passes all four. These are correctness durations, not speedups.

## Retained local paired profile

`RESULTS.json` records 21 randomized pairs for each of three scenarios (63 pairs,
7.312 seconds total), using the guarded ownership composition. The selected
**actual** send branches call actual `RelayAPI.putBlob`, its atomic bearer helper
and bound client through Foundation URLSession. An intercept-all URLProtocol
returns synthetic replies after a requested 8 ms delay. It validates all four
body byte arrays, MIME types, captured synthetic bearer, row order and one commit.
No network packets or real services are involved. Every sample records four PUTs,
zero active work at completion and peak body concurrency 1 → 2.

| Four-attachment batch | Serial p50 | Two in flight p50 | Reduction | Median process max-RSS change |
|---|---:|---:|---:|---:|
| People, 64 bytes each | 47.410 ms | 24.994 ms | 47.28% | +160 KiB |
| Project, 1 MiB each | 47.849 ms | 25.722 ms | 46.24% | +144 KiB |
| Project, 8 MiB each | 52.828 ms | 28.079 ms | 46.85% | +144 KiB |

Mean paired savings and 95% seeded bootstrap intervals are respectively
22.783 ms [21.446, 24.027], 22.225 ms [20.692, 23.839], and
26.259 ms [23.059, 30.401]. All 21 differences per scenario are positive;
the two-sided exact sign-test p-value is 9.54e-7 in each. The JSON retains every
sample, coarse sample p95/p99/range, source/fixture hashes and statistics seeds.
The public generator reproduced the measured Swift fixture hashes exactly
(`37fcb8d1…` before, `1855c513…` after) and recompiled/ran both successfully.
An initial oracle setup omitted a method's actor annotation and an error stub
field; that compiler setup error was corrected before timing, without product edits.

These are **local synthetic asynchronous transport measurements**, not upload
bandwidth, production/app percentiles or a guarantee on a phone. Foundation and
OS timer coalescing affect their absolute duration. The oracle deliberately holds
copies of all bodies to verify bytes. Its median process maximum RSS was
12,255,232 → 12,419,072 bytes, 21,397,504 → 21,544,960 bytes, and
81,362,944 → 81,510,400 bytes. `time -l` reports whole-process maxima, not allocation
attribution. This does not prove that URLSession avoids copying already-loaded
Data. For the largest fixture, logical in-flight payload rises from 8 to 16 MiB;
real transient memory depends on body sizes, Foundation and concurrent batches.
The existing input/file policies are unchanged; the new limit counts requests,
not bytes. A real-device upper bound is not established by this oracle.

## Ownership and source composition

The API-only helper is identical in standalone and owned variants. On standalone
main, callers pass their captured API and retain baseline auth behavior. The
Phase4 composition must keep `work.api` and all caller owner guards; never replace
those guarded callers with the standalone main function. Real raw HTTP authorization
is separately pinned by the sixteen URLProtocol owner-boundary cases. These
controller fakes observe logical API admissions, not network dispatch. At account
retirement they snapshot the already-admitted A calls and require no additional
logical admissions, B authority or late private UI/commit. The bound raw API rejects
new HTTP admission after retirement, including any remaining helper attempts.
Previously issued HTTP effects cannot be recalled. The helper does not pretend
that account retirement automatically cancels its task group.

The original eleven Phase4 upload assertions remain intact in the separate
compatibility amendment. Its fake supports multiple pending requests, snapshots
admission at retirement and matches bytes/MIME by key rather than relying on
independent PUT start order. It extracts the actual batch helper and enum when
present; its case count remains eleven. The extra six performance cases and four
new helper contracts in this artifact are separately identified. No test removal
or reduced input margin is needed for concurrency. Whole real-SDK app compilation
is reported separately from these selected-source fixtures and actual local
Foundation transport; neither establishes physical device or full UI behavior.
