# Phase 2: CI audit (report only)

Sample: latest 150 runs of `.github/workflows/ci.yml`, captured with `gh` on
2026-11-09: 2026-11-07 05:59:12 UTC through 2026-11-09 23:58:59 UTC.
There were 79 pull-request and 71 push runs: 114 successful, 12 failed,
7 cancelled and 17 `action_required`. All 150 job requests succeeded; 526 jobs
were returned, 18 runs had no jobs. No workflow or runtime source changed.

## Timing method

Read the run and job APIs, never logs, secrets or operational services:

```sh
gh api 'repos/unarbos/arbos/actions/workflows/ci.yml/runs?per_page=100&page=1'
gh api 'repos/unarbos/arbos/actions/workflows/ci.yml/runs?per_page=100&page=2'
gh api 'repos/unarbos/arbos/actions/runs/<run-id>/jobs?per_page=100&filter=latest'
```

Retain the first 150 runs, latest attempts. Queue is **job** creation to start,
not run creation: dependency/concurrency waiting is separate. Invalid or missing
timestamps are unavailable. Step timestamps have one-second granularity. Setup
includes checkout, toolchains, installs, cache restore and container preparation;
task includes actual checks. Teardown and unallocated step gaps are shown
separately rather than hidden in either bucket. These are observational samples,
not controlled benchmarks or actual CPU profiles.

The five slowest completed jobs are all Rust. Times are seconds:

| Job link | Queue | Setup | Tasks | Teardown + gaps | Wall |
| --- | ---: | ---: | ---: | ---: | ---: |
| [42584303427 / 129060221682](https://github.com/unarbos/arbos/actions/runs/42584303427/job/129060221682) | unavailable | 199 | 615 | 4 | 818 |
| [42678963566 / 129368148942](https://github.com/unarbos/arbos/actions/runs/42678963566/job/129368148942) | 2 | 178 | 621 | 4 | 803 |
| [42610786866 / 129138367185](https://github.com/unarbos/arbos/actions/runs/42610786866/job/129138367185) | 2 | 202 | 598 | 3 | 803 |
| [42559727455 / 128973273430](https://github.com/unarbos/arbos/actions/runs/42559727455/job/128973273430) | 2 | 150 | 644 | 5 | 799 |
| [42695383679 / 129425733270](https://github.com/unarbos/arbos/actions/runs/42695383679/job/129425733270) | 2 | 157 | 620 | 5 | 782 |

The first job's API reports creation after completion; inventing a queue duration
from run creation would give a false number. Across successful jobs, Rust wall
median is 724.5 s (p95 775), setup 125 s, task 598.5 s and queue 2 s. Harness wall
median is 24.5 s; Layout 25 s; Dockerfile lint 11 s. Queue medians are 2 s.
Job medians need not sum because each is computed independently.

## Nine checks

| Check | Current measured cost / finding | File and proposed action | Expected saving |
| --- | --- | --- | --- |
| 1. Default runner queue | Successful Rust/harness/Layout/image queue median 2 s; usual p95 3 s. Maintainer approval holds are not runner queues. | Keep `ci.yml:61,118,354,377` hosted runners. Resolve fork approval separately. | No evidence that buying runners helps. |
| 2. Slow lint | Rust fmt median 6 s, clippy 61 s; Layout task 18 s. Fmt occurs after setup. | Move fmt from `ci.yml:328` immediately after toolchain setup. | Zero on a green run; roughly 125 s setup avoided before a formatting refusal. |
| 3. Slow typecheck | Harness typecheck 6 s, build 6 s; both scripts are `tsc --noEmit` (`harness/arbos-pi-ext/package.json:9,10`). | Remove the redundant CI invocation at `ci.yml:370,371`; retain typecheck and all tests. | Median 6 s per harness job (3–7 s in 132 successful samples); little overall critical-path impact. |
| 4. Unsharded tests / machine size | Rust `cargo test` step median 526.5 s, including compilation. No test-only timing, CPU or memory profile was collected. | Investigate `ci.yml:330` with per-crate/test timing before any `nextest`/matrix change. | Unquantified; do not pretend the entire step is parallelizable test time. |
| 5. Per-file whole-app reload | Two harness test files instantiate extension registrations; all harness tests together median 1 s. Native Swift tests are app-hosted; Rust tests share crate binaries. | Preserve `harness/arbos-pi-ext/*.test.ts` integration coverage. A pure module seam can reduce import coupling; no CI speed case for dropping it. | No material measured CI saving. |
| 6. Slow setup | Rust cache 56.5 s, clang install 56 s, Linux libraries 15 s medians. Existing dependency-only cache key deliberately avoids workspace-version churn. | Keep `ci.yml:191–238,239,279`. Full-history clones in `swift-core-check.yml:87` and `release-gate.yml:108` serve history-sensitive gates. Do not shallow them blindly. | No safe, demonstrated free cache fix established; no blobless-clone estimate without measurement. |
| 7. Unnecessary job dependencies | Dockerfile lint starts after Rust/harness due to `needs`, although it only reads `image/Dockerfile`. Its successful wall median is 11 s plus 2 s queue. | Remove `needs: [rust, harness]` at `ci.yml:379`, preserving push condition and lint. | About 13 s off successful push workflow critical path; same check, overlapping existing Rust work. |
| 8. Full suite on every PR | `ci.yml:17` starts the full gate for PRs. Sample has 79 PR runs. | Preserve full coverage. Path skipping or selective suites at this trigger need owner approval. | No savings claimed without changed-file and risk analysis. |
| 9. Local checks before push | No tracked pre-push hook or hook manager; README lists manual checks. | Document a quick local check command; keep expensive native checks explicit. Do not install global Git hooks. | Rejected pushes avoided is unmeasured; quick harness checks measured locally at ~11 s total. |

Source line numbers refer to base `2865cb29b`. The lint/container setup split and
all API timing anomalies are retained in local audit evidence. Raw job timing
metadata and the collector remain outside the committed report.

## Free Phase 9 candidates

Parallelize Dockerfile lint and move formatting before heavyweight setup. The
duplicate harness no-emit invocation can also be removed without dropping a test.
No change is made in this phase.

## Needs the owner

- Approve fork workflow runs so native checks can execute; existing draft PR CI
  ends `action_required` with no jobs. Recommended default: permit the existing
  gates; no extra runner spending or reduced suite.
- Paid/larger runners, additional sharded machines and any selective PR test
  policy remain separate owner decisions. Recommended default: retain current
  runners and all tests until CPU/test-only measurements justify a change.

## Before / after

Workflow files and runtime behavior are byte-for-byte unchanged. Phase 0's
lint/typecheck/test/build results therefore carry forward unchanged; no checks
were rerun for this report. The new measured output is the 150-run / 526-job audit.
Native local build capacity and fork approval blockers remain explicit.
