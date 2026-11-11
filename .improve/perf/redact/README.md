# Shared redaction microbenchmark

This small standalone Cargo workspace compiles the **actual** repository
`crates/arbos-core/src/redact.rs` as its library. It does not copy the
implementation or build the native app, Matrix SDK, WebRTC or FFI. The exact
regex-lite and serde_json versions match the root lockfile. No services,
credentials, user files or additional root dependencies are needed.

Run the retained module tests and checks:

```sh
cargo test --manifest-path .improve/perf/redact/Cargo.toml --locked --offline
cargo clippy --manifest-path .improve/perf/redact/Cargo.toml --locked --offline --all-targets -- -D warnings
```

The measurement script limits Cargo to two jobs and disables debug information
and incremental output. It uses cached dependencies in offline mode and owns
its `target/`; it never deletes or reuses another checkout's build output.

For a before/after comparison, capture the actual source before editing, apply
the change, then capture after. Keep the first captured executable; both are
built from the same retained benchmark and linked directly to the source being
measured. When comparing an existing patch, use a disposable checkout and put
the same harness there before applying the patch.

```sh
python3 .improve/perf/redact/measure.py capture before --iterations 2000 --rounds 15
# Apply the proposed source change in this disposable checkout.
python3 .improve/perf/redact/measure.py capture after --iterations 2000 --rounds 15
python3 .improve/perf/redact/measure.py compare --iterations 2000 --rounds 21
```

Each capture runs formatting, clippy, check, tests, build and the optimized
benchmark build. Raw commands, durations and samples go to ignored `results/`;
release executables stay in ignored `target/`. The comparison alternates the two
executables in seeded randomized order for each pair. Run while other builds
and benchmarks are idle. `nice` is used on Unix; CPU identity is read with sysctl
on macOS and platform metadata elsewhere.

There are 109 deterministic cases: plain log/prose/URL/path text, synthetic
examples of every supported key family, headers/assignments and placeholders,
with punctuation, Unicode, word, URL and newline boundaries. A noncryptographic
fingerprint compares actual before/after outputs and classification byte ranges
on every case; mismatches stop the comparison. It is a behavioral comparison,
not a claim that hashing proves equivalence for all inputs. The existing source
unit tests pin expected classifications, markers and false-positive behavior.

Timings are nanoseconds per call after warmup in a release build, with identical
workloads/iteration counts in both versions. `scrub_value` includes cloning a
JSON fixture containing two text leaves, nested structures and scalar values.
The initial scrub+classification cost is recorded separately; its regex
initialization is excluded from warm samples. Per workload, comparison reports
median/min/max, paired median savings, a seeded 2000-resample percentile
bootstrap 95% interval and a two-sided paired sign test. These summarize local
microbenchmark variation; they do not establish app open/send latency or a
production-wide speedup. Full workspace and platform integration gates remain
separate.
