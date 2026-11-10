# Phase 3 structural audit — backend/shared Rust

Historical audit snapshot: later implementation, corrected native artifact isolation
and current prerequisites are recorded in [the phase report](../structure.md).
The original disk/fork-approval observations below are not current blanket blockers.

Inspected base `2865cb29b806121d4a90f55ba6ce151a150279a0`, isolated worktree, branch `improve/structure`. This audit makes no source edits. Inventory spans all 283 Rust files across 11 assigned crates; GPUI is assigned separately. Module/file inventory plus search and focused reads is not a claim of line-by-line review. Supporting raw scan: `backend-special-cases.txt`; count inventory: `backend-structural-inventory.json`; workspace dependency graph: `backend-dependencies.dot`.

## Status and exit condition

**Actionable native issues remain. Phase 3's whole-repository exit condition does not pass.** Several fixes touch durable storage, authorization or runtime scheduling and require their regression tests first. Full native checks are blocked by ~2.7 GiB available disk; fork CI is `action_required` with no job execution (per root). That is a concrete verification blocker, not proof the current implementation is green. No test removal, auth change, live service access or durable rewrite was attempted here. Effect has no Rust integration at these seams; keep native Rust types/futures/tracing rather than introducing TypeScript across the runtime boundary.

## Proposed shared fixes, followed by problems they solve

### 1. Make append/dedupe one atomic database operation — worth fixing, blocked native verification

- **Hidden link:** `relay/db.rs:1879–1890` (`row_seq_for_batch`) and `:1909–1917` (`append_row`) hand-maintain the same rows ∪ quarantine batch lookup. They must change together to preserve replay idempotency. A private shared connection-level lookup could be used from both without re-locking Db.
- **Atomicity gap:** `append_row:1919–1927` inserts room head, increments it, reads seq and inserts bytes through separate autocommit statements. The mutex excludes concurrent callers but cannot roll back prior statements after a failed insert.
- **Local evidence:** `append-row-atomicity.json` records an in-memory SQLite run using the source SCHEMA and an aborting insert trigger: injected row write fails, `head=1`, zero rows, no active transaction. This is SQL-level failure injection, not a Rust runtime regression test or proof an ACK was sent.
- **Shared fix:** hold one SQL transaction around dedupe/head/row insert and commit before returning; factor dedupe at connection/transaction seam; preserve quarantine lookup and `(seq, dup)` contract.
- **Tests/risk:** pin current successful append/replay/quarantined replay behavior; add failed-insert rollback and retry sequence behavior. Reuse `rooms.rs:2582,2876` tests. Data durability is high risk; do not submit as verified without native test execution.

### 2. Separate minimal room position from expensive room statistics — worth fixing in performance phase

- **Needless link:** cache advancement/build only need `(head,floor)` but depend on `Db::room_state`, whose interface also computes checkpoint metadata, count and byte sum (`db.rs:1828–1865`, `rooms.rs:33–34,62–65`). More stored rows make a position lookup slower for an unrelated reason.
- **Shared fix:** a typed focused `room_position(room)` query; retain full statistics at real diagnostic/protocol callers. Avoid a global connection-pool rewrite before measuring.
- **Tests/risk:** source-SQL local scale fixture for exact existing/proposed query; native return/default/caller checks when gates available. This seam is lower risk and directly relevant to chat catchup/send latency.

### 3. Load a chat read model once — worth fixing, blocked behavior verification

- **Hidden link/N+1:** `db_people.rs:483–502` first loads chats then rosters per chat. `people.rs:1144–1168` reads hidden marks separately, followed by `people_chat_dto:480–515` activity/read/unread reads and per-member projection. `member_dto:278–390` queries user cards/presence/project/owner; `member_alive:469` checks projects again.
- **Special cases supported by real requirements:** per-device hidden/read mark, per-user cursor, soft-deleted agents, human/agent defaults, stable same-millisecond member order, generated group names and privacy projection. A DTO cache that omits these would be a regression.
- **Shared fix:** Db owns a batched chat/roster read projection, DTO layer renders already-resolved data; preserve protocol shape/order and authorization at the request edge. Do not make independent raw SQL lookups concurrent against the same mutex.
- **Tests/risk:** exact JSON/roster/order/privacy behavior across human-only, agent-only, mixed group, DM, hidden/deleted and absent member data; existing `db_people.rs:1466`, group name/access tests are meaningful. Medium/high because privacy and compatibility are affected.

### 4. Fail and retry before advancing an undelivered sidecar page — worth fixing, blocked native verification

- **Swallowed failure:** `sidecar/daemon.rs:58–82` logs `mirror.apply_event` errors and ignores a failed `wake_tx.send`, increments the applied count, and saves `page.next` regardless. It can commit a cursor past a local write failure or closed waker.
- **Hidden ordering requirement:** module header says mirror → wake → persist means nothing dropped. Current `apply_page` interface returns only a count, and partial successful processing is not reflected by a separate durable receipt.
- **Shared fix:** explicit receipt/checkpoint progression with retry/idempotent mirror semantics, distinguish required durable writes from best-effort brief/context updates. Do not blindly add `?`: partial mirror success followed by a failed wake could suppress the wake on replay (`Mirror::apply_event` returns None for an already-written message), so design both effects together.
- **Tests/risk:** fault injection before append, after append/before wake, closed channel, metadata write failure, and restart replay. Existing `daemon.rs:680,738,768,817` tests exercise success/reconnect/backoff/key failure; preserve all. Data loss/replay risk is high.

### 5. Represent group-add policy reads as policy-or-error — worth fixing, owner/security review + native gates

- **Swallowed error:** `relay/people.rs:135–143` defaults a failed `group_add_policy` read to `anyone`. `db_accounts.rs:444` already defines absent rows/NULL's open default; catching every database error is broader and can bypass a stored `nobody`/known-people setting during failure.
- **Shared fix:** typed policy and explicit DB error handling; absence compatibility remains its own case. Handle failure at the group-add boundary with a consistent nonleaking error.
- **Tests/risk:** pin `anyone`, `people_i_know`, `nobody`, missing row, invalid stored policy and forced DB failure before changing behavior. Authorization boundary: read local security-boundaries policy when implementing. Listed for owner review; no behavior changed here.

### 6. Compile redaction patterns once without changing the rule table — worth fixing in performance phase, blocked native gates

- **Needless recomputation:** `core/redact.rs:13–57` constructs regular expressions on each call, including every key prefix and static bearer/env/placeholder pattern. `Store::redact` (`agentd/store.rs:312`) calls this on write paths; relay debug/feedback also share it.
- **Existing good structure:** one `KEY_PREFIXES` table drives classification and redaction. Preserve that hidden-link fix; do not create a second pattern inventory.
- **Shared fix:** lazy static compiled pattern sets derived from this table. Preserve pattern order, replacements, invalid-pattern behavior, and exact supported key shape semantics.
- **Tests/risk:** existing redaction/classification tests plus generated nonsecret shape strings, roundtrip markers/idempotence and false-positive corpus. Redaction is a security boundary even though caching is behavior preserving.

### 7. Unify protocol invariants in pure shared code, preserve host-specific adapters — worth design investigation, blocked broad native gates

- **Hidden links:** app `arbos-client/room.rs`, agentd `sync.rs` and CLI `room.rs` each own handshake/cursor/checkpoint/push reconciliation. Same frame types/version-vector/cursor durability invariants recur; hosts persist and schedule differently.
- **Existing meaningful constraints:** app-only `arbos-client` is forbidden to relay/agentd/ctl/local by workspace architecture. FFI and GPUI consume it; app-specific policies cannot be imported wholesale into servers just to remove repetition.
- **Shared fix:** only after real divergent bug evidence, move invariant-only reducers into no-I/O core or a deliberately scoped shared protocol crate and keep persistence/scheduling edges native. Do not rename/move all files or create a general framework speculatively.
- **Tests/risk:** common golden frame event sequences, pending-dependency recovery, ACK idempotency, refused/rate handling and persisted-cursor laws. Numerous existing tests pin regressions; no removals.

### 8. Put lifecycle decisions with the state they mutate — worth fixing in Phase 4, blocked native gates

- **Hidden links:** agent turn fields (`agent.rs:1644–1788`) and Agent maps/atomics (`:2095+`) make cancellation, ack/result row, recovery and work ownership depend on several files/call orders. Persisted `JobRow.state` plus timestamps/error/worker fields (`store.rs:151–193`) permits combinations callers must prevent manually.
- **Shared fix:** explicit state types and transition functions per actual submachine with side effects at edges, not a cosmetic tagged union over the whole Agent. Preserve send ordering, cancel/reap and refusal laws before migration.
- **Tests/risk:** current race suites (`job_*`, rowless/cancel/steer/reparent/refusal/send_order) are meaningful safety nets. High risk; cannot declare no actionable findings without addressing or blocking these.

### 9. Remove confirmed unused aliases and Value adapters — worthwhile small cleanup after test review, blocked compile checks

- `relay/unfurl.rs:481–483`: `fetch_bounded` is explicitly retained for hypothetical other modules, but full Rust-tree search finds only its definition. It rebuilds Fetcher just to delegate; current site-icon path owns a Fetcher. Remove the function, dead_code suppression and stale prose together if compile/doc checks agree.
- `relay/vaults.rs:108–110`: `features()` is a pass-through documented alias with no callers; update stale `features.rs:10` prose when deleting it.
- Candidate unused Value adapters: `people::member_json` (`people.rs:273`), `api::chat_json` (`api.rs:1209`), `signin::user_json` (`signin.rs:184`), `vaults::{access_json,list_json,access_item_json}` (`:600,616,654`). Full Rust search found only those definitions for these names in their modules; verify tests/doc references before deletion.
- **Not dead:** `secrets::{access_json,secret_json}` are called by fixture tests (`secrets.rs:4121–4134`), so a blanket removal of every `allow(dead_code)` would break real wire-contract tests. `HarnessAdapter` has multiple actual runtime adapters and default safe-retention semantics, so its breadth alone is not proof of surplus.
- Avoid touching secret/auth behavior under the guise of alias deletion. Compile warnings and targeted fixture tests must run before calling this clean.

## Special cases deliberately retained

- **Protocol compatibility:** retired avatar columns/feature flags, legacy header/routes, Matrix room-v12 creator fallback and old app DTO fields encode shipped-client contracts. They need a support-window decision, not opportunistic deletion.
- **Quarantine stays outside history reads:** `db.rs:89–103` explains why importing undecodable quarantined bytes into checkpoint coverage reintroduces a wedge; only dedupe reads union quarantine. This comment is a design constraint, not redundant implementation prose.
- **Native wrappers:** FFI records/adapters isolate UniFFI derives and thread ownership; wrappers earn their interface despite line-level forwarding. `core` cannot import them.
- **Polling/retry budgets:** typed send acquisition (`agent.rs:4581–4610`) checks voice preemption while waiting; send-order's 120 s completed-answer release is a deliberate silence/order tradeoff. Busy quick-status's 100 ms snapshots (`status.rs:633–647`) wait only a short grace. These cannot be replaced by one awaited mutex without modeling preemption and wakeup semantics; preserve until a tested event-driven redesign.
- **Process reap sleeps:** isolation reaping (`isolation.rs:426,757`) waits for OS processes and signals; cancel tests exercise real subprocesses. Removing waits mechanically would weaken lifecycle safety. Tests that need OS scheduling should use bounded eventual assertions at behavior boundaries, not arbitrary fewer sleeps.
- **Ignored tests:** actual attributes: relay carry E2E `upgrade_carry.rs:1519` needs Daytona key and creates two paid sandboxes; agentd isolation `isolation.rs:1213` needs root/useradd; room-load profile `sync.rs:2016` is an explicit measurement harness (asserts no correctness). Do not run these, remove them, or mark them passing. Counts in inventory distinguish actual attributes from mentions in comments.
- **Live-secret process registry:** `agentd/store.rs:281–324` holds live vault values only for process-wide redaction. It is a global dependency, but per-workspace isolation would risk logging a value seen by a sibling worker. Any redesign must preserve cross-workspace redaction and lock behavior; not a convenience-global removal.
- **Source-check tests:** some scan literals/constructors to guard security structural invariants (e.g. redaction sweep must not match lifecycle literals), which can catch a real bug even if tied to source. Replace with stronger behavior verification when refactoring that seam; do not delete solely for reading source.

## Comments and Tests additions, in the required order

Comments audit distinguishes behavior/compatibility/privacy/OS constraints above from stale hypothetical wrappers and implementation-restating narration. Remove stale comments with the relevant code, preserve requirements. Tests are reviewed last after actual structural changes. This audit removed **zero** tests; the currently blocked native rewrite cannot safely reduce its safety net. The root's separately verified pure TypeScript extraction should be independently reviewed after the final code/tests exist.

## Dependency review

Workspace graph from cached `cargo metadata --no-deps --locked --offline` shows stable `arbos-core` as dependency of native app-only client, relay, agentd, ctl, local and FFI; FFI additionally depends on app client and Matrix. ACP/sidecar/update remain independent. No existing workspace import violating the documented app-only restrictions was found. Rust module-level cycles and Agent's distributed impls still require the lifecycle refactor above; a reasonable crate graph does not make those issues disappear.

## Remaining action / owner grouping

One owner action can unblock these: provide sufficient isolated local build disk and allow the fork's blocked CI run, then run the required Rust/iOS gates and behavior tests. The high-risk native findings remain explicit review items, not separate requests for broad deployment permission. No deployment, account, runner spend, fewer-test policy or production access is needed or authorized here.

## Narrow native test seam verified after audit

The full native gate blocker remains, but a source-level module seam is feasible without building the workspace: `.improve/evidence/redact-check/Cargo.toml` uses its own `[workspace]` and points `[lib].path` at the actual repository `crates/arbos-core/src/redact.rs`. It pins cached `regex-lite=0.1.9`, `serde_json=1.0.154`, builds offline with two jobs and disabled debug/incremental output. Existing tests: **9 pass / 0 fail / 0 ignored**, wall 2.847 s; artifacts 23,669,773 bytes (~22.6 MiB). Logs/JSON in that directory. Dependency build scripts were inspected first and only perform compiler/config generation. No dependency download or native source edit. This enables a focused future redaction optimization/regression suite, and does not substitute for arbos-core/workspace/native integration gates.
