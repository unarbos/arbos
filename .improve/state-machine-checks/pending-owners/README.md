# Approval and creation ownership laws

This independent test-only Cargo package compiles literal definitions selected from the
current production source. `extract.py` records full-file and selected-item SHA256 hashes;
only module/import wrappers and an in-memory Store constructor are added. It does not
copy an alternate transition implementation. Six fixed source paths are read, never
configuration, credentials, service endpoints, or arbitrary requested paths.

The H4 lane classifier uses the actual `WaitDelivery` transition and real oneshot
sender/receiver handles. Actual `Store::decide_approval` / `consume_grant` run against
actual schema DDL plus the existing grant-consumed migration, using bundled SQLite.
The fresh fixture does not certify the complete filesystem migration/open path.
The H6 driver polls real `admit_creation` mutex futures; each valid async scope owns its
permit and borrows it into the actual `CreatingGuard`. There is no unsafe, fabricated
self-reference, or reference implementation of the mutex. Dropping the scope exercises
actual guard/permit cleanup. Rust's borrow checker prevents a guard outliving its permit;
the generated traces exercise legal caller interleavings rather than proving fairness.

Five Effect 4 Schema-derived laws run 256 seeded inputs each (1,280 finite cases):
park/closed delivery ownership, first durable decision plus one-use grant, SQL ABORT/IGNORE
rollback plus legacy terminal compatibility, same-user exclusion plus independent-user
progress, and waiter cancellation/owner release. Mandatory positive probes remain when
random event tails shrink. A sixth test rejects malformed/unknown/oversized protocol
requests and checks recovery and final EOF. Native handler/provider regressions remain
separate evidence; this leaf does not execute full handlers, provider creation, production
SQLite files, external effects, or exhaustively prove state transitions.

From repository root, fetch the pinned test-only dependencies before the CI no-egress
fence (the separate lock uses root-lock versions/checksums), then run inside that fence:

```sh
cargo fetch --locked --manifest-path .improve/state-machine-checks/pending-owners/native/Cargo.toml
node --test harness/arbos-pi-ext/pending-owner-laws.test.ts
```

CI should make its ordinary `CARGO_HOME` and `RUSTUP_HOME` cache locations available to
the test process, alongside Cargo/Rust/Python/Node on PATH. The child build preserves
only PATH and those optional cache roots. It runs Cargo locked/offline and cleans this
leaf package before compiling, so a copied newer artifact cannot replace current source.
The owned target and generated files are ignored. No product dependency is introduced.

Negative controls mutate only one actual production definition in each disposable tree:
closed receiver stays Inline, ignored grant claim commits, same user gets separate
mutexes, and one owner's Drop clears another user's marker. Each must falsify its seeded
law and reproduce the exact shrunk counterexample. The script copies only its explicit
source/driver allowlist, cleans the leaf package, and preserves original source hashes.
On macOS an APFS clone seeds dependencies; elsewhere a temporary ordinary copy is used.
Run with the same no-egress fence and Cargo cache environment:

```sh
python3 -I -B .improve/state-machine-checks/pending-owners/check_mutants.py --output /tmp/arbos-pending-mutants
```

`GATES.json` records the captured source pin, tools, finite counts, durations and mutation
results. Existing H4/H6 native failure pins remain the actual caller/side-effect proof.
