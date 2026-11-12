# Atomic cumulative voice settlement

One Immediate SQLite transaction now owns cumulative usage observation, the
existing ordered postfact debit policy, and the settlement cursor. Only a
committed VoiceSettlement carries facts to logs and best-effort analytics after
the database lock is released. SQL errors or skipped critical writes roll back;
existing business-policy refusals remain settled. The same connection-level
credit helpers serve ordinary postfact charges, avoiding duplicated debit math
and repeated legacy grants across separate database handles.

Actual original-source failure pins cover receipt/debit ABORT and IGNORE faults
and concurrent cumulative reports: 5 semantic failures plus 8 passing policy controls.
Two further SQL-ignore cases failed against the initial candidate: a skipped
legacy-user insert or configured-cap update could still commit a debit under the
schema default. The required cap update now checks its affected-row count. These
extra before gates are labeled candidate execution, not original-base execution.

Final native results: voice 24 (17 new, including a finite 32-trace actual-helper
fault/retry/reordered-report law and input-decoder cases), credits 8, accounts 9:
**41 distinct tests, 0 fail/ignored**. All-target relay Clippy with warnings denied
passes 28.914 s. The exact earlier frozen candidate separately passed check, Clippy
and production build; its 39 tests overlap the final 41 and must not be added.
Commands, hashes, counts and durations are in GATES.json and BEFORE.json.
A unique private APFS-cloned dependency target had all 12 workspace packages
cleaned initially, and the changed production package was explicitly rebuilt
after refinement. Builds use the root lockfile/offline Cargo, OS-denied network,
a fresh HOME and no inherited service options. No real financial account or
external effect was exercised.

These native regressions use the existing Cargo test setup:

```sh
cargo test --locked --offline -p arbos-relay voice:: -- --test-threads=2
cargo test --locked --offline -p arbos-relay credits::tests:: -- --test-threads=2
cargo test --locked --offline -p arbos-relay db_accounts:: -- --test-threads=2
cargo clippy --locked --offline -p arbos-relay --all-targets -- -D warnings
```

Use the project's isolated test runner, or a fresh test HOME with networking
disabled. The selected tests use synthetic local state and do not require any
account or service configuration. This is finite native execution; no Effect
shrinking, exhaustive proof, full workspace or app-performance claim is made.

CALIBRATION.json records preserved thresholds, strict part admission, voice-before-
vision order, UTC checks, legacy default 0 semantics, soft caps, negative override
and balance clamp. Reported settlement differs from ledger debit: reported 150
can debit 100. No schema, pricing, automatic retry or historical reconciliation
is introduced. Informational machine-meter receipts remain separate catalog scope.

## Needs the owner

Review the intentional storage-failure behavior before merging: failed settlement
is retryable atomically; business-policy refusals still advance reported totals.
The original five failures and extra two required-metadata failures demonstrate
why. Historical inconsistencies are not repaired automatically.
