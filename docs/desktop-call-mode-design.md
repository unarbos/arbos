> **RECOVERED from the repository mirror — nearly complete.** Source: `docs/design/desktop-call-mode-design.md` in `unarbos/arbos` at commit `1374ed4` (2026-09-13 21:11 UTC). 21,849 bytes there against 22,278 in the store's last listing, so about 429 bytes of the original — most likely store-relative media links — are not in this copy.
>
> The original was lost together with the whole `docs/` directory on 2026-09-16 between 07:43 and 09:01 UTC. Restored by the store-recovery worker `bc-0b112226-cf98-5cab-92c3-2671518dd9b9`. Cause, timeline and the full recovery inventory: `internal/store-docs-loss-2026-09-16.md`.

# Desktop call mode: voice as a narrator over the main chat

Design, 2026-09-13. Short and scannable. Terms are defined the first time they appear.

**Status (2026-09-17 20:30 UTC):** slice 7: gateway half [#492](https://github.com/unarbos/arbos/pull/492) is on `main` (it carried the first cut of the desktop work too); desktop half [#490](https://github.com/unarbos/arbos/pull/490) is one desktop-only commit on `main` `7ea0a3ec`, merges clean, desktop harness 23/23 · 22/22 · 12/12: a call reaches the caller's project or is refused with the reason in the chat; voice rows are display-only. Jacob's Mac must join the hub before a call from it can attach (see Slice 7). Awaiting that and the Mac live check. Earlier: **Status (2026-09-13 20:50 UTC):** slice 5 (desktop call = phone call) on #100/#101, handed to the Mac worker for the live check. Earlier status: slices 1–4 implemented on [#100](https://github.com/unarbos/arbos/pull/100) (gateway, deployed on the pod), [#99](https://github.com/unarbos/arbos/pull/99) (kernel), [#101](https://github.com/unarbos/arbos/pull/101) (desktop, on #71). Harness: 11 scenarios green; desktop test 13/13; live calls through the tunnel and through the hub into `mac/.arbos` verified. **Ongoing perfecting is owned by the QA loop** from here. #101 is rebased on #71 at `56132a6` (head `2d421cc`): the spoken-line mark reads `channel` from the transcript event, the call strip names the microphone and its level (or the mic error in red), and a call without a microphone program is refused with the install hint; integration `9645676` (mic by name, ffmpeg off the GUI PATH) is cherry-picked until #71 carries it. Nothing is left open for the call-mode agent.

## 1. Model

**One project, one main agent, one conversation, two channels.**

- The main agent is the project's `root` agent in `.arbos/agents/root/`. It is a coordinator (Cursor model): it spawns children, steers them, and says things. Children do the work.
- The **text channel** is the desktop transcript. The **voice channel** is the call. Both write to the same agent state on disk: `inbox/` files start turns, `turns/` record them, `transcript.jsonl` is the record. Nothing forks: there is no "voice conversation" beside the chat.
- The **narrator** is a small program in the voice gateway (the `voice-server/` process). It sits between the kernel and the caller. It holds a rolling summary of the main transcript, speaks highlights, answers "more detail" questions from the transcript, and dispatches sub-agents through the same `spawn` path the main agent uses. It is policy code plus an optional small model; it is not a second agent and has no folder.
- The **speech model** (NVIDIA NemotronLabs VoiceChat, full duplex) is the narrator's ears and mouth for the fast part of a call: hearing, acknowledging in under a second, and calling tools. The narrator's own highlights are voiced by the gateway's TTS (Kokoro) so they are exact.

### What a voice utterance becomes

1. The speech model transcribes it (`transcript.final`).
2. The gateway writes it to the main agent as a `user` frame with `channel = "voice"`. The kernel files it as `agents/root/inbox/<time>-user-<seq>.md` with `channel = "voice"` in the front matter (`steer` when a turn is running). Typed text goes the same way with `channel = "text"`. The two interleave in the inbox by time; the transcript shows both as user lines, each tagged with its channel.
3. The narrator says "On it." (gateway voice) when the words go to the agent. The speech model's own voice is off by default in call mode (see "The speech model's own voice"); its tools in call mode are `more_detail`, `agent_status`; work requests are already on their way to the main agent, so it must not `send_agent` itself. (Outside call mode, `send_agent` stays.)
4. The main agent's turn runs. The narrator watches the kernel attach stream.

### Narration policy: what is spoken

| Event on the kernel stream | Spoken? | How |
| --- | --- | --- |
| Main agent turn ends (`event assistant` for `root`) | yes | Highlight: first sentence plus the one that names a result, cap 240 chars / 2 sentences. If the text was longer than the cap or held code, a diff, a list or a link: append "The rest is on your screen." |
| Child `done` file lands (`event say from=<child>` whose text starts `Turn ended`) | yes | "<child> is done: <last words, cap 200>." Bad end: "<child> ended badly: <why, cap 200>." |
| `ask` frame, a question | yes | "Arbos asks: <question, cap 200>. Options: …" Then wait; the caller's next utterance is the answer (`answer` frame with the ask id). Never auto-filled. |
| `ask` frame, an approval (`allow <tool>: <command>`) | yes | "Arbos wants to run <tool>: <command, cap 120>. Allow?" A yes/allow or no/deny in the next utterance is the `approve` frame at once ("Allowed." / "Denied."); other words leave it open and go where they were going; a question card answered elsewhere closes it (the `approval` event). At 30 s: "Still waiting on <tool>: … Allow? I deny it in 15 seconds." At 45 s (`--approval-timeout`): denied, "No answer in 45 seconds; I denied <tool>: <command>." **The gateway never auto-approves** (`--auto-approve` opts its own kernel in explicitly; a hub attach never). |
| Kernel link lost / restored (`link` frames from the client's reconnect) | yes, once each | "I lost the connection to the agent. Reconnecting." / "Connected to the agent again." Reconnect with backoff 1, 2, 4 … 30 s; frames sent meanwhile wait and go out on restore. |
| `notice failed=true`, `error` frame | yes | "Something failed: <cap 160>." |
| Tool start/end, folds, compaction, thinking, diffs, job output, `tree`, `plan` | no | They are on the screen. `more_detail` can read them. |
| Streaming deltas (`assistant_delta`) | no | Spoken once, at the turn end, as a highlight. |

Rules:

- **Length caps**: 240 chars for highlights, 200 for child reports, 160 for errors. Over the cap: cut at a sentence end. The pointer to the screen ("The rest is on your screen." / "Details are on your screen." / "More on your screen." / "The full reply is on your screen.", rotating; "in the chat" on the phone) is added only when more than one paragraph is left unspoken (code, a list, a diff and further prose count as paragraphs).
- **Coalescing**: highlights that arrive while one is being spoken queue; two child reports within 3 s become one line ("Two agents finished: A and B."). A queue longer than 3 drops the oldest and says "and more on your screen".
- **No self-echo**: the highlight is the main agent's reply to the caller, so the caller's own words are never read back.
- **Provenance**: every spoken line is also sent to the client as `narrator.say {text, kind, ref}` (`ref` = `transcript:<seq>` or `agent:<id>`), so the desktop can write it into the transcript as a `voice ·` line and the record is complete.

### Drill-down by phrase

The speech model seldom claims "why exactly did it fail?" with `more_detail`. Once something has been said, the narrator recognises the phrasing itself (why/what/how … exactly, say that again, more detail, what did it say/write/change, what was the error), answers from the record, and leaves the main agent alone. A tool call for the same question within seconds gets the same answer (one line spoken).

### Who answers an utterance: the grace rule

Every utterance is on its way to the main agent by default. It waits a short grace (1.5 s) for the speech model to claim it as a question for the narrator (a `more_detail` or `agent_status` tool call). Claimed: the narrator answers from the record and the main agent never sees it. Not claimed: it goes to the agent with `channel = "voice"`, the narrator says "On it.", and the agent's reply arrives as a highlight. Nothing is lost when the model stays quiet; a drill-down does not wake the agent. Measured on the pod: the NVIDIA model rarely calls `more_detail`, so most drill-downs today take the second path (about 5 s to the answer).

### The speech model's own voice

`--call-model-voice auto | off | ack | full` (default `auto`). Live on the pod the duplex model answered for the agent ("I cannot send agents to write poems…") and filled silences; narrator-only (`off`) fixed that but made the desktop call feel one-way (Jacob: "not full duplex, did not talk back"). `auto` is the phone's feel: small talk and general questions (`is_conversational`: greetings, thanks, short lines, what/who/how-many questions with no project words) are answered by the model in its own voice and never reach the agent; anything else goes to the agent, the model's own answer to it is cut, and the narrator says "On it." and the result. `ack` lets the model's first three seconds through; `full` is everything (the harness uses it to test barge-in against the model's speech).

### Dictation

`session.start {mode: "dictation"}` is the ASR pipeline on any engine: whisper partials into the composer as the words come, a final on release, no reply, no speech model, no agent. Measured on the pod: first partial 0.71 s after speech begins, final 0.58 s after speech ends, WER 15% on the fixed sentence (orthography only); the old path (a `voice` session on the duplex model, which also answered the dictation) was 1.03 s / 0.85 s.

### Drill-down protocol

The caller asks for more ("why exactly did it fail?", "what did the agent change?"). Two tiers:

1. **From the record, no new work.** The speech model calls `more_detail(question)`. The narrator reads the relevant part of the transcript through the kernel's file frames (`read`, `tail`, `list` on `agents/<id>/transcript.jsonl` and `agents/<id>/jobs/*/out.log`), picks the lines that answer (tool errors, notices, the child's last words, the last assistant text), and answers in at most three sentences. With a narrator model configured (`--narrator-model`, OpenRouter) the excerpt is summarised for the question; without one, the extractive answer is spoken as is. Deterministic in tests.
2. **Escalate only when new work is needed.** If the record does not hold the answer (the narrator finds nothing, or the question asks to change something), the narrator says so and hands the question to the main agent as a normal utterance (`channel = "voice"`). The main agent's reply then arrives as a highlight.

### Interruption and turn-taking

- Full duplex: the caller can speak at any time. On `speech.started` while the gateway is speaking, the client flushes playback and sends `interrupt`; the gateway drops the queued audio (gen bump) and everything queued behind it, and emits `response.done {interrupted:true}`.
- An interrupted highlight is not retried aloud. It stays on the screen (the `narrator.say` frame was already sent). The narrator remembers it as "not heard" so `more_detail` can bring it back on request ("say that again").
- The caller's utterance during a running main-agent turn is a **steer** (`steer = true`), the same as typing during a turn.
- The speech model's own chatter is bounded by instructions ("acknowledge in one short sentence, then wait") and by the gateway's dispatch guard (one dispatch per utterance).
- Silence is fine. The narrator speaks only on events; it never fills quiet.

### Interleaving on disk

```
agents/root/inbox/2026-09-13T15-02-10Z-user-000.md   channel = "voice"  "Send an agent to fix the CI skeptic test"
agents/root/inbox/2026-09-13T15-02-41Z-user-001.md   channel = "text"   "Also bump the timeout to 60 s" (typed during the call; steer)
agents/root/inbox/2026-09-13T15-04-03Z-agent-fix-ci-000.md  kind = "done"  (kernel wrote it when the child ended)
```

`channel` (`voice` | `text`) and `device` (`phone` | `desktop` | `cli`) are optional keys on inbox front matter, absent on old files and on kernel/agent messages. The kernel copies them from the `user` frame onto the inbox file and onto the transcript's `user` line; clients never write inbox files themselves. A `user` frame without a channel is a typed line.

## 2. Multitasking

- The main agent stays a coordinator: `spawn`, steer (`say`), `say`. It does not edit code. Children work in their own folders and the kernel writes a `done` inbox file to the parent when each child turn ends (#98).
- The narrator reports each `done` as it lands (see the table). The main agent's own follow-up turn (it wakes on the `done` file) produces the next highlight.
- Queued follow-ups from voice behave exactly like typed ones: a `user` frame; the kernel decides steer versus queue by whether a turn is running. The desktop's queue strip shows both channels.
- The main chat keeps streaming text on the screen during the call. Voice never blocks it: the narrator is a listener on the attach stream, not a participant in the turn.

## 3. UI in the tabs layout (#71)

- **Call button** in the right panel head (beside the project name): a phone glyph, tooltip "Call this project". `⌘⇧C` also starts a call. Disabled with a tooltip when `voice_url` is not set. One call per window; a second tab's Call ends the first.
- **In-call state** replaces the context row under the composer with the call strip: an orb (pulses with the mic level when listening, with a slow wave when Arbos speaks), a live transcript strip (the caller's partial words, then the last spoken highlight, one line each, truncating), `speaker: <device> · mic: <device> · <level>%` (or the mic error in red, so a silent call is never a mystery), a **Mute** toggle, and **End**. Audio plays in-process (cpal on the default output; PCM16 24 kHz resampled and peak-normalised to about -6 dBFS; the queue is cleared on `response.done {interrupted}`); the mic streams the whole call; `client.speaking` brackets playback and a client-side energy gate silences the mic's echo of the speaker while a voice over it still passes. A call is refused up front, with the install hint, when no microphone program exists. The composer stays usable: text typed during the call is sent as usual (`channel = "text"`).
- **Spoken highlights appear in the transcript** as `voice · <text>` notice lines (from `narrator.say`); "On it." is heard, not written. The caller's utterances arrive as ordinary user cards the moment the kernel records them (the desktop takes the live `user` event from any client and drops only the echo of its own prompt), drawn with a small microphone when spoken on a call.
- **Sub-agents keep appearing in the right panel** as today; the narrator's child reports refer to them by name.
- Driver: `state.call = {active, phase, muted, partial, last_said}` so the harness and the QA loop can assert on it.

## 4. Where the narrator runs

- In the voice gateway (server), one `Narrator` per call. Phone and desktop share one implementation; the desktop's local mode runs the same gateway on the laptop (`deploy/run.sh --engine pipeline --reply kernel --kernel-place <place>`).
- `session.start` gains `project` (`"<machine>/<project>"`, a hub name), `device`, and `mode: "call"`. The gateway resolves the project to a kernel: its own `--kernel` when `project` is empty or names it (`--hub-machine/<place>`, or the bare folder name); otherwise a per-call attach `wss://<hub>/attach/<machine>/<project>` with the gateway's hub client token (`--hub`, `--hub-token`; the token lives with the gateway, not the caller), the plain attach wire over a WebSocket. The call's tools, mirror and narrator follow that attach; `session.ready` says `via: hub`. A name the hub does not know is **refused** (`error {code: project_unknown|project_offline|project_unreachable|no_hub}`, close 4404), and since [#492](https://github.com/unarbos/arbos/pull/492) so is a bare folder name the gateway does not serve (`project_not_on_hub`): the call reaches the caller's project or nothing, never another kernel. The desktop names its tab `<machine from ~/.config/arbos/hub.toml, or ssh alias>/<folder>`, and sends the bare folder only when it has no machine name — which a gateway elsewhere refuses, so the desktop refuses first, in the chat, with what to do.
- Transcript deltas and events come from the kernel attach stream the gateway already holds; drill-down uses the kernel's `read`/`tail`/`list` frames on the same socket, so the narrator never touches files itself and works for remote kernels.

## 5. Test harness (standing goal: perfect the interface, test it by mocking voice)

`voice-server/tests/` drives a real gateway process two ways, no GPU:

- **Real audio uplink**: scripted user lines become WAV utterances (Kokoro TTS when its model files are present; else a deterministic speech-like synthetic signal). The harness plays them into the uplink at real time with scripted pauses, and can start one on top of Arbos's reply (barge-in).
- **Mock duplex model**: a WebSocket server that speaks the NemotronLabs realtime protocol the gateway expects. Energy VAD on the uplink drives `speech_started/stopped`; utterance N is "recognised" as scripted line N; scripted outputs are `response.output_audio*` (a tone with transcript) or `response.function_call_arguments.done` (tool calls). Barge-in stops its audio and closes the response.
- **Mock kernel**: speaks the attach wire (`hello`, `snapshot`, `user`, `event`, `turn`, `read`, `tail`, `list`) and writes a real `.arbos/` place on disk (inbox files with `channel`, transcripts), scripted per scenario (spawn a child, finish it after N seconds, produce a large tool output). A real kernel can replace it (`--kernel tcp://...`).
- **Assertions**: what was spoken (every `narrator.say` and `response.transcript` the client received, plus that audio bytes followed), inbox files and their `channel`, and desktop state through the driver socket when `ARBOS_DESKTOP_BIN` is set.
- **Scenarios** (`tests/scenarios/*.toml`): `dispatch-highlight-drilldown`, `interrupt-mid-sentence`, `typed-during-call`, `child-done-while-talking`, `large-output-not-read`, `narrator-only-voice`. Run: `cd voice-server && python -m tests.run` (all) or `python -m tests.run <name>`. `python -m tests.desktop_call <name>` drives the built desktop under Xvfb through the same mocks. `python -m tests.live_call --url …` runs a scripted real call. The QA loop runs the same commands.

## Slice 2 (2026-09-13, same PRs)

- Q1 fixed: the desktop shows a user card for a prompt any client sent (the gateway's spoken line, the phone), with a microphone when spoken on a call. Root cause: the live `user` event fell through `kernel_event` to the empty arm; only a reload of the transcript file showed it.
- Hub attach per call (above); mock hub and `hub-attach` scenario in the harness; the pod gateway holds the hub client token and answers a foreign name through the real hub.
- Q4 decided: `channel` + `device`, two fields, on the frame, the inbox file and the transcript line.
- Q5 measured: policy stays the default; `--highlights model` with a gpt-4.1-mini-class model is available behind guardrails; nano-class models invent outcomes.
- Narrator: a turn's final words are not lost when the next turn starts before they arrive.

## Slice 7 (2026-09-17, the call is for the caller's project or nothing — the ArbosLife fix)

Jacob's Mac call talked to ArbosLife. The Mac is off the hub, so the desktop sent the bare folder name; the gateway took a bare name as its own kernel — the phone kernel on ArbosLife. Two PRs:

- **Gateway, [#492](https://github.com/unarbos/arbos/pull/492) (voice worker, deployed 19:39Z):** a bare name the gateway does not serve is refused (`project_not_on_hub`, close 4404, a message saying what to do); `machine/project` attaches through the hub or is refused; `session.ready.project_info.place` says which folder the call is on. GPT Live gets the project brief, the last 12 chat lines and quiet context during the call — from the call's kernel, so the client sends none of it. Reasoning for Jacob: `docs/gpt-live-context.md`.
- **Desktop, [#490](https://github.com/unarbos/arbos/pull/490):** `session.start.project` is `{machine, project, path, host, name}` when the machine is known, else the bare folder (never a dict without a machine, which the gateway reads as "no project"). A call from a machine off the hub to a gateway elsewhere is refused **before dialing**, in the chat, with the hub.toml instructions; a gateway refusal lands in the chat as a red `voice · call refused: <code>: <message>` line instead of "did not answer". `project_info.place` is checked against the tab's folder; a mismatch hangs up. **Voice rows are display-only:** the caller's `transcript.final` becomes a `voice` user card at once and the kernel's echo merges into it; the model's accumulated `response.transcript` becomes a `voice ·` row on `response.done`, deduplicated against narrator lines and bare acks. Nothing is sent to the kernel for a spoken line. Typed lines go to the kernel as before.
- Harness: `tests.desktop_call` (22/22, 23/23 on main's and #492's gateway; `ARBOS_VOICE_SERVER_SRC` runs another checkout's gateway) and the new `tests.desktop_call_refused` (7/7). Contract note for the voice worker: `internal/call-mode-project-binding-agreement.md`.
- **For the Mac:** the call works only once the Mac is on the hub (`~/.config/arbos/hub.toml` with url, `machine = "mac"`, token; restart the kernel). Until then the desktop refuses with those words.

## Slice 6 (2026-09-17, the sound of work)

While the agent runs a turn or a tool, the call plays a quiet bed (or ticks), driven only by `agent.activity` frames the gateway derives from the kernel's own frames, with a 5 s heartbeat; it ducks under any voice and stops when the frames stop. Design note and Jacob's one question (bed or ticks): `docs/call-mode-work-sound.md`. PR on `main`.

## Slice 5 (2026-09-13, desktop call = phone call, on #100 and #101)

- What was missing on Jacob's Mac: reply audio went to a player program the Mac lacks (dropped silently); the call was narrator-only (no reply to small talk); Fn dictation ran as a `voice` session on the duplex model.
- Fixed: in-process cpal playback with normaliser and interrupt stop; `--call-model-voice auto`; dictation mode on the gateway; `client.speaking` + client-side echo gate + `--echo-margin`; strip `speaker · mic · level`; a call without a mic program is refused.
- Scenarios: `desktop_call.py` (17 checks, downlink verified), `desktop_dictation.py` (5), `small-talk-model-voice`; `live_dictation.py` for real numbers. Handed to the Mac worker for the live check.

## Slice 4 (2026-09-13, decisions on the open questions, on #100)

- Approvals re-ask once at two thirds of the wait, deny at the end (30 s / 45 s by default).
- Screen pointer only when more than one paragraph is left unspoken; phrasing rotates.
- Kernel reconnect for the shared and the per-call client, with backoff and an outbox; spoken once each way. Scenario `kernel-reconnect` (the mock kernel restarts mid-call; the caller's next words still land).
- Escalation log: question-shaped utterances forwarded to the agent within 90 s of a highlight are appended to `$VOICE_HOME/logs/call-mode-escalations.jsonl` on the gateway; mined for the drill-down phrase list (`_DRILL` in `narrator.py`). First entry: "and is that the whole sentence it wrote?" after a report.

## Slice 3 (2026-09-13, safety, on #100)

- The gateway never auto-approves. Approvals and asks are spoken and answered by voice or a card; an unanswered approval is denied on timeout with a spoken note. Outside call mode an asks-only narrator does the same for the phone's voice sessions. Scenarios: `approval-spoken-and-answered`, `approval-timeout-denies`.
- Drill-down by phrase (above); scenario `drilldown-by-phrase`.
- Live: a real call from the pod gateway through `arbos-hub` into `mac/.arbos` (Jacob's Mac): a read-only question answered as a highlight in 23 s, no approvals asked.

## Slice 1 (implemented, 2026-09-13)

PRs: gateway [#100](https://github.com/unarbos/arbos/pull/100) (on `cursor/voice-server`, #56), kernel [#99](https://github.com/unarbos/arbos/pull/99) (on the integration base), desktop [#101](https://github.com/unarbos/arbos/pull/101) (on #71).

- Gateway: `voice_server/narrator.py`; `narrator.say` frames; `more_detail` tool; `session.start {mode:"call", project, channel, screen}`; `user` frames carry `channel`; kernel `read`/`tail`/`list` helpers in `kernel.py`; `--tts tone`, `--asr none`, energy-VAD fallback, `--narrator-model`, `--call-model-voice`; harness under `voice-server/tests/`. Deployed on the pod gateway.
- Kernel: `channel` on `Frame::User`, `inbox::Message`, `Node`; written to the inbox file (PR on the integration base).
- Desktop (on #71): handset in the panel head (⇧⌘C), call strip (orb, live words, Mute ⇧⌘M, End), `voice ·` lines, driver `state.call`.
- Measured on the pod (real duplex model, phone kernel, gpt-4.1-mini): "On it." 2.0 s after the caller stops; the main agent's highlight 7 s; the sub-agent's report 15 s; a drill-down answered by the main agent 4.7 s.

## Open questions

1. Decided: grow the drill-down phrase list from the escalation log (QA loop; each new phrase gets a scenario line).
2. Shipped (#101 `0ef9779`): the card reads `channel` from the transcript event; the "a call is live" guess remains only for lines from a kernel that wrote no channel.
3. Decided and shipped: re-ask at 30 s, deny at 45 s.
4. Decided and shipped: reconnect with backoff, spoken once.
5. Decided and shipped: screen pointer only when more than one paragraph is left; rotating phrasing.
