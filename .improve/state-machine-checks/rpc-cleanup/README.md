# RPC cleanup and deterministic stopping evidence

`SOURCE.json` pins the exact repaired production source and test fixture. `GATES.json` records sanitized commands, source/log hashes, durations and results. Product regressions live alongside the actual `RpcChild` and `RpcSessions` code; no parallel model is tested.

The actual RPC writer retains stdin while sending a large synthetic Prompt. If its owned child does not read, the former shutdown waited indefinitely for that mutex before starting its five-second grace. Both actual stop and shutdown failed the retained-writer regression. The repair grants the same five seconds to the entire graceful EOF path, then kills/reaps its captured child before waiting on stdin. Capturing the handle outside the timed future preserves owned cleanup on cancellation. It uses existing Child kill-on-drop and never reconstructs authority from ambient PIDs.

The original three cleanup tests yielded **one control PASS and two semantic FAILs**. Final four PASS: stop, shutdown, graceful EOF, and added cancellation-after-capture control. The related harness gate passes33tests, **including those four**. All-target Clippy/check, package build and three-file formatting pass. The normal EOF marker proves the graceful policy remains; the default response timeout and existing grace are unchanged. This does not replace RPC EOF grace with Claude's immediate-kill policy.

The Linux cancel/sweep fixture also replaces an invalid400ms sleep/obsolete EOF premise with a test-only barrier on that exact child's process-handle mutex. RPC stdin, the session registry and sibling workers remain independent. All four physical-writer test cases, writer-progress budget, exact doomed-session assertions and unread/liveness protections are preserved. The [fresh Linux follow-up](linux/README.md) passes all4 physical and4 RPC cases. Its fixture waits for the actual Ready receipt and resolves the synthetic Python child through the clean PATH; production code and all physical assertions remain unchanged. A Mac host gate alone cannot certify Linux `/proc` behavior.

Re-run focused product regressions using:

```sh
cargo test --locked --offline -p arbos-agentd rpc_blocked_cleanup_ -- --test-threads=2
cargo test --locked --offline -p arbos-agentd harness:: -- --test-threads=2
```

On Linux, also run `cargo test --locked --offline -p arbos-agentd sweep_contention_tests:: -- --test-threads=1`. Tests launch only owned synthetic children. Use the project's no-egress test runner for full package/CI execution. Source and builds here used fresh HOME/TMPDIR, an allowlisted environment, locked/offline cached dependencies and an OS network fence. A unique target cloned only dependency artifacts; all twelve workspace packages were cleaned inside that clone before compiling. No live service, account, key, provider, deployment or release was exercised. Recorded gates are scoped checks, not the full workspace suite.
