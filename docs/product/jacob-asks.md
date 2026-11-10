# Jacob's asks, and what holds each one

Every row here is something Jacob asked for. Each one names a test that fails if the app stops
doing it. `scripts/ci/jacob-asks-check.py` (when enabled) enforces the pairing on every PR, so an
ask cannot quietly lose its guard and a new ask cannot land without one.

A visual ask with no test is the failure this file exists to stop: it regresses on some unrelated
rebase, nobody notices, and it surfaces weeks later in a screenshot.

The broader register of earlier asks lands with the jacob-ask-registry work; this file starts with
the asks this change answers so they are already named and guarded.

## The rules the check enforces

1. **Every `[feedback <slug>]` tag in `apps/ios/Arbos/Arbos` has a row here.** Answer a new ask,
   tag it in the code, add the row.
2. **Every row names a guard that exists** — a suite (`BlockedStateTests`) or one case
   (`ChatSearchCloseTests.testATapOnAResultClosesThenReveals`) in `ArbosTests` / `ArbosUITests`.
3. **The guard names its ask**, in its own doc comment or its suite's, so the pairing is checked
   rather than asserted here.
4. **The guard is not switched off.** A `throw XCTSkip` that always runs means the ask is
   unguarded.

Naming a suite guards an ask with all of its cases; naming a case is for a suite that covers
several asks. Prefer the narrower one.

## The register

| ask | asked | platform | what it must do | guard |
| --- | --- | --- | --- | --- |
| `solo-1to1-no-agent-reply-quote-ios-2026-11-10` | 2026-11-10 | iPhone | In a solo 1:1 agent chat, an agent bubble never shows a reply quote header — even when the row still carries replyTo from an older box. A group or channel still quotes. A human's own reply still quotes. | `SoloAgentReplyQuoteTests` |
| `solo-1to1-no-agent-reply-quote-mac-2026-11-10` | 2026-11-10 | Mac | Same rule on Mac: solo 1:1 agent bubbles never quote; groups and channels still do; a human's own reply still does. Guarded in-tree by `crates/arbos-mac` `solo_agent_reply_quote_tests`; the iPhone suite is the named twin. | `SoloAgentReplyQuoteTests` |

## Changing a guard

Deleting a row, deleting the test it names, or skipping that test unconditionally is a decision
about whether Jacob still gets what he asked for. Write the reason for a person reading the ask's
history a year from now.
