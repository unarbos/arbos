# Linux cleanup fixture and RPC validation

Final verification passes **four physical-writer/sweep cases and four actual RPC cleanup cases** under the verified local Linux kernel fence. The four physical cases retain their original independently observed `/proc` writers, writer-progress budget, exact unread session, and honest `Nothing` versus `Unconfirmed` assertions. No assertion, test case, production behavior, timeout or grace constant was removed or relaxed.

This two-file test-only successor makes two setup requirements explicit:

1. `/proc` can observe a child before PiAdapter completes GetState and publishes Ready. Wait for the actual doomed worker's Ready receipt before capturing its process-handle barrier; the barrier then holds only that child while the sibling registry remains readable.
2. Resolve literal `python3` using the runner's clean known PATH, as existing fake-Pi fixtures do. The local image provides it under `/usr/local/bin`, while other platforms use `/usr/bin`. Preserve `-I`, `-u` and the same stdlib-only synthetic child. No provider, model, SDK or new configuration is involved.

`SOURCE.json` pins the patch base, exact before/after source and final binary. `GATES.json` distinguishes the immutable historical failures from the final result. The v2 failure was a Starting/Ready fixture race. The v3 physical group passed; its four RPC setup failures were the missing absolute interpreter path, not shutdown failures. Final v4 passes both groups. The earlier actual production backpressure regression (Mac one control PASS/two semantic FAILs before the cleanup repair) remains separate evidence; these fixture changes do not modify that repaired production function.

The final fresh no-run build took 227.636 seconds; the two groups took 15.024 and 12.048 seconds, 28.078 seconds container wall. All twelve workspace packages were cleaned only inside a new private target clone before compiling; source and the original targets were not modified. Builds were locked/offline with Rust 1.99.1 and an immutable local image, without dependency fetching.

Runtime independently verified UID/GID, empty supplementary groups, zero capability sets, no-new-privileges, read-only root/source/target, fresh HOME, executable tmpfs, working self-hosted loopback, no non-loopback address/route, and immediate numeric TEST-NET unreachability. The local Docker adapter explicitly checks its nine inert tunnel interfaces are down and addressless. It does not change the project's stricter CI predicate requiring only loopback. No real endpoint, account, credential, host socket or host PID namespace was exposed.

Re-run on Linux with the project's no-egress runner, selecting `sweep_contention_tests::` and `rpc_blocked_cleanup_` if using a scoped test invocation. The verification here runs eight selected actual-source cases, not the whole workspace suite. Raw logs, local SDK/cache paths and binaries are intentionally omitted from this portable receipt.
