# Settlement fixture bootstrap repair

The combined harness failed before observing settlement behavior: its fresh in-memory database omitted the production devices.user_id migration. Phase 7's real accounts schema now creates devices_user_live on that column. The original compiled Db::fresh diagnostic failed with `no such column: user_id`; the unchanged five-test suite failed 0/5 with strict-response errors.

The one-file patch extracts the complete actual `let _ = c.execute(...)` statement from db.rs exactly once and runs it after SCHEMA, before migrate_accounts/ACCOUNTS_SCHEMA, matching the relevant production ordering. Its ignored duplicate-column result is preserved. Schema/index definitions, settlement algorithms, Rust bridge, strict response schema, generators, seeds and all test assertions are unchanged. PROVENANCE.json checks that the only generated change is this literal statement at bootstrap.

Canonical a4d22466f894: original 0/5 (2.160 s), repaired 5/5 (31.684 s). Standalone Phase 4 9e50073163c8: repaired 5/5 (33.964 s). Each successful run covers four laws ×256 =1,024 generated traces plus nine invalid protocol frames and valid recovery. Timings include bridge preparation and are verification durations, not performance results.

Run after the existing dependencies have been prepared:

```sh
node --test harness/arbos-pi-ext/voice-settlement-laws.test.ts
```

Local checks used separate source snapshots/targets, explicitly cleaned the bridge package, a fresh task HOME and an OS sandbox denying all network. This receipt is fixture verification, not full CI or application/runtime validation. Raw local logs, generated source, caches and diagnostic binaries remain private and are excluded here.
