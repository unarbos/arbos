# Structural regression checks

Run the actual standard-library-only Mac modules, with independent function mutations:

```sh
python3 .improve/structural-checks/check_mac.py --mutants
```

This compiles the production modules used by main/relay/fixture: title8, mentions3,
expiry2, pending ownership5, snapshot scheduling7 and room-document ownership7
tests (32total). Three broken
snapshot decisions must fail those same production-module tests. No app, device,
network, model or environment credentials are loaded. Generated logs/results stay
ignored. This does not compile the surrounding GPUI caller.

Other retained product regressions use the existing project test setup:

```sh
cargo test --locked -p arbos-sidecar daemon::tests
cargo test --locked -p arbos-sidecar mirror::tests
cargo test --locked -p arbos-relay append_row_tests
cargo test --locked -p arbos-relay group_add_privacy_tests
cargo test --locked -p arbos-core --test sites_routes
node --test edge/sites-worker/tests/routes.test.js
python3 scripts/ci/test_xctestrun_env.py -v
```

Use a fresh local test environment and a network fence; these commands do not
supply a production endpoint or opt into the ignored live/provider suites. The
retained native gates used actual source, cached locked dependencies and macOS
network denial. They are distinct from fullworkspace, socket-room or GUI tests.
The Phase9 Linux wrapper retains loopback for the complete native suite.

The JSON records retain RED/GREEN counts, commands, durations and source hashes.
Filtered DB/people test runs overlap and must not be summed into unique coverage.
Source-only Mac/API cleanup has reference inspection, formatting and its package
checks; deleting unreachable code is not an app-speed measurement.

The sidecar guarantee is replayable page application and local queue acceptance.
A saved cursor does not establish durable agent delivery: a crash can lose an
unconsumed volatile wake, or repeat an accepted wake before cursor persistence.
No outbox, end-to-end exactly-once/at-least-once or fsync guarantee was added.

Mac ACK ownership now separates rooms, preserving the old within-room rule that
any ACK clears that room's pending indicators. Exact batch-to-message receipts
remain outside this change. Snapshot scheduling retains80ms trailing windows,
immediate live updates, LinkLive duplicate suppression and the supplied Tokio
clock; old test-only copies were removed after actual-source checks caught mutants.

The shared sites fixture binds the actual core and Worker policies. Reserved,
punycode and ambiguous double-dash routes are intentionally refused according to
the existing authoritative allocation rules; valid landing/slash adapters remain.

The reviewed test-plan relay isolation is carried here as a safety prerequisite
before any core-path push can start Swift checks. It is byte-identical to the
Phase9 helper/Fastfile change; integration must recognize that identical overlap.
It keeps the complete test scheme and does not force global fixture state.

A later review caught an intermediate page-dedup regression: two distinct
messages were collapsed when positions were missing/shared. Two new actual daemon
cases failed before the correction. The final key includes chat/event/message
identity, and all15daemon+4mirror cases pass. Identical copies still wake once;
completed-page replay remains unchanged. `identity-red.json`, `identity-clippy.json`
and the final root native gate record distinguish this follow-up from the earlier
13-case candidate.

Room document work now holds one owned per-room mutex handle, while the registry
lock only looks up/replaces handles or admits short publication. Same-room Loro
serialization stays intact. Exact-source replay of snapshot versus newest-message
read reproduces the old cross-room block, then passes with the new ownership
boundary; `room-docs.json` distinguishes five assertions from the seven production
module tests. Replacement, retirement and registry destruction revoke old handles.
The Phase4 account-session work supplies real account retirement hooks and owns
the broader store/task lifetime; this module alone does not prove account isolation.

The full actual Mac all-target compiler/Clippy gate passes with warnings denied
(`native-mac.json`,23.291s). It uses the inspected, checksum-verified pinned public
media SDK locally, locked offline dependencies, a private target with all workspace
crate artifacts cleaned, and network denial. It is compiler/lint evidence, not a
physical device or complete GUI test run. Whole combined tests remain separate.
