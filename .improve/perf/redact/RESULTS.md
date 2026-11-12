# Shared redaction performance result

Actual `crates/arbos-core/src/redact.rs` now compiles its ordered patterns once with three standard-library `LazyLock` sets. Both key anchoring sets still derive from the unchanged `KEY_PREFIXES` table. Invalid patterns still skip, rather than introducing a startup panic. No patterns, replacements, ordering, boundaries, classification heuristics or JSON traversal rules changed.

| Warm workload | Before median (µs/call) | After median (µs/call) | Speedup | Before range (µs) | After range (µs) |
| --- | ---: | ---: | ---: | --- | --- |
| `redact_patterns` | 50.237 | 11.970 | 4.20× | 49.746–52.658 | 11.845–12.125 |
| `find_key_like` | 30.369 | 1.065 | 28.52× | 29.996–40.522 | 1.056–1.094 |
| `redact_placeholders` | 5.758 | 1.927 | 2.99× | 5.673–7.477 | 1.897–1.956 |
| `scrub_value` | 111.792 | 26.989 | 4.14× | 110.643–117.933 | 26.810–27.287 |

21 paired process runs, randomized before/after order with seed 20261109, 2000 calls per workload/sample, 109 deterministic cases. Every workload improves in all 21 pairs (two-sided paired sign-test p = 9.54e-7). Seeded 2000-resample percentile-bootstrap 95% intervals for paired median savings are:

| Workload | Saved µs/call, 95% interval |
| --- | ---: |
| `redact_patterns` | 38.005–38.379 |
| `find_key_like` | 29.151–29.375 |
| `redact_placeholders` | 3.801–3.883 |
| `scrub_value` | 84.128–85.355 |

Hardware/toolchain: Apple M5 Pro, 18 logical CPUs, 48 GiB RAM; macOS 26.6 arm64; Homebrew rustc 1.98.0. Release profile, two build jobs; timings measured with other agents’ builds/benchmarks paused. Baseline was captured before source editing. First-use compilation is outside warm samples; one initial scrub+classification observation per version is retained in `RESULTS.json`, insufficient to claim a cold-start improvement.

`scrub_value` includes cloning a nested JSON fixture with two text leaves and scalar values. The before/after behavioral fingerprint matches for all fixture outputs, classifier metadata/byte ranges and JSON projections (`9ec42eeb1a212e90`); this is fixture equivalence, not proof for all possible inputs. The expected-output tests cover every key family, false positives, bearer/env/placeholder replacement and nested JSON behavior.

| Leaf gate | Before | After |
| --- | --- | --- |
| Format | pass | pass |
| Clippy all targets, warnings denied | pass | pass |
| Typecheck all targets | pass | pass |
| Tests | 9 pass / 0 fail / 0 ignored | 10 pass / 0 fail / 0 ignored |
| Build | pass | pass |
| Optimized benchmark build | pass | pass |

The added expected-output regression exercises eight concurrent readers, Unicode token byte ranges, classification and nested-value scrubbing with JSON keys/scalars preserved. Existing nine tests were run unchanged before editing. All checks compile the actual source module in the retained isolated crate, offline with lockfile-pinned cached dependencies. Exact gate durations and raw paired samples are retained in `RESULTS.json`; compiler durations include different cache states and are not speedup evidence.

New isolated target artifacts total 66,506,742 bytes (63.4 MiB), below the 200 MiB budget.

Tradeoff: 39 compiled regular expressions remain allocated for the process lifetime after first use, replacing per-call compilation. This microbenchmark supports reducing shared redaction/classification CPU work. It does not establish user-visible chat open/send latency. Full workspace/native/iOS builds remain blocked by local disk and fork CI approval. No auth, signing, grant, money or deployment behavior was changed.
