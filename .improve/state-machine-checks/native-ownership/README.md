# Native ownership laws and controller checks

The explicit native checks compile actual Swift/Rust ownership helpers and
extract actual controller method bodies. They use synthetic HTTP/SDK/store edges,
fresh temporary builds, bounded protocols and no device, Keychain, provider or
existing user database. macOS child execution denies network access. Cargo is
locked/offline; dependencies must already be cached.

From the repository root with Python3, Swift/Rust toolchains, Node22 and the
harness frozen install (including its existing Effect4 dependency):

```sh
python3 .improve/state-machine-checks/native-ownership/check_swift.py --output /tmp/arbos-swift-checks-1
python3 .improve/state-machine-checks/native-ownership/check_mac.py --output /tmp/arbos-mac-checks-1
node --test harness/arbos-pi-ext/native-ownership.laws.ts
python3 .improve/state-machine-checks/native-ownership/check_mutants.py --output /tmp/arbos-native-mutations-1
node harness/arbos-pi-ext/node_modules/typescript/bin/tsc --noEmit --strict --target es2022 --module NodeNext --moduleResolution NodeNext --allowImportingTsExtensions --skipLibCheck --typeRoots harness/arbos-pi-ext/node_modules/@types harness/arbos-pi-ext/native-ownership.laws.ts
```

Output directories for the controller/mutant commands must be fresh. The native
`.laws.ts` command is explicit; the existing Ubuntu `*.test.ts` glob acquires no
compiler prerequisite. `check_mac.py` delegates to the current23-case fresh Shared
successor adapter under `../session-owner`; its old mutable-Shared adapter was
superseded. Its former immediate call revocation contract is exercised by the
separate actual call suite, not by a fake call implementation here. The Swift
controller retains all16 auth/Matrix/snapshot cases, including current controls,
single-flight cleanup, captured logout authority and same-live-doc fallback.

Seven laws use Effect4 Schema/Arbitrary,256 runs each, deterministic seeds
90001–90007, concurrency1 and canonical shrinking. They cover Swift/Rust closed
and foreign work tickets, Swift/Rust Matrix stale completions, Swift account/doc/
revision snapshot admission, UI account+control admission (including backend
replacement under the same control), and TaskOwner closed absorption. Every
trace includes current-owner/progress controls, so rejection is nonvacuous. Noise
and late arrays are bounded0–8 steps; this finite sampled domain is not a proof
for every schedule. The8-test gate passes1,792 generated cases plus malformed
protocol recovery in4.665 s, including native compilation. Strict TypeScript
checking also passes.

The persistent native JSONL bridge returns observations, never law verdict flags.
It hashes all compiled helper sources before/after setup, bounds input/output,
validates IDs/shapes/counts, serializes requests, enforces deadlines and cleans up
children. `ARBOS_NATIVE_RECORD=/tmp/native-gates.json` saves source/outcome metadata.
For a single exact replay, set `ARBOS_NATIVE_LAW` to its seed and
`ARBOS_NATIVE_REPLAY` to the `shrunk_input` JSON in `mutations/RESULTS.json`.

Eight isolated mutations of actual helper functions each falsify, accept one
canonical shrink and fail exact replay, with originals hash-unchanged. These
include accepting a foreign/closed owner, stale Matrix failure resetting current
state, snapshot empty-paint bypassing ownership, stale UI controls/account origin
and reopening a retired task owner. `GATES.json`, `laws-after.log` and
`mutations/RESULTS.json` retain the final expanded domains rather than summing
older overlapping subsets. Compiler failure alone is not a detected mutation.

No product bridge or dependency is added. Actual SDK compiler gates, source-
extracted controller checks and physical GUI/media behavior have different scope.
Already-dispatched remote effects cannot be recalled by rejecting a late local
completion. Current same-account refresh, consent, call membership, media/physical
calibration and same-live-doc snapshot fallback remain deliberate contracts.
