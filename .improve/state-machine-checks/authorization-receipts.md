# Observed one-use grants and owned receipts

A successful exact observed-grant SQLite claim now owns each sensitive disclosure
or prepared external attempt. Concurrent answered-card requests can no longer
both disclose a one-use value, SQL consumption failure refuses disclosure, and
an old browser card cannot substitute a newer matching authorization. Complete
local request preparation precedes claim. Multi-field operations deduplicate one
grant and claim their entire unique set in one transaction; lost rows, changed
scope/status/current expiry or SQL failure roll back every claim. Metadata use
requires the opaque non-Clone receipt. No transaction survives external I/O.

Original actual-handler checks produced3controls/3semantic failures; a later
preparation refinement failed once and batch refinement failed three times
against its intermediate candidate. They are distinct stages. Final16new native
checks are a subset of39secret/db-secret checks; with3vault controls there are
42distinct targeted passes. Four EffectSchema-derived exact-SQL laws execute
1024seeded traces plus protocol recovery. The real status-guard mutant is detected,
shrinks and exactly replays. SQL bridge coverage complements the native Rust
receipt/handler tests; its PythonSQLite version is reported separately from the
native bundled engine. See ONCE_CLAIM.md and ONCE_CLAIM_VERIFICATION.json.

## Needs the owner

Review at-most-one authorized attempt/disclosure before merging: local preparation
failure retains authorization, but cancellation or uncertain remote outcome after
claim may spend it; no restoration or automatic retry is introduced. Existing
selection remains project/target/host/use-kind based, with the caller's current
asker checks. CAS compares the selected row's stored chat; it does not introduce
an operation-chat restriction. Confirm whether allow-once should be chat-local;
recommended default is preserving the existing policy until that contract is
established. No live secret/provider or real credential was used in verification.
