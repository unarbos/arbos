# Rerun the fixed-policy cache comparison

`driver.rs` is the Phase 6 portable benchmark driver plus 21 minimum nonword-tail
fixtures and an `--outputs` mode. The original 109 cases remain unchanged. No
redaction implementation is embedded in the driver.

To generate ignored snapshots in a fresh private temporary directory, set `bundle`
to the public directory containing this driver/runner and the two small manifests,
and `resolution_patch` to the published Phase 6→8 canonical redaction patch. From
the repository root:

```sh
repo="$(git rev-parse --show-toplevel)"
bundle="$repo/.improve/integration/bench"
resolution_patch="$repo/.improve/integration/redaction-after-performance.patch"
scratch="$(mktemp -d)"
cp -R "$bundle/." "$scratch/"
mkdir -p "$scratch/phase6/crates/arbos-core/src"
git -C "$repo" show e978f28d145d8674cbd5de5b1ecb02f3cc2f5d2d:crates/arbos-core/src/redact.rs > "$scratch/fixed-uncached.rs"
git -C "$repo" show 373c94b021e70ca0d1ae5049cd0e720b98aa103a:crates/arbos-core/src/redact.rs > "$scratch/phase6/crates/arbos-core/src/redact.rs"
patch -p1 -d "$scratch/phase6" < "$resolution_patch"
cp "$scratch/phase6/crates/arbos-core/src/redact.rs" "$scratch/fixed-cached.rs"
CARGO_TARGET_DIR="$scratch/target" cargo build --manifest-path "$scratch/uncached/Cargo.toml" --release --locked --offline --jobs 2 --bin redact-bench
cp "$scratch/target/release/redact-bench" "$scratch/uncached-redact-bench"
CARGO_TARGET_DIR="$scratch/target" cargo build --manifest-path "$scratch/cached/Cargo.toml" --release --locked --offline --jobs 2 --bin redact-bench
cp "$scratch/target/release/redact-bench" "$scratch/cached-redact-bench"
"$scratch/uncached-redact-bench" --outputs > "$scratch/uncached-outputs.json"
"$scratch/cached-redact-bench" --outputs > "$scratch/cached-outputs.json"
cmp "$scratch/uncached-outputs.json" "$scratch/cached-outputs.json"
python3 "$scratch/measure.py"
```

Verify the source hashes below before timing. `patch` operates only in that fresh
temporary source tree; the current checkout is not edited. The small manifests
expect `../fixed-uncached.rs` and `../fixed-cached.rs` respectively and share
`../driver.rs`. Generated snapshots, executables, output dumps and targets are
ignored; public artifacts contain no copied redaction implementation.

Build two isolated standalone Cargo crates with the same `driver.rs` as their
`redact-bench` binary. Use the Phase 6 portable manifest/lock as the template:
`improve/performance` commit `373c94b021e70ca0d1ae5049cd0e720b98aa103a`,
`.improve/perf/redact/{Cargo.toml,Cargo.lock}`. Set each `[lib].path` to its real
source input and each binary path to the shared driver:

- Uncached: redaction source at Phase 8 commit
  `e978f28d145d8674cbd5de5b1ecb02f3cc2f5d2d`, SHA256
  `51053e0dd677a570466b66431d8863519dbd9628175f8556cfd0589db4e8117c`.
- Cached: Phase 6 source with the reviewed Phase 6→8 resolution patch applied,
  canonical SHA256
  `1919913639b72ba43ffcff9a51bd29fe3038f08701de5e390463613b2dbde3da`.

Run `cargo build --manifest-path /path/to/manifest --release --locked --offline
--jobs 2 --bin redact-bench` for each. Copy the executables beside `measure.py` as
`uncached-redact-bench` and `cached-redact-bench`. Use an owned isolated target;
neither source nor binary belongs in a shared user's build directory.

Before timing, run each binary with `--outputs` and save its stdout beside the
runner as `uncached-outputs.json` / `cached-outputs.json`. The files must be exactly
equal. Expected current SHA256 is
`289dc0cc6387f7a1db7538dee6390611d4fe8b110c47ea3941f104f65eb8b6e5`.

Reserve a quiet team CPU window, then run `python3 measure.py`. Paths are relative
to the runner. It checks output equality and all runtime fingerprints, records
21 seeded randomized pairs of 2,000 calls, and writes `RESULTS.json` plus raw
observations to `raw-pairs.json`. Both policies include the boundary fix. The
reference run took 13.821 s; a slow host may exceed the runner's bounded window
and must report that limit rather than a complete comparison.

Public review artifacts need only this README, the driver/runner, results, raw
pairs and source/output hashes. Exclude binaries, package trees, full source copies
and the optional large synthetic output dumps.

The public runner only adds a non-Darwin CPU metadata fallback; its workload,
seed, pair order, timing and statistical calculations are unchanged.
