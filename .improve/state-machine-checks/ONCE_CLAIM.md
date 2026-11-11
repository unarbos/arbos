# One-use claim laws

Run `cd harness/arbos-pi-ext && node --test once-claim-laws.test.ts` after the normal pinned harness dependency install. Python3 is the only extra runtime and is already available on the existing Ubuntu CI runner. No services, credentials or network are used; the Python process receives only PATH, runs isolated from a fresh temporary cwd, and uses in-memory SQLite.

Four Effect4 Schema-derived laws execute 256 seeded cases each: spent terminal absorption; exact scope/status/expiry; a unique grant set shared by fields; and atomic rollback on lost claims/SQL errors. Every law has mandatory nonvacuous successful controls. The transport rejects invalid shapes/oversize traces, checks exact IDs and step counts, bounds output/time, and closes its child.

The bridge extracts the exact `SECRETS_SCHEMA` and sole `claim_allowed_once_batch` UPDATE from `crates/arbos-relay/src/db_secrets.rs`, refusing changed extraction shapes. The bridge transaction loop is a small SQL test adapter, not execution of Rust receipt or handler code. Operation-field deduplication and the actual native transaction/opaque-receipt/authorization return boundaries are covered by the 16 native `one_use_regressions` tests. Generated SQL traces complement those tests; they do not replace native integration proof or prove all possible traces.

The bridge reports its Python/SQLite versions and query SHA. The local capture uses Python3.14.2/SQLite3.53.4; the pinned native `libsqlite3-sys0.30.1` bundle is SQLite3.46.0. CI reports its own runtime. No SQLite version equivalence is implied.

For the retained semantic negative control run `python3 .improve/state-machine-checks/check_once_claim_mutant.py --root . --out <private-output-directory>`. It copies only the actual query source, bridge and test into a temporary fixture, links existing dependencies read-only, and changes the actual SQL status guard so a spent row can be claimed again. The unmodified 256-case control must pass; the mutant must produce `ReturnedFalse`, shrink, and replay the identical counterexample. The real input source hashes must remain unchanged. No source is installed or edited by the mutation gate.
