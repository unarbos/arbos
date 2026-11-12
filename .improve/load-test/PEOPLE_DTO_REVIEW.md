# Bounded people DTO batch review

Source-only review of the batching and live-device index at commit `b96ef1f90`.
The six reviewed hashes are retained in `PEOPLE_DTO_RESULTS.json` under
`builds.after.sources`. No test/build/profile was executed by this reviewer.
Final bounded review passes after the independent error-decoding correction below;
parent native9 and Clippy gates are separate evidence. The later activity-index
change and its additional test have their own `ACTIVITY_NATIVE_GATES.json`.

## Required correction identified

user_presence decodes card and presence in one query_map closure, then filter_map(Result::ok) drops the whole user. That couples two failures that the old member_dto deliberately ignores independently. SQLite tables are affinity-typed, not STRICT: users.created_at can contain a nonnumeric TEXT while live devices have a valid i64 last_seen_at. The card conversion errors, but old user_last_seen still succeeds; candidate loses both. Conversely a non-i64 MAX(devices.last_seen_at) loses a valid card. Parent reproduced both actual native cases RED (0PASS/2semanticFAIL), then isolated card conversion in an inner Result.ok.flatten and independently decoded seen with get.ok.flatten. Re-read actual db_people.rs SHA d3846f6106a079b30a74e492a72c2770727a0b338764d98420280524170cda85: corrected behavior matches both old independent fallbacks, retaining the safe outer requested-id lookup. Final9native/all-target Clippy reported green by parent. Other five source hashes are unchanged. Table-query preparation/step failure remains the existing per-key fallback boundary.

## Coherent reviewed boundaries

- Chat list still obtains only viewer-owned memberships and filters hidden device views before profile hydration. Banned memberships are fetched only when the viewer is owner or qualifying admin; only their human profiles enter the batch. Human/project prefix semantics and original list/member iteration order remain.
- Block batch uses blocker=viewer and blocked=requested-id, same direction as blocks(viewer,id). A HashSet is only lookup state; blocked_senders is still emitted by the original human membership iteration, so no unordered response projection is introduced.
- Project resolver retains live id precedence, then name by descending last_active_at; deleted rows are excluded in both branches. Rust serde_json parsing retains invalid-model default and duplicate-key last-value behavior, rather than relying on a different SQLite JSON interpretation. Public projection excludes private instructions/runtime state.
- Project owner fields use the shared original Sender/card/presence assembler; effective avatar disc uses the same id/color conversion. Missing cards retain original default sender fallback. Legacy device-name lookup and seen MAX use revoked_at IS NULL.
- Partial devices_user_live(user_id) index excludes revoked devices and leaves frequent timestamp updates outside its key. Rowid is the implicit tie order within each equal user key; retained native tie fixture checks the existing first-device winner. No new last_seen ordering/index policy is introduced.
- Baseline bans/hidden/membership/foreign fixtures and whole JSON oracle are appropriate regression seams. The statement-budget callback has no context pointer, so a panic cannot leave a borrowed counter dangling; trace removal and final full native composition are separate checks owned by parent.

## Limits

Request-local maps intentionally produce one profile observation per key; concurrent external writes are not made into a cross-query database transaction. Online timestamps are still evaluated at DTO assembly time and may cross their real-time threshold. This read makes no end-user latency, production-proportion, full-Synapse or statistically significant speedup claim. Arbitrary corrupt types in unused private project columns are not an equivalence guarantee of the new narrow public projection; supported JSON/legacy cases have explicit fixtures.
