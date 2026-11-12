# Actual fork CI evidence

Observed at 2026-11-10T15:07:54.094987+00:00. Exact run/head/job identities, job-step durations and emitted test summaries are in RESULTS.json. Raw logs and full API actor payloads remain private; LOG-PROVENANCE.json carries only their hashes and lengths.

| Linux run | Source | Status | Rust P/F/ignored | Node P/F | Rust job seconds |
|---|---|---|---|---|---:|
| [p3](https://github.com/cisterciansis/arbos/actions/runs/42826558559) | d40572b8fb43 | completed: success | 2423/0/3 | 14/0 | 2573.0 |
| [p6](https://github.com/cisterciansis/arbos/actions/runs/42828799835) | 84cb882feefc | completed: success | 2374/0/3 | 14/0 | 1878.0 |
| [p8](https://github.com/cisterciansis/arbos/actions/runs/42826558764) | b15e3f9ba808 | completed: success | 2376/0/3 | 14/0 | 2461.0 |
| [final](https://github.com/cisterciansis/arbos/actions/runs/42833942969) | 419d3944cf77 | completed: success | 2586/0/3 | 94/0 | 2106.0 |
| [p4](https://github.com/cisterciansis/arbos/actions/runs/42833942694) | 6247938b68aa | completed: success | 2548/0/3 | 94/0 | 693.0 |
| [p5](https://github.com/cisterciansis/arbos/actions/runs/42833941005) | ef3cfc72a020 | completed: success | 2373/0/3 | 14/0 | 2441.0 |
| [p7](https://github.com/cisterciansis/arbos/actions/runs/42833940432) | a35e9ccfd54b | completed: success | 2387/0/3 | 14/0 | 2460.0 |

For verified completed Linux runs, the actual fenced step invoked run-tests-no-egress.sh, emitted a runtime smoke observation with non-root UID/GID, empty groups, no-new-privileges, zero capabilities and only loopback, then emitted nonzero Rust summaries. The positive loopback fixture worked and a documentation-address socket was immediately unreachable. Runner/smoke source hashes are bound to each exact head. All Rust summary lines are counted, including zero-test binaries and ignored tests; zero-test summaries are never represented as the whole test gate. Layout Python contract counts are 31 except logging-only P5 (19); image is a lint job, not a test-count claim.

| Swift run | Source | Workflow / actual build-test state | Assertion evidence |
|---|---|---|---|
| [a4-swift](https://github.com/cisterciansis/arbos/actions/runs/42826560300) | a4d22466f894 | in_progress/pending; in_progress/pending | logs_unavailable_no_assertion_inference |
| [p8-swift](https://github.com/cisterciansis/arbos/actions/runs/42826558733) | b15e3f9ba808 | in_progress/pending; in_progress/pending | logs_unavailable_no_assertion_inference |
| [final-swift](https://github.com/cisterciansis/arbos/actions/runs/42833942995) | 419d3944cf77 | completed/failure; completed/cancelled | runner_not_acquired_no_assertions |
| [p4-swift](https://github.com/cisterciansis/arbos/actions/runs/42833942682) | 6247938b68aa | completed/failure; completed/cancelled | runner_not_acquired_no_assertions |
| [p7-swift](https://github.com/cisterciansis/arbos/actions/runs/42833940490) | a35e9ccfd54b | completed/failure; completed/cancelled | runner_not_acquired_no_assertions |
| [p6-swift](https://github.com/cisterciansis/arbos/actions/runs/42828800182) | 84cb882feefc | in_progress/pending; in_progress/pending | logs_unavailable_no_assertion_inference |
| [p3-swift](https://github.com/cisterciansis/arbos/actions/runs/42826558755) | d40572b8fb43 | completed/failure; completed/cancelled | runner_not_acquired_no_assertions |
| [p5-swift](https://github.com/cisterciansis/arbos/actions/runs/42833940754) | ef3cfc72a020 | completed/success; completed/skipped | skipped_not_applicable_no_assertions |

Swift eligibility jobs have no assertion count. A skipped build is N/A; queued/active jobs with unavailable logs have no inferred assertion progress. Hosted-runner acquisition/capacity annotations are infrastructure observations and do not count as assertion failures or successes. Any completed XCTest console observations remain separate per suite/attempt; nested summaries are not blindly summed. This read-only evidence collection did not rerun, cancel or mutate CI; earlier task actions are documented in REPORT.

Job and step seconds are wall-clock API timestamps, affected by runner/cache/workload conditions. They are not app latency, benchmark evidence or a claimed CI speedup. Completed counts apply only to the exact source identities shown.
