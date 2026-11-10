# Generated billed-settlement laws

These checks supplement the existing 41 native caller/concurrency regressions. Four Effect
4 Schema-derived laws each generate 256 seeded inputs (1,024 finite inputs); a fifth test
checks the bounded native transport. A property may run several fresh fixtures, so the
input count is not a count of SQL statements or distinct integration tests.

`extract.py` compiles 34 literal actual definitions from six fixed production files:
`Db::settle_voice_usage`, its shared debit/grant/legacy helpers including the required
configured-cap affected-row guard, actual SpendLimit/threshold/UTC helpers, row readers,
schema DDL and migrations. The reviewed block reader in sibling `pending-owners/extract.py`
is reused. Only imports/modules and a fresh in-memory Db constructor are added; no second
money implementation or product dependency is introduced. Full source and selected-item
hashes are retained. The fixture calls actual schema/migrations but does not certify the
complete filesystem `Db::open` path. The actual bundled SQLite reports 3.46.0
(rusqlite 0.32.1 / libsqlite3-sys 0.30.1), rather than Python's system SQLite.

The native JSONL child receives bounded synthetic integer-micro-dollar reports, returns
actual receipts/account/ordered-ledger/cumulative-row values, and performs no network,
account, key or configuration access. SQL errors return a generic tag without raw errors.
Real production functions own settlement policy; tests calculate expected invariants.
No pass flags are supplied by the child. Dynamic environment parsing helpers are compiled
as part of the actual SpendLimit impl but never called.

The generated properties cover:

- Atomic rollback/retry for receipt/debit ABORT and IGNORE, skipped legacy-user insert,
  skipped configured-cap update, and skipped grant insertion. A successful retry commits
  once and an identical replay adds no debit.
- Repeated/reordered cumulative reports and limited-balance clamping. A mixed voice/vision
  bill pins voice-before-vision allocation, reported-versus-debited distinction and
  nonnegative balance. The cursor may intentionally trail by <=1e-6.
- Existing strict per-part thresholds at 0, 1 and 2 micro-dollars, combined parts, lazy legacy
  initialization and unmetered absence of initialization/debit.
- Daily soft-cap refusal after crossing, negative-balance override, zero-default unfunded
  initialization, and settled refusals that do not debit on replay.

This preserves the existing policy, including <=1e-6 total no-settlement, strict >1e-6
part admission, generic inclusive >=1e-6 chargeability, voice-before-vision order, daily
soft caps, negative override, balance clamp and reported150/debited100 distinction. It
introduces no financial repair, retry, pricing or authorization behavior. Native tests
remain the concurrency and real-handler proof; these finite generated observations are
not exhaustive, a proof of fairness, a whole-relay build or an app-speed measurement.

From repository root, fetch these pinned test-only dependencies before CI's no-egress
fence, then execute the normal suite inside the fence:

```sh
cargo fetch --locked --manifest-path .improve/state-machine-checks/voice-settlement/native/Cargo.toml
node --test harness/arbos-pi-ext/voice-settlement-laws.test.ts
```

As with the sibling pending-owners package, make ordinary CARGO_HOME/RUSTUP_HOME cache
locations and Cargo/Rust/Python/Node PATH available. Builds are locked/offline, ignore
product config/environment, and clean only the owned leaf package to avoid stale copied
workspace artifacts. Generated files and the target are ignored.

After the normal suite passes, run the negative controls within the same no-egress fence:

```sh
python3 -I -B .improve/state-machine-checks/voice-settlement/check_mutants.py --output /tmp/arbos-settlement-mutants
```

They edit only actual definitions in disposable explicit-source copies: skipped cap
updates commit, the settlement cursor never advances, strict part admission becomes
inclusive, or positive-balance clamping is disabled. Each seeded property must falsify,
shrink where applicable, and replay its exact counterexample. Original hashes remain
unchanged. No persistent account database or external effects are used. LAWS_GATES.json
contains exact provenance, finite counts, versions, durations, hashes and mutation results.
