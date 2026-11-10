# iOS account work and draft ownership checks

From the repository root, on macOS with the Apple Swift toolchain and Python 3:

```sh
python3 -I -B .improve/state-machine-checks/ios-account-work/run_all.py --out /tmp/arbos-ios-account-checks
```

The output path must not exist. `--tree PATH` checks an explicit source tree;
`--only uploads` (or another named gate from `--help`) runs one controller group.
The complete command now has 38 entrypoints and 296 runtime assertions. It compiles
selected **actual production method bodies** into fresh temporary executables,
with paused synthetic API/store/UI/SDK collaborators, then runs them under
`network*` denial. No relay, account, keychain, model, device, microphone, camera,
notification service or existing database is used. Every driver records source
and fixture hashes, compiler status, exact expected case count and elapsed time;
the top-level runner rejects missing results, compiler errors, zero/wrong counts,
nonzero exits, assertion failures and source changes. The synthetic collaborator
contracts are intentionally smaller than the real SDKs.

`GATES.json` records the earlier pre-ChatView checkpoint: 280/280 assertions in
31.885 seconds on one local machine. These are correctness gates, not app-speed
measurements. `BEFORE.json` retains the earlier failure-first cases, including
positive controls. Those 31 groups contain 126 failures and 84 passing controls;
the three ConnectReveal closure cases were pinned after the shared model fix,
before its UI callback fix. The separate final-edge checks cover a late private
reveal and queued announcement. The real SDK compiler also caught two introduced
candidate wiring errors (a duplicate announcement binding and a missing picker
parameter); both were corrected and the picker probe now compiles its actual
helper signature, clear callback and all three caller expressions. Compiler
errors are not counted as original behavioral failures. An initial portable
runner path error and a scratch TypeScript launcher path error were harness setup
errors, then corrected; the final command ran all expected gates.

## Contract exercised

`AccountWork` captures the existing opaque `WorkLifetime.Ticket` and an
owner-bound `RelayAPI` on the main actor. It does not introduce an account epoch.
Ordinary same-account refresh keeps that owner; retirement makes queued work and
late results ineligible. Both initial request and retry authorization use that
same owner. UI callbacks capture it before scheduling tasks or while rendering
the action, and check it again before publishing private results, errors or busy
state. Completion checks remain necessary around later non-HTTP awaits.

The controller groups cover directory fallback, registry/cache/room creation,
machine/account/unread/pins/people/server refresh, startup and push handoff,
Matrix credential fetch, secrets/device/server/profile actions, shared rendered
callback capture, private screenshot/feedback state, member/group actions,
first-login retry, sticker/GIF requests, media/memo/forward uploads, notification
construction, known-user replacement and account reset. The upload controls keep
input order, bytes, MIME, reply/forward/extra parts, and the existing difference:
people sends omit a failed attachment; project sends retain its nil-blob row.
The push-admission fixture checks the source admission prefix, not every event
branch. Opaque store-doc bytes are a SQL/controller seam, not a valid Loro fixture.

The account shell uses the opaque Hashable ticket as its SwiftUI identity so a
real retirement remounts private view state; its four portable checks establish
identity semantics and the production consumer, not rendered SwiftUI behavior.
Item confirmations have their separate actual-helper suite. The final ChatView
and card callback audit is a separate source handoff; these counts do not include
that later work. Parent SDK verification compiles the full app against real
LiveKit/FFI; unit/UI execution and physical media/device behavior remain separate.

## Draft reducer laws

With the repository's frozen harness dependencies installed (Effect 4.1.2,
TypeScript and Node 22), explicitly run the native-compiler-dependent laws:

```sh
cd harness/arbos-pi-ext
node --test draft-send.laws.ts
```

This is intentionally outside the ordinary `*.test.ts` glob: the command requires
Swift and macOS in addition to the TypeScript harness. Three Schema/Arbitrary
laws, seeds 90008–90010, run 256 cases each. They check per-draft FIFO isolation,
stale completion rejection after a same-draft retry, and absorbing retirement /
foreign-owner admission with positive current-owner controls. The bounded JSONL
bridge returns actual reducer observations; TypeScript supplies the expectations.
A maximum of 64 actions / 65,536 input bytes, sequential requests, reply schemas,
IDs, deadlines, child cleanup and malformed-request recovery prevent missing
observations from appearing as passes. The module is also in the Xcode app source
build phase; compiling it only in the oracle is not sufficient integration proof.

```sh
python3 -I -B .improve/state-machine-checks/ios-account-work/check_draft_mutants.py
```

Two disposable mutations alter the actual production transition: accepting an
old attempt, and reopening a closed queue. Both are falsified, meaningfully
shrunk and replayed with the same counterexample; `DRAFT-MUTANTS.json` records
those observations. `DRAFT-LAWS.json` records the 768 successful generated cases.
These are sampled laws over a declared finite trace domain, not a mathematical
proof or whole-app state-machine certification. The retained current-draft,
coalescing, callback order, cancellation and account-retirement controller cases
exercise the effect adapter separately.

## Policy and limits

Already-issued remote effects cannot be recalled. Previously admitted same-account
draft sends keep their destination and callback FIFO when a different draft is
selected; their completion cannot clear or route that replacement draft. Account
retirement closes all draft queues. Existing attachment failure policies,
authorization/consent/role policies, calibration, timeouts and delays are kept.
Public/pre-auth reads, synchronous URL construction and fixture paths remain
available. The opt-in MetricKit queue is deliberately device-wide and identity-free
under its existing contract; this change does not redefine it as account data.
APNs/global notification attribution and persistent legacy-cache provenance are
not certified by selected controller tests. Full app/unit/UI/device verification
is reported separately rather than inferred from these local collaborators.

## Raw HTTP admission follow-up

The upload review found four raw-body/read methods bypassing the bound account
admission used by JSON requests. The actual methods and bound API, intercepted
by URLProtocol under network denial, initially failed all four retired-owner
cases while twelve current/unbound/wire-policy controls passed (1.582 seconds).
`requestBearer` now supplies the same atomic actor admission and fixed bearer
to JSON, blob PUT/read, avatar PUT and transcription. All sixteen cases pass
in 1.528 seconds; `RAW-API-BEFORE.json` and `RAW-API-AFTER.json` record them.
Bodies, MIME, response handling, the transcription deadline and the original raw
methods' no-401-retry behavior are preserved. These cases are additive to the
280-case checkpoint; a later full command records 296 only when actually run.
The current and original selected HTTP adapters include this actual helper;
the historical Matrix adapter also includes the shared AccountWork boundary
required by the evolved production startup method. No copied authority model
is used to decide the expected retired-owner outcome.
