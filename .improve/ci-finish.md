# Free CI fixes and verification

Phase2 remains report-only. This phase applies three free changes:

1. Run rustfmt before dependency setup and expensive compilation.
2. Remove only the duplicate identical TypeScript no-output build alias; retain
   the typecheck and every test.
3. Run independent image lint without waiting for unrelated jobs.

Existing runners, job identities, test suites and dependency caches remain. No
paid runner, reduced suite, merge or deployment was used. Queue/setup/task savings
in the audit remain estimates; unrelated CI wall times are not app speedups.

Runtime safety is explicit. Linux tests require the verified namespace runner:
nonroot identity, zero groups/capabilities, NoNewPrivs, only-loopback networking,
positive loopback and negative external reachability controls. XCTest plans write
and verify loopback relay endpoints for all unit/UI hosts and the UI app. Native
parent-process denial permits only the exact owned active-boot Apple IPC socket;
it does not establish universal simulator egress isolation.

Seven completed Linux source runs pass. The combined Rust input has2586 passes,
zero failures and3 existing ignored cases; its normal harness has94 passes.
Final integrated native verification accepts559 unit cases (558 strict passes,
one intentional expected-failure control) and five avatar UI cases with no skips.
The separate complete predecessor UI run has55 passes and13 existing opt-in skips;
its sole unit global-counter failure remains an accurately recorded full-run red.
The DEBUG observer/test-only successor fixes that oracle and preserves Release
projection. These scopes are not summed or called a final unfiltered full-UI run.

The versioned source-only assembler uses immutable phase pins, exact blob/mode/
diff guards and private indexes, with no conflict fallback or branch merges.
Final Rust execution is reused by exact input projection; changed Swift/helper/
workflow inputs have separate checks. Historical hosted failures, source identity,
native fixtures/FFI, before/after numbers and remaining owner work are in
[REPORT.md](REPORT.md), [CI receipts](verification/ci/README.md),
[native receipts](verification/swift/observer-final/README.md), and
[the recipe](integration/assembly/README.md).

The owner will ask Jacob (const) about existing telemetry. Completed logging fixes
remain; broader tracing/propagation/lint/wide events/tail sampling are deferred
without blocking these phases.
