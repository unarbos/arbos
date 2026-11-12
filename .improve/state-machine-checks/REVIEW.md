# Independent voice state review

Scope: AUX-H3 lifecycle, AUX-H4 mode/mute application and AUX-H5 vision consent. The retained property bridge imports the actual new pure Python modules. It does not import the worker, SDK, config, providers or .env. SDK adapter regressions remain a separate gate: state laws alone cannot establish that callers honor the machine.

## Design assessment

- Call lifetime has explicit startup/running/finishing/cleanup-failed/closed variants. An attempt owns its remaining local cleanup steps; repeat stop during an attempt is shared, first reason stays sticky, and retries own only failed steps. One-shot goodbye/left/final usage/shutdown is distinct from retryable local resource cleanup.
- Mode carries group or duplex(participant identity) atomically. Duplex rejects an absent/empty identity. Mute and mode each carry unapplied/applying/applied/failed variants and attempt tokens. Failure retains the last confirmed value plus the requested target; it cannot masquerade as applied success, and reconciliation to either the failed or prior target remains possible.
- Vision has enabled/disabled/closed and monotonic generation. An encoded frame also borrows stream, camera-map ownership and model-session identity, so the adapter checks more than the pure generation model. Observable seeing identities derive from the currently enabled owned streams; no independent mutable seeing set survives retirement.

Controller review found no remaining writable duplicate mode/identity/mute/leaving booleans. Worker `vision_enabled` is the owner's configured preference; the controller's effective lifetime remains its own derived state. Device thresholds, JPEG quality, camera timing math, VAD/idle values and prompts are outside the formalization and are separately checked unchanged by the source owners.

## Retained generated checks

`harness/arbos-pi-ext/voice-state-laws.test.ts` uses Effect4 Schema/Arbitrary, six seeds84001–84006 and256 cases each (1536 voice cases). Mandatory real transitions make each critical assertion nonvacuous, followed by bounded generated event traces. It checks absorbing closed states, monotonic attempts/generations, permanently revoked old frame tokens after re-enable, cleanup ownership/reason across retry, failed mute preserving the confirmed value, retry reachability, and stale mute/mode completions refusing to overwrite a newer request. Successful current completions are positive controls.

The custom shrinker deterministically reduces optional generated traces/values to a valid canonical input; mandatory counterexample scaffolds stay intact. Failures include seed/replay data and the shrunk input. This is bounded property exploration, not a proof for every possible interleaving.

`voice/tests/state_bridge.py` returns actual tagged dataclass snapshots, generations, attempt tokens and operation results, without pass/fail verdicts. The JSONL protocol caps input bytes/events, validates allowlisted event fields/types and IDs, and returns generic malformed-input errors. The caller runs persistent Python with `-I -B`, a clean environment/private cwd, response schema/ID/length checks, deadlines/output cap and child shutdown. An audit hook refuses sockets/subprocesses and credential-file opens. The protocol regression has seven negative requests, including oversized input and a string pretending to be a mute boolean.

`GATES.json` retain exact source hashes, durations and commands. The full harness includes the existing approval generated cases separately; do not conflate its256 with the1536 new voice cases, or with Phase8's separate1792-case suite.

## Negative controls

`check_mutants.py` copies the actual pure source, exact property test and exact bridge into disposable directories. It mutates only the temporary function, reuses installed harness dependencies and leaves live source hashes unchanged. Four actual-function defects each falsify the corresponding generated law, rather than causing a child transport failure:

1. Ignore frame generation: revoked-lease law, seed84002.
2. Commit a mute target on apply failure: failed-mute law, seed84004.
3. Ignore completion attempt: both stale mute/mode laws, seeds84005/84006.
4. Restart a closed call: absorbing-state law, seed84001.

Exact commands, isolated source hashes and counterexamples/replay details are retained in `MUTANTS.json`. They are deliberate negative controls, not product edits or regressions in the actual source.

## Adapter review findings

Actual original worker regressions first showed6 failed/2 controls passed; original expanded vision regressions showed7 failed/2 controls passed. Later adversarial tests pinned additional real edges before changes: startup connect/info/human completion after closure, same-turn mute/unmute stream ownership, immediate seeing retirement, stale queued status attributes, and cancellation of stop/detach while SDK cleanup is paused. `GATES.json` retains exact RED stages and final hashes without raw SDK logs.

Review also flagged late group response/greeting work after waiting for a judge, speak lock or silence: those paths now recheck lifecycle/ownership before starting new work, while retaining the one-shot goodbye during Finishing. A blanket mute guard on group model generation was rejected during review: that path also admits the unmute_self tool, while stable muted SDK output stays disabled. Background `say()`/greeting output and interactive model/tool recovery have different admission requirements; the final patch preserves both stable and uncertain interactive mute recovery, with explicit positive controls. Additional owned-call checks reject late look/blob/handoff work, model context, queued relay dispatch and deferred callbacks after termination.

**Final scoped controller review PASS**, worker SHA256 `dbae3cf0df16897feb159d9be9a5e72761bb52b4a177d4fd733f8918e2ee9548` and vision `0a32b6df229abbc4c4a379e554776fbfdc309e55d88320683f53aeb45c8ab663`. The combined guarded SDK gate is84/84: original10, worker53, vision21, no failures/skips;6.203s wall (`GATES.json`). The reviewed pure module hashes remain those pinned by the1536-case property gate. No remaining actionable defect was found in this scoped diff. These are local effect fakes under an actual guarded SDK, not live-call/device calibration evidence. The authors must describe calibration parity as unchanged protected constants/prompt bodies or a documented normalized comparison, rather than claiming AST identity for entire functions that gained lifecycle guards.

The separate native CI audit found an unisolated Swift test-host production health probe; native simulator gates must isolate that probe before running offline. These SDK-free Python properties do not exercise that test host.
