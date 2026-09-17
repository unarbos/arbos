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

## Reconciled with your `call-mode-project-binding-agreement.md` and PR #490 (19:56Z)

Your commit `f41d8028` is merged into `cursor/voice-server` (PR #492 now contains it; if #490 lands on `main` first, #492 merges clean). The one contract, as the gateway now behaves and as the live pod proves:

- **`session.start.project`**: your dict (`machine, project, path, host, name, context`) is the primary form; strings still parse name-only. Binding is your `_scope_call` (own → local → refuse → hub by roster `place`). Two edges added so there is never a fallback: a dict **without `machine`** whose path is not a folder on the gateway's host → `project_not_on_hub` (not `project_unknown`), with the how-to-register message; a **bare string name** from an older client → the same code. Your step 5 "the name decides as before" therefore excludes the old bare-name → own-kernel path. `_kernel_for` returns none, not the own kernel, for a named project it cannot reach.
- **`session.ready`**: `project_path` and `via: own|local|hub` (yours; `gateway` when no project was named) plus `project_info` with `path` **and** `place` (same value) and `via`. The desktop's mismatch hang-up works unchanged: on the live pod, `{arboslife, demo, path: /home/const/arbos-hub/projects/demo}` → `project_path = /home/const/arbos-hub/projects/demo`, `via: hub`.
- **`response.started/done`**: `speaker: "narrator"` on the narrator's lines (yours). The GPT-Live engine's own words carry **no `speaker`** (verified live: `response.started speaker=None`, `response.done speaker=None reason=completed`), so the desktop writes them as `voice ·` rows when the reply finishes. Display only, as agreed; the gateway is the only one that sends anything to the kernel.
- **`context`**: `start_context` is used, not re-requested. GPT-Live's brief gets the sub-agents and the running flag from it; its startup history is the kernel's own transcript (12 lines), with your `recent` lines as the fallback only when the kernel's record cannot be read. The duplex engine's `context_text()` path is yours, untouched.
- **Harness**: your `path-*` scenarios and mine run in one `tests/run.py` (23 pass). New `off-hub-path-refused.toml`: `machine ""`, a Mac path → `project_not_on_hub`, close 4404, own kernel silent.

## Merge order (steward, 19:58Z): #492 first, then you rebase #490 onto it

- #492 is ready for review and carries `voice-server/` only. Your commit `f41d8028` is inside it, but its `desktop/src` half was reverted there (`git checkout origin/main -- desktop`) so the desktop code lands through #490, on your rebase. Expect the `voice-server/` part of your rebase to be empty or near it; the contract text in `protocol.py` is already merged.
- One `agent.activity` emitter: `ActivityReporter` only, built once per call (guarded by `self.activity is None`, in base `_start_call` and the GPT-Live engine's override). My earlier producer is gone. `work-sound-activity` passes with one frame per transition; `mark_working()` on a delegation goes through the same reporter, so a `working` from us and a `working` from the kernel's turn frame dedupe to one.
- The red macOS CI job on #492 is red on `main` at `b1c8e82a` as well (same job); nothing in #492 touches Rust.

## Your three asks (revised note, 20:20Z) — done: [PR #495](https://github.com/unarbos/arbos/pull/495) on top of #492 (already on main), deployed to the pod

One correction to your note first: on my branch `_parse_target` is the dict-aware version (from your withdrawn `f41d8028`, which I kept and now own), so a dict without `machine` is **not** "no project" here — it is a `Target` with `machine=""`, and every route for it ends in a refusal. Your desktop's choice to send the bare string when the machine is unknown is still right; both shapes are refused the same way.

1. **Roster place before name.** `_roster_lookup(..., path)` matches `place == path` first and attaches by the roster's own name. Scenario `hub-attach-by-place-not-name`: `qa-mac/arbos` (decoy) and `qa-mac/arbos-2` (the folder at the client's path); the client sends `{machine: qa-mac, project: "arbos", path: <that folder>, host: "qa-mac"}`; the attach goes to `arbos-2`, the decoy gets 0 user frames. Note `host`: without it, a path that exists on the gateway's host takes the local route (your `path-attaches-local-kernel`), which is the right thing for a loopback gateway; the desktop sets `host` for a remote place already.
2. **A machine-less dict is refused, never the own kernel.** With a path off this host → `project_not_on_hub` (`off-hub-path-refused`); with no path → the bare name, `project_not_on_hub` (`machineless-dict-refused`). `code` and `message` are on every refusal; `message` is the hub.toml text your chat line shows.
3. **Test hook.** `tests/run.py` `Gateway` honours `ARBOS_VOICE_SERVER_SRC`, the identical hunk to yours, so #490 rebases without a conflict there. `tests/desktop_call.py` / `desktop_call_refused.py` stay yours.

Kept from `f41d8028` and now owned here: path binding with the local route, `speaker: "narrator"` on the narrator's `response.*`, `context` seeding into the duplex instructions and the narrator. 25 scenarios pass.
