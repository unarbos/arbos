# Phase 9: free CI fixes

The Phase 2 audit measured 150 runs and 526 jobs. This patch implements three
free scheduling/build changes, plus test isolation and an explicit improve/** push
filter so contributor-fork CI can run. No tests, job identities, runner classes
or deployment conditions are removed.

| Change | Historical cost / expected effect | Verified behavior |
| --- | --- | --- |
| Move `cargo fmt --all --check` immediately after the Rust toolchain step | Failing-format runs can stop before cache restore, Linux libraries and clang setup. The audit's total setup median was125s; earlier reclaim/checkout/toolchain work remains, so125s is not a measured saving. Green runs keep the same formatting work. | Exactly one format step; compiler components already installed; later Rust steps unchanged |
| Remove `pnpm -r build` after typecheck | Both package scripts are exactly `tsc --noEmit`; historical duplicate invocation median6s. | Same strict typecheck and all tests retained. Restore a separate build gate if the package starts emitting artifacts; comment records this boundary |
| Run push-only image lint independently | It previously waited for Rust and harness although it consumes neither. Audit estimates median14s workflow-tail reduction on relevant pushes, not a724s total-workflow saving. | Same push-only `if`, runner, timeout, checkout and hadolint configuration; only `needs` removed |

These are estimates from historical runs. Upstream PR CI requires maintainer approval; the public contributor fork can
run its own standard runners. Post-change actual run outcomes are pending and
are not represented as green. Paid runners and reduced tests are not justified by the measured
2s median Rust queue wait and are not introduced.

## Verification

A structural YAML comparison confirms all original top-level triggers retained plus improve/** pushes, and unchanged job
IDs/names; unchanged Rust steps apart from formatting order; unchanged harness
steps apart from the identical build alias; and unchanged image configuration
apart from its dependency. The package script equality was checked explicitly.

Actionlint1.7.12 was fetched from its [official GitHub release](https://github.com/rhysd/actionlint/releases/tag/v1.7.12), both downloaded
assets checked against GitHub SHA256 digests, the archive checked against release
checksums, and safe archive entries inspected before extracting only the local
arm64 binary. No installer, global binary, production service or account change
was used. The binary is not committed.

| Gate | Before | After |
| --- | --- | --- |
| `ci.yml` actionlint | pass,0diagnostics,.007847s | pass,0diagnostics,.031672s |
| All8workflow actionlint | pass,0diagnostics,.008362s | pass,0diagnostics,.009070s |
| YAML semantic invariants | original source snapshot | pass; changes constrained as above |
| Whitespace diff check | pass | pass,.045548s |

`-shellcheck=` explicitly disables only an unavailable external shellcheck tool;
pyflakes is also unavailable. These external lints are not represented as run.
The native actionlint checks remain active, with no ignore rules. Local gate
wall times are not speedup measurements. Baseline application/test reruns and
combined scoped validation are recorded in REPORT.md.

## Test isolation and new regression gates

[CI isolation](ci-isolation.md) records the earlier automatic Swift health-probe
reachability finding, cancellations and unknown prior contact. The test lane now
uses build-for-testing, patches/verifies all test-host/target-app relay URLs in
the generated plan, then executes the unchanged full plan. The actual helper and
Fastfile regressions are12groups: original13failing subcase assertions,2.146s;
fixed12groups pass,2.600s, plus Ruby syntax. Public static image fetching may
remain: this is a relay endpoint boundary, not a Swift no-egress sandbox.

The Linux full test command is compiled first and runs only after entering a fresh
loopback-only namespace with nonroot identity and no capabilities. Wrapper mock
contracts and a real-kernel self-hosted TCP/route/privilege smoke gate protect
that boundary; actual CI outcomes will be recorded after the first matching push.
There is no unfenced fallback, paid-runner change or test reduction.
