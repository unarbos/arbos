# Phase 0: baseline

Base commit: `2865cb29b806121d4a90f55ba6ce151a150279a0` (upstream
`main`, fetched 2026-11-09). This phase changes documentation only. Nothing was
merged or deployed. The working tree is independent of the contributor's other
two checkouts.

The inventory covers 12 Rust workspace crates and 773 source files: 409 Rust,
199 Swift, 78 Python, 77 shell, 6 TypeScript and 4 JavaScript files, approximately
513,000 source lines. Generated Swift and auxiliary JavaScript files are not all
standalone compilation units.

## Before and after

There are no implementation changes, so the measured baseline is also the after
state. Durations are one local invocation, not performance benchmarks. Counts
refer to gates, files or tests as identified; these units are not interchangeable.

| Check | Before | After | Duration | Measured counts |
| --- | --- | --- | ---: | --- |
| `cargo fmt --all --check` | pass | unchanged | 3.016 s | 1 gate passed |
| `cargo metadata --no-deps --format-version 1 --locked --offline` | pass | unchanged | 0.013 s | all 12 workspace crates |
| Full offline Cargo dependency resolution | pass | unchanged | 1.107 s | 1 gate passed |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | blocked | blocked | not run | unmeasured |
| `cargo check --workspace --all-targets --locked` | blocked | blocked | not run | unmeasured |
| `cargo test --workspace --locked` | blocked | blocked | not run | unmeasured |
| `cargo build --workspace --locked` | blocked | blocked | not run | unmeasured |
| `scripts/avatar-lint.sh` | pass | unchanged | 0.084 s | 1 gate passed |
| `scripts/glass-lint.sh` | pass | unchanged | 2.590 s | 1 gate passed; 76 existing unjudged sites |
| `apps/ios/scripts/control-size-lint.sh` | pass | unchanged | 0.065 s | 1 gate passed |
| `pnpm -r typecheck` in `harness` | pass after dependency setup | unchanged | 4.548 s | 1 gate passed |
| `pnpm -r build` in `harness` | pass after dependency setup | unchanged | 3.718 s | 1 gate passed |
| `pnpm -r test` in `harness` | pass after dependency setup | unchanged | 2.797 s | 14 passed, 0 failed, 0 skipped |
| Voice pytest collection | prerequisite failure | unchanged | 6.386 s | 0 tests run, 2 collection errors; missing `pytest_asyncio` and `aiohttp` |
| Python syntax inspection | pass | unchanged | 0.126 s | 78 files passed, 0 failed, 0 skipped |
| Standalone JavaScript syntax inspection | pass | unchanged | 0.067 s | 3 files passed, 0 failed, 0 skipped |
| Swift source parsing | pass | unchanged | 3.092 s | 192 app files parsed; not a typecheck or build |
| iOS typecheck, tests and build | blocked | blocked | not run | generated FFI artifacts absent; native build space unavailable |
| shellcheck / actionlint | missing tools | unchanged | not run | unmeasured; no installer executed |
| `scripts/merge/selftest.sh` | existing macOS Bash parse failure | unchanged | 34.659 s | 57 pass markers before exit 2 at line 350 |
| Busy-reply fake-relay unittest | pass | unchanged | 46.103 s | 2 passed, 0 failed, 0 skipped |

The initial harness invocations failed because dependencies were absent (typecheck
2.038 s, build 0.311 s, tests 0/2 files in 2.745 s). An inspected, frozen-lockfile
install with lifecycle scripts disabled made all three gates runnable. This is
environment preparation, with no source or lockfile change. Initial failure logs
and successful reruns are retained locally. Voice collection still lacks its
documented Python dependencies; they include the heavier LiveKit runtime.

The host had 2.84 GiB free. CI documents a roughly 19 GiB Rust clippy/test peak
and a 25 GiB free-space floor (`.github/workflows/ci.yml`). A fresh isolated native
build cannot fit safely. No other checkout's target directory was reused, and no
user files were deleted. Missing dependencies and blocked checks are not source
regressions or passing tests. Phase 0 deliberately fixes none of them.

## Run locally

Read `README.md`, `docs/self-host.md` and `docs/crdt-sync.md` for the supported
flows. Rust stable, Node >=22.19 and pnpm 9 are required. On Linux the documented
system packages are in `scripts/workspace-test-deps.sh`; inspect it before use.

For a device-only offline smoke test after building, use
`ARBOS_RELAY_URL=fixture cargo run -p arbos-mac`. Do not launch the bare default
command during this audit: its compiled-in relay is the hosted service.

For a local relay use
`cargo run -p arbos-relay -- --listen 127.0.0.1:7700 --data ./.arbos-local/relay`.
Its SQLite database and blobs live under that new data directory. Point each
device and CLI explicitly at the loopback relay; keep CLI configuration isolated
with `ARBOSCTL_CONFIG`. The documented process-box setup additionally needs
`arbos-agentd`, the pi extension and a local pi installation. Actual model turns
need a provider and incur external calls, so they were not run here.

SQLite tests can use temporary files or `:memory:` without launching any service.
Never point a test at an existing user database. Matrix-backed flows additionally
need a separately configured local homeserver; no hosted Matrix endpoint was
contacted. iOS needs the branch-matching Rust XCFramework and generated Swift
bindings from `scripts/build-xcframework.sh`; copying artifacts from a checkout
with different Rust code is not valid.

## Needs the owner

Provide at least 30 GiB of local scratch space (more for full iOS artifacts), then
rerun all blocked native gates with worktree-owned build directories. Recommended
default: free or attach local storage; leave existing contributor worktrees alone.
