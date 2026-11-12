# State-machine property coverage

The Phase4 PR owns these actual-definition suites together with the code they
exercise. The final combined normal harness gate executes its test files; native
adapter/caller checks cover the effects that finite transition laws cannot prove.
Each file retains seeded Schema-derived inputs, observable outputs and bounded
source-derived bridges. Component documents distinguish inputs from tests and
retain falsification/shrink/replay results. Do not add overlapping counts together.

| Actual boundary | Phase4 test file under harness/arbos-pi-ext | Additional boundary evidence |
|---|---|---|
| Main/worker ticket transitions | worker-lifetime-laws.test.ts | Real RPC/Claude blocked writers, cancellation, restart/drop and registry publication |
| Account/task/Matrix/call ownership | rpc-owner-laws.test.ts | Actual Rust/Swift source and raw Foundation authorization cases; cache close/open races |
| Pending approval/creation permit | pending-owner-laws.test.ts | Actual oneshot/Immediate SQL claim/borrowed cancellation-safe permit;1280 inputs and9 protocol negatives |
| Billed voice settlement | voice-settlement-laws.test.ts | Actual34 definitions/schema/migrations/bundledSQLite;1024 inputs,9 protocol negatives,4 actual mutants; native41 caller/concurrency cases separate |
| Scoped one-use receipts | once-claim-laws.test.ts | Atomic SQL claim and actual external-admission boundaries; uncertain effects remain spent |
| Planned restart | restart-state-laws.test.ts | Freeze/notify/drain/ready ownership, Drop and absorbing shutdown |
| Voice lifetime/mode/vision | voice-state-laws.test.ts | Actual Python controllers, revoked frames/effects, shutdown cleanup and preserved calibration |
| Feedback admission | feedback-state-laws.test.ts | Actual source/approval/revision evidence and persistent delivery states |
| Release publication | publication-state-laws.test.ts | Actual draft intent/publish/refusal/TERM cleanup and local HTTP doubles |
| Deployment lanes | deploy-state-laws.test.ts | Actual queue/revision/approval/hold checks, publication and activation lease adapters |
| Shared Mac heavy slot | lease-state-laws.test.ts | Real permanent flock inode/request/lease/GUI/boot helpers, cancellation/reclaim |

The independent configured-approval reducer tests additionally exercise generated
terminal absorption. Per-draft iOS FIFO laws compile the actual queue through its
Swift bridge, with768 generated inputs and real mutations. Phase3 shared schedule,
room-ACK ownership and sidecar mirror/wake/cursor regressions exercise actual
components. Phase4 speculative room-cache tests use32 seeded native Loro/SQLite
traces: refusal/accepted append/duplicate replay/retirement remain observable.
These native traces are finite generated checks, without a claim to an Effect
implementation, exhaustive interleavings, distributed exactly-once or a proof.

The money domain is Arbos's usage/credit ledger, not staking or wallet allocation.
Reported usage can exceed debited/clamped credit; daily caps are existing soft
caps, and strict micro thresholds remain unchanged. A development oracle that
incorrectly required exact cursor equality was corrected to permit its existing
<=1e-6 lag; that was an oracle defect, not an upstream financial bug.

Every bridge uses bounded synthetic inputs, fresh local state and pinned actual
source/dependencies. Runtime tests are isolated from real accounts/providers.
Full native/canonical runtime gates, SDK compilation and device acceptance remain
separate. Bend2 is absent: no unexecuted formal proof is presented as verified.
