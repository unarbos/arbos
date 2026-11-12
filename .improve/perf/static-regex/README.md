# Cached fixed parser patterns

```sh
python3 .improve/perf/static-regex/check.py --out /tmp/arbos-parser-check --measure
```

Use a fresh output directory, cached Rust dependencies, and macOS or the
project's loopback-only Linux test namespace. Builds are locked/offline with
separate new per-arm targets. macOS denies network; this check executes no HTTP,
agent process, database, account storage or application code.

The driver extracts actual HTML conversion, tag stripping, entity decoding,
relative-link handling, loose JSON parsing and truncated-card parsing functions.
Search's two actual regex bindings run against three generated HTML fixtures;
the HTTP fallback is not executed. It compares 39 HTML, 22 card and three search
outputs to the frozen upstream base. Literal pattern inventories must match.
The closing-list-item fixture also catches replacing `</li>` with a different
pattern, a mechanical-edit error caught and corrected before this change shipped.

Seventeen fixed patterns now initialize once through standard-library LazyLock.
The five discarded-element passes retain their original order. Search retains
its Result/error-string behavior, truncated-card parsing its optional compile
behavior, and HTML its existing panic-on-invalid-constant behavior. Valid JSON
cards still return before initializing salvage patterns. User-supplied grep and
approval patterns remain input-dependent. No dependency or regex syntax changes.

`RESULTS.json` records 21 seeded randomized paired process samples with 50 warmup
calls. Each sample times 1000 short HTML conversions, 100 conversions of a generated
32-link page, and 2000 truncated-card parses. Medians:

| Warm workload | Before | After | Ratio |
| --- | ---: | ---: | ---: |
| Short HTML | 33.680 µs | 7.327 µs | 4.60× |
| 32-link HTML | 870.729 µs | 772.695 µs | 1.13× |
| Truncated memory card | 5.674 µs | 2.223 µs | 2.55× |

All paired savings are positive (two-sided sign-test p=9.54e-7 per workload);
paired median bootstrap 95% intervals are 25.998–26.529 µs, 89.450–98.769 µs and
3.425–3.555 µs respectively. These are local warm CPU timings on a shared
workstation, without cache flushing or CPU isolation. Initial regex compilation
is outside warm measurements. This is no network, hosted-agent or chat-latency
speedup claim. Full source hashes, raw samples and output fingerprint are retained.

The actual full agentd caller passes all-target Clippy with warnings denied
(66.538 s) in an isolated snapshot. Three existing native pure-parser tests
also pass with exact nonzero count guards: HTML conversion, entity decoding and
truncated-card parsing. The first test command includes 174.261 s of full codegen;
remaining commands take 0.789/0.259 s. NATIVE_GATES.json and NATIVE_TESTS.json
record those source hashes and commands. They are separate from full combined CI.
