# Jacob's asks, and what holds each one

Every row here is something Jacob asked for. Each one names a test that fails if the app stops
doing it. `scripts/ci/jacob-asks-check.py` enforces the pairing so an ask cannot quietly lose its
guard and a new ask cannot land without one.

This branch registers the two mention-chip asks below. The broader register of Jacob's other asks
lands separately; absorb these rows when that register merges.

## The rules the check enforces

1. **Every `[feedback <slug>]` tag in `apps/ios/Arbos/Arbos` has a row here.** Answer a new ask,
   tag it in the code, add the row.
2. **Every row names a guard that exists** — a suite or one case in `ArbosTests` / `ArbosUITests`.
3. **The guard names its ask**, in its own doc comment or its suite's.
4. **The guard is not switched off.** A `throw XCTSkip` that always runs means the ask is
   unguarded.

## The register

| ask | asked | platform | what it must do | guard |
| --- | --- | --- | --- | --- |
| `mention-chip-name-tie-ios-2026-10-29` | 2026-10-29 | iPhone | A mention chip keeps the first person when two share a display name — never the dictionary's pick — so the photo does not flip to another Scout's initials. | `MentionDraftTests.testASharedDisplayNameResolvesToTheFirstEntityOnEveryRebuild` |
| `typed-handle-mention-chip-2026-10-29` | 2026-10-29 | Mac | A typed `@handle` of mine is my chip for me too (the viewer is in the table by handle like every other member). | `MentionDraftTests.testMyOwnTypedHandleIsAChip` |

## Superseded

Kept so the tag in the code resolves, and so nobody re-adds the behaviour by mistake.

| ask | asked | why | what replaced it |
| --- | --- | --- | --- |

## Changing a guard

Deleting a row, deleting the test it names, or skipping that test unconditionally is a decision
about whether Jacob still gets what he asked for. `scripts/ci/jacob-guard-gate.py` fails a PR
that does any of the three unless its description carries a line naming the ask:

```
Guard-change: <ask-id> — <why, and who agreed>
```
