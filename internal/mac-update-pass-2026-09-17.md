---
cursor:
  subagentId: "bc-71eb0fc3-658e-5b64-8b2b-9854416c9baf"
---

# Mac Update pass — 2026-09-17, 18:40–19:40 UTC

Jacob asked to test the latest on his Mac through the in-app Update. This is what went onto `main` for that, what stayed out and why, and the build the dev channel carries at the end.

Rules kept: every merge is a merge commit on `main`; `rust` fast-forwarded to match after each push; the `v0.2.0` draft release was not touched.

## The build he should see

**Arbos 0.2.0 build 1588**, commit `2c8d879`, published 19:31:55 UTC — the dev channel's newest entry, read from `arbos-dev.json` itself. Notarised: the publish log reads `signed by 'Developer ID Application: Jacob Steeves'`, `Current status: Accepted`, `The staple and validate action worked!`, `source=Notarized Developer ID`. All four payloads resolve with HTTP 200 (macOS app 26.7 MB, macOS kernel 8.4 MB, Linux app 36.1 MB, Linux kernel 9.2 MB). The 1534 entry now lists no Linux app, so the 404 the feed advertised is gone (#487's pruning, on its first run).

## Merged, in order

| Head | Merges | What Jacob gets |
|---|---|---|
| `097c006` | #478 | arbos.life "Download for Mac" serves the newest dev zip while v0.2.0 is a draft (his decision). Site only. |
| `1b51d97` | #464, #479, #467, #481, #436, #476, #432, #482, #483 | Contract rules (twin/stage, producer); a failed connection says why on the bar; the replay race behind every `history_by_the_workers_name` red fixed (#481) and #436 re-landed on it — a worker's name resolves to its record, so the phone's "Nothing on record yet" ends; the side panel as one drawer with its own tabs, asking the kernel what it holds (#476, containing #445); a coordinator's `sleep` to wait on workers refused; Jacob's reports −29…−34; the multitask fold assertion. |
| `cb51833` | #485 | iOS GPT Live: the project line, no double answer, a working tick. |
| `2c8d879` | #487, #488 | Dev channel publishes payloads first with retries, requires this build's own payloads, prunes the feed of anything not on the tag, uploads the feed last — heals the 1534 Linux 404. `surfaces` in the attach snapshot. |

Tests on the merged tree before each push: workspace 834 pass / 0 fail, desktop lib 98 pass / 0 fail for the big batch; `surfaces_e2e` and `serve_e2e` plus core (104) for the last one.

Fix-ups of the steward's own, each stated on the PR:

- #467 over #462: the feed-installed kernel's `Installing` step now fills `bytes` from the feed's payload size (`remote_kernel.rs` took #467's `Installing { version, bytes }` variant).
- #467 shipped three `*.orig` files (`session.rs.orig`, `workspace.rs.orig`, `transcript.rs.orig`); dropped from the merge.
- #482 over #476: both added a key to the driver's chat JSON (`connect_fault`/`reconnect_in_secs` and `hover_link`); both kept.
- #436 re-land: the earlier revert (`62684e1`) was reverted, then the branch head `a2295f2` (the worker's reply pinned to its id) merged on top of #481; its test went from 5-of-6 failing to 6/6 green here.

## Held, and why

| PR | Why it stayed out |
|---|---|
| `cursor/voice-server` `eef9455` (OpenAI GPT-Live engine, post-#421) | Merged locally and run through the 17-scenario voice harness: 16 pass, `work-sound-activity` **fails** — `agent.activity` frames come doubled because #428 on `main` already emits them and this commit adds a second emitter in `base.py`. Not on `main`. The pod runs it from the branch, so the phone is unaffected. Noted for the voice worker on #421: rebase, one emitter, open as a PR. (#421 itself is on `main` as `ea87527`.) |
| #480 (iOS: an opened project does not vanish when its machine goes off) | Author's body: "Cause not established… this stays a draft until the line reads as intended." Not finished by its own account. |
| #433 (iOS: the list's composer names a project on screen) | Red on `restart_states_e2e`, whose fix (#425) is on `main`; the branch is 149 commits behind and has not been re-pushed. iOS change itself is fine. |
| #469 (feedback over ssh for a tab that never attached) | Kernel job red at the time of the pass on a test not yet named; not re-run in this pass. |

Not a hold, but for the record: #445's commits arrived through #476 (the coordinator's word that #476 contains them); #445 itself is left for its author to close.

## Flakes met in this pass

`work-sound-activity` (voice harness) was a real doubling, not a flake. No Rust flake fired on the merged trees this pass; the standing list (directory-renamed re-exec, `feedback_bundle`, `place_probe`, `stop_keeps_follow_up`) did not appear.

## The build, as read from the feed

Dev-channel run for `2c8d879`: all four jobs green (what to publish; Arbos.app macOS arm64; arbos Linux x86_64; sign and publish). Feed at 19:31:55 UTC:

| build | commit | payloads |
|---|---|---|
| **1588** | `2c8d879` | app/macos, kernel/macos, app/linux, kernel/linux |
| 1534 | `097c006` | app/macos, kernel/macos, kernel/linux (Linux app pruned — it never reached the tag) |
| 1509 | `2f9a042` | all four |

`v0.2.0` remains a draft release; nothing in this pass touched it.
