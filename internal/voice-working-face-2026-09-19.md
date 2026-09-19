---
cursor:
  subagentId: "bc-ba36bd8d-62d9-547d-a1a7-a15f737a6dcf"
---

# Voice working face — 2026-09-19

Jacob's Mac call: after "I just want to know what's going on in this repo" the kernel worked with no tool or agent rows and no spoken "yeah one sec".

## Screenshot

Assigned copy: `/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/voice/2026-09-19-no-tools-no-filler.png`

Source `/home/ubuntu/.cursor/projects/workspace/assets/93bb8199-6a76-4279-a5a3-b577e3220317.png` was not on disk in this environment (the `assets` folder is absent). The copy was not written. The turn he named is the one this PR answers.

## What was wrong

1. **Spoken-row-only hid the work.** `start_call_mirror` only drew `caller.said` and `call_line` (narrator / model.reply). `tool.call`, `agent.event/tool`, `agent.event/say`, `agent.done`, and `agent.turn` were dropped. The comment said the kernel attach already had them. Two further hides sat on top of that:
   - `voice_prompt` draws a display-only user card and does not open a turn. The kernel's later `user` echo matched `awaiting_echo` and returned without `open_turn()`, so `busy()` stayed false and the live working headline / unnamed workers did not show.
   - Project-chat style still hid the root's tool rows behind the fold, even when the items were there.

2. **The working line was muted.** GPT Live is told to say "one sec" only after a delegation. `_turn_to_kernel` sets `decision = "kernel"`, and `_on_model_audio` drops all model audio in that state. So even when the model said it, Jacob heard nothing. `session.thinking.append` ("Arbos is working on it.") is quiet context, not speech. The work-sound bed still follows `agent.activity`; he asked for a spoken line driven by that running state.

## What changed

PR: https://github.com/unarbos/arbos/pull/769
Branch `cursor/voice-tools-and-filler-6dcf` on `unarbos/arbos`.

### Tools and agents visible

- Call `session.start` sets `agents: true` even when `voice_mirror` is off.
- `apply_call_work` writes tool and agent rows from the call mirror (display only; `delegate` is skipped; kernel rows with the same label adopt the placeholder id).
- A kernel echo of a spoken card calls `open_turn()`.
- A voice-delegated turn (`channel = voice` on the user card) does not use project-style hiding of tool and agent rows.

### Spoken working line

- When a delegation actually starts (`mark_working` / kernel turn), the gateway emits `narrator.say` kind `working` and `session.commentary.append` "Yeah, one sec."
- That audio is allowed through while `decision` stays `kernel` (`working_line_open` after the filler words). Other model words stay unheard.
- Barge-in (`on_interrupt`) ducks the line and does not cancel the kernel. Work sound still ducks on either voice.
- GPT Live stays the backend. Jev is not on this path.

## Proof

`cd voice-server && python -m tests.run` (venv with websockets, numpy, httpx, onnxruntime):

| Scenario | Result |
|---|---|
| live-work-question-one-answer | PASS — spoken contains "one sec" and the kernel answer; not "nothing is in progress"; 2 commentaries |
| live-model-delegates-too | PASS — spoken contains "one sec" and tidy-release-notes; not the model's "let me check" |
| live-small-talk-still-speaks | PASS — kernel not asked |
| live-session-context | PASS |
| live-seed-fits-128 | PASS |
| work-sound-activity | PASS — activity `working → tool → working → idle` unchanged |

`cd desktop && cargo check` — clean (6 pre-existing warnings, none in these edits).

No Mac live call in this environment. No browser pass (desktop is native).
