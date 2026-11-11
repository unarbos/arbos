# Mac pure cleanup review

PASS for the scoped extraction; full native build remains unverified.

The relay and fixture lexical helper bodies are identical to the extracted
mentions helper after visibility is ignored. Module declarations and explicit
imports resolve the two callers without shadowing the intentionally different
collective-token helper in sync/mod.rs. The new module comment preserves that
policy distinction. Group collective expansion, IDs, parsing and authorization
are unchanged.

Expiry types, all four choices, default and fallback-label implementation are
byte-identical to the original extracted block. Its comments about bounded
lifetimes/default intent moved with it. Both sync callers use the stable module;
UI reexports the two constants used by other UI callers. No caller names the
removed UI Expiry/expiry_label paths. This moves display metadata, not expiry
validation or authorization enforcement.

The direct core avatar color call resolves to the exact function already
reexported by ui/avatars.rs. It removes that UI dependency for this caller without
changing color selection. Other fixture/UI interactions remain outside this
small patch; this is not removal of every sync→UI edge.

Retained GATES.json shows the original three mention/two expiry tests pass before
and after extraction. They cover literal/Unicode/boundary behavior and all
choices plus unknown-hour fallback. No additional test run was needed for this
read-only review. No actionable defect or lost contract comment found.
