---
cursor:
  subagentId: "bc-32d10b66-6bef-50c3-9ccf-4350ba54f23a"
---

# What GPT Live can see on a call

Jacob asked two things: why the call thought it was on ArbosLife, and what GPT Live can actually see — including chat history. This page answers both, literally. Terms are defined the first time they appear.

## Words used here

- **Gateway**: our voice server (`voice-server/`). The phone and the desktop connect to it. It connects to GPT Live and to a kernel.
- **Kernel**: `arbos-kernel serve <folder>`. One kernel serves one project folder. Its working directory is that folder. Its chat is the file `<folder>/.arbos/agents/root/transcript.jsonl`.
- **Hub**: the mesh directory. Kernels register on it as `<machine>/<project>`. The gateway can reach a kernel on another machine only through the hub.
- **GPT Live session**: one WebSocket to OpenAI for the length of the call. It holds the model's memory of the call.
- **Client delegation**: GPT Live decides a question needs the backend and raises a delegation. The gateway asks the kernel and hands the answer back. GPT Live speaks it.
- **Inject**: text the gateway sends into the GPT Live session. Three kinds: `instructions` (rules), `session.input` (chat history at start), and appends during the call (`thinking` = quiet context; `commentary` = words to speak; `instructions.append` = new rules).

## The wrong-directory bug, in one paragraph

Jacob's Mac is not on the hub. So the desktop named the project by its bare folder name, `discord_backups`, not `mac/discord_backups`. The gateway saw a bare name it did not serve and fell back to its own default kernel: the phone kernel on ArbosLife. GPT Live was told nothing about place, delegated "where are we?" to that kernel, and reported ArbosLife's folder. Two faults: the fallback, and the silence about place.

## The fix ([PR #492](https://github.com/unarbos/arbos/pull/492), branch `cursor/voice-server`, deployed to the production gateway 2026-09-17 19:39Z)

1. **The call's kernel is the caller's project or nothing.** A bare name the gateway does not serve is refused: `error {code: "project_not_on_hub", project, message}` then close 4404. The message says what to do. No other kernel takes the call. A `machine/project` name attaches through the hub to that kernel, and only that kernel. An empty name still means the gateway's own kernel, and the gateway says so plainly to the model and to the client.
2. **GPT Live is told where it is.** From the hub roster the gateway learns the project's folder (`place`). It writes a PROJECT CONTEXT brief into the session instructions: name, `machine/project`, folder (= working directory of everything the backend runs), `arbos://` address, and the rule "answer place questions from this and only this".
3. **GPT Live is given the chat.** The last 12 user and Arbos lines of that project's main chat go in as startup history (`session.input`), behind a developer note saying they are context from before the call.
4. **GPT Live follows the project during the call.** Quiet context appends, at most one every 3 seconds: lines typed in the project chat, Arbos's text replies it did not relay itself, sub-agents (workers) starting and finishing, and the tools they run.

Proof on the live gateway (2026-09-17 19:36Z–19:39Z), with the ArbosLife `demo` project standing in for the Mac (the Mac is not on the hub, so no test can reach it):

| Probe | Result |
|---|---|
| `session.start project: "discord_backups"` | `error project_not_on_hub`, close 4404, no kernel touched |
| `session.start project: {arboslife, demo}` | `session.ready.project_info.place = /home/const/arbos-hub/projects/demo`, `via: hub` |
| Spoken: "Which project and folder are we in right now?" | GPT Live, from the brief, 861 ms after the question ended: "We're in the demo project, in the folder `/home/const/arbos-hub/projects/demo`." The kernel, asked independently, gave the same folder. |
| Typed `text.input` during the call | Reached the demo kernel as a `user` line with `channel: text, device: desktop` |
| Kernel's own transcript | The spoken question filed with `channel: voice, device: desktop` |

Harness: `voice-server/tests/scenarios/bare-project-name-refused.toml` and `hub-attach-reports-own-folder.toml`; every hub scenario now asserts `project_info.place` is the project kernel's folder. 19 of 19 scenarios pass.

## What GPT Live sees: three stores, side by side

There are three separate records. They are not the same thing and none is a copy of another.

| | **Kernel transcript** (`.arbos/agents/root/transcript.jsonl`) | **GPT Live session** (OpenAI's memory of the call) | **What we inject** (gateway → GPT Live) |
|---|---|---|---|
| Lives | On the project's machine, in the project folder | At OpenAI, for the call; gone at `session.close` unless `store: true` (we do not set it) | Sent once per call; becomes part of the session's memory |
| Holds | Every user line (typed or spoken, with `channel` and `device`), every Arbos reply, tool calls, worker events, asks and approvals | The call's audio both ways, its own transcripts of both, our instructions, our appends, delegation ids | See rows below |
| Who reads it | The kernel's model, every attached client (desktop chat, phone chat), the gateway | GPT Live only | GPT Live only |

What reaches GPT Live, item by item, **today (before the fix) vs now (after the fix)**:

| Item | Before | Now |
|---|---|---|
| Project name and machine | No | Yes, in instructions, from the hub roster |
| Project folder (working directory) | No — it guessed or asked the wrong kernel | Yes, in instructions, from the roster's `place` |
| `arbos://` address | No | Yes, in instructions |
| Chat history before the call | **No.** The session started empty | The last 12 user/Arbos lines of the main chat, as `session.input` (cap 600 chars a line, 6,000 total; OpenAI's cap is 128 messages / 8,192 tokens) |
| Lines typed in the chat during the call | No | Yes, as quiet context ("The user typed on the desktop: …") |
| Arbos text replies during the call that it did not speak | No | Yes, as quiet context |
| Delegated answers | Yes, as commentary (it speaks them) | Same; the chat copy of the same answer is not sent again |
| Workers starting/finishing, their tools | No | Yes, as quiet context, coalesced |
| Tool output, diffs, file contents, code | No | **Still no.** Only the kernel's spoken-form answer and one-line tool names |
| Approvals and asks | Spoken to the caller by us; the answer is ours to take | Same. Approvals never go to the provider to decide |
| The caller's audio | Yes (that is the call) | Same |

Where the model's knowledge of place comes from, in order: the instructions brief first; if the brief has no folder (roster gave none), it must delegate. It is told never to invent one.

## Display rules for a client's chat (voice rows)

For the desktop and the phone. The gateway already sends every frame needed.

1. Draw the caller's spoken line from `transcript.final` and Arbos's spoken line from the accumulated `response.transcript` up to `response.done`. Mark them as voice. **Display only. Never send those words to the kernel.** The gateway already did, if they needed the kernel.
2. The kernel's own `user` event with `channel: "voice"` for the same words is the same line. Show one row, not two.
3. Small talk ("hey" → "hey") never reaches the kernel, so it exists only as voice rows. That is correct: the kernel was not woken for it.
4. A typed line in the composer during a call goes to the kernel as usual (`channel: text`; steer if a turn runs). The gateway learns of it from the kernel's event and passes it to GPT Live as quiet context; do not also send it to the gateway.

## What Jacob must do for the Mac

The gateway can only reach a kernel through the hub. The Mac is not registered. On the Mac, create `~/.config/arbos/hub.toml` (mode 0600):

```toml
url = "<the hub's current URL: the hub: line of voice-endpoint.txt>"
machine = "mac"
token_env = "ARBOS_HUB_TOKEN"   # or token = "…"
```

The hub on ArbosLife already has a `[[machine]] name = "mac"` entry (`/home/const/arbos-hub/hub/hub-server.toml`, token in the env var `ARBOS_HUB_TOKEN_MAC` of the hub process). That token value is what goes on the Mac; whoever runs the hub has it. Nothing else on the server needs to change.

Then restart the project's kernel (close and reopen the tab, or restart the desktop). The desktop will then send `mac/discord_backups`, the roster will show the Mac's folder, and the call will attach there. Until then, the call is refused with `project_not_on_hub` and the message above, not answered from ArbosLife.

## Limits and honest caveats

- The chat seed costs a moment at the start of the call, in parallel with the OpenAI connect: session start 1.6 s measured, up from 0.5–0.8 s. It never delays the caller's first word: the seed waits at most 1.5 s and is dropped if the kernel is slow.
- Appends are limited to 500 tokens each by OpenAI, and their acknowledgement does not prove the model used them.
- GPT Live compacts long calls: past about 90% of a 128k-token window it keeps the original instructions and up to 8,192 tokens of recent history. Facts that matter live in the kernel, not in the call.
- What OpenAI keeps: audio and transcripts of the session under their API data policy; we do not enable `store`. This is the trade named in `internal/voice/gpt-live-backend-2026-09-17.md`.

Related: the desktop side of the display rules is with the desktop call worker (`internal/call-mode-voice-rows-and-project-scope.md`).
