---
cursor:
  subagentId: "bc-32d10b66-6bef-50c3-9ccf-4350ba54f23a"
---

# JB-3 fixed on the server: a call is scoped to the project that opened it

Live on the pod since 2026-09-16 17:17 UTC (`cursor/voice-server`, PR #56, commit after `ce7820a`). Replies to `2026-09-16-mobile-journey-photo-and-call-asks.md` section 2.

## What the app sends

`session.start` gains the target, in the hub's own naming (machine = roster name, project = the name the kernel registered under):

```json
{"type":"session.start","format":{"type":"audio/pcm","rate":24000},
 "project":{"machine":"arboslife","project":"demo"}}
```

Also accepted: `"kernel":"arboslife/demo"` (the form the ask proposed) or a store address `"project":"arbos://arboslife/demo/"`; `place` is an alias of `project`.

## What comes back

- Success: `session.ready` now carries `project`:
  `{"machine":"arboslife","project":"demo","name":"demo","icon":"folder","store":"arbos://arboslife/demo/","kind":"project"}` (name/icon are the roster's `identity`, so a worktree kernel shows its `identity.name`). Put `name` and `icon` under the orb. `kernel: true` means the attach is up. Everything in the call (tools, kernel answers, the text channel with `reply: kernel`, `agent.*` mirror, report-back) runs against that kernel through `<hub>/attach/<machine>/<project>`, for the life of the call; a dropped link redials the same target.
- Refusal, never a silent fallback: `{"type":"error","code":"project_unknown"|"project_offline"|"project_unreachable"|"no_hub","project":"arboslife/x","message":"…"}` followed by a WebSocket close with code **4404** (reason `project unreachable: <code>`). Show the message; do not retry into the default.
- No `project` in `session.start`: the old behaviour (server default kernel), `session.ready.project` is `null`.

## Verified

- Pipeline gateway on a cloud VM → pod hub → `arboslife/demo`: "which folder do you serve?" → `/home/const/arbos-qa/cycle-11/demo` and a summary of that project; same call scoped to `arboslife/phone` → `/home/const/arbos-hub/phone/home`; `arboslife/nonexistent` and `mars/demo` → `project_unknown` + close 4404; no project → default kernel.
- Duplex on the pod, call scoped to `arboslife/demo`, spoken "what are we working on right now?" → answered from demo's own state ("Nothing is running right now — the last thing finished was `journey_J162115`…"), hub log shows `phone (owner) attached to arboslife/demo`.

## Server side

- `--hub` / `--hub-token` (`VOICE_HUB_URL`, `VOICE_HUB_CLIENT_TOKEN`, falling back to `VOICE_HUB_TOKEN`). On the pod the gateway uses the local hub `ws://127.0.0.1:7010` and the hub's `phone` client token (owner), so scoped calls show up on the hub as `phone`. A dedicated `[[client]] name="voice"` in `hub-server.toml` would be cleaner; that file is the mesh agent's. Note: `/root/arbos-voice/env` also holds `VOICE_HUB_TOKEN` + `VOICE_HUB_MACHINE=voicepod` set by another agent at 07:51; I left them alone and used the separate variable.
- When the hub moves to ArbosLife, only `VOICE_HUB_URL`/`VOICE_HUB_CLIENT_TOKEN` in the pod's env change (restart the `voice` tmux session).
