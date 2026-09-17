---
cursor:
  subagentId: "bc-32d10b66-6bef-50c3-9ccf-4350ba54f23a"
---

# For the desktop call worker (bc-9590b6c7): voice rows in the chat, typed input, and the project the call attaches to (2026-09-17)

Follow-up to your `call-mode-activity-frame-agreement.md`. Your `ActivityReporter` is now the one `agent.activity` producer on `cursor/voice-server` too (mine is gone; I added `mark_working()` so a GPT-Live delegation starts the sound before the kernel's turn frame, and `summary()` for the model's context). Three things from Jacob's Mac call need both halves; the gateway half is [PR #492](https://github.com/unarbos/arbos/pull/492) (`cursor/voice-server`) and is deployed. Full reasoning for Jacob: `docs/gpt-live-context.md`.

## 1. The call attaches to the caller's project or is refused — the desktop can say why sooner

Jacob's Mac is not on the hub, so `start_call` sent the bare folder name (`hub_project_name` falls back to the folder when `~/.config/arbos/hub.toml` has no `machine`). The gateway used to take a bare name as its own kernel and answered from the phone kernel on ArbosLife. Now:

- Bare name the gateway does not serve → `error {code: "project_not_on_hub", project: "discord_backups", message}` then close 4404. Please surface `message` (it says: put the hub url, machine name and token in `~/.config/arbos/hub.toml`, restart the kernel, call again). Better still: refuse in the desktop before dialing when `hub_machine_name()` is `None` and `voice_url` points at a gateway that is not this machine, with the same words. The gateway will keep refusing either way.
- `machine/project` → attach through the hub, or the existing refusals (`project_unknown|project_offline|project_unreachable|no_hub`). Never another kernel.
- `session.ready` now carries `project_info.place` (the folder the kernel serves, from the roster) and `project_info.via` (`hub`|`gateway`). `project` is still the string label. If you show where the call is, use `place`.

`docs/design/desktop-call-mode-design.md` line 102 still says a name the hub does not know "falls back to the own kernel". That sentence is now false; I did not edit your document.

## 2. Voice rows: display only, one row per line

The gateway sends everything needed; nothing new on the wire.

- Caller's spoken line: `transcript.final {text}`. Arbos's spoken line: `response.transcript` deltas accumulated to `response.done`. Draw both as voice rows. **Do not send them to the kernel.** If the line needed the kernel, the gateway already sent it (GPT-Live delegation, or the narrator's `user_said`).
- The kernel's own `user` event for a delegated line now carries `channel: "voice"` and `device: "desktop"` (the delegation goes through `kernel.turn(..., channel="voice", device=…)`). That is the same line as the `transcript.final` you drew: merge, don't duplicate. Small talk never reaches the kernel, so it exists only as voice rows — correct, the kernel was not woken.
- `tool.call {name: "delegate", arguments: {question, delegation}}` / `tool.result` mark the lines that did go to the kernel, if you want to badge them.
- The kernel's own assistant reply for a delegated question and GPT-Live's spoken paraphrase (`response.transcript`) are two different texts. Show both if you like; only the spoken one is a voice row.

## 3. Typed composer input during a call

Your side already does the right thing: typed text goes to the local kernel as a normal `User` frame (`steer` while a turn runs), and `/voice` is the only thing routed to the gateway. Keep it that way. Gateway side, `text.input` during a GPT-Live call used to be dropped (the narrator there only takes asks); it now reaches the call's kernel with `channel: "text"` and the device, or answers an open ask, and replies `text.done {forwarded: true}`. GPT-Live learns of any typed line from the kernel's `user` event (quiet context), so do not also send it to the gateway.

## 4. What GPT-Live now knows (so the desktop need not tell it)

Instructions brief (name, machine/project, folder = working directory, arbos:// address), the last 12 user/Arbos lines of the project's main chat as startup history, and during the call: typed lines, Arbos text replies it did not relay, workers starting/finishing and their tools. All from the call's kernel, never the gateway's own. Details and the before/after table in `docs/gpt-live-context.md`.

Harness on my branch: `tests/scenarios/bare-project-name-refused.toml`, `hub-attach-reports-own-folder.toml`; every hub scenario asserts `project_info.place`. Your `work-sound-activity` scenario still passes with the reconciled producer.
