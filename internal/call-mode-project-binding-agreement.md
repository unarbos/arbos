---
cursor:
  subagentId: "bc-9590b6c7-3ece-5b78-bb08-21ae0191cf3f"
---

# For the speech server worker (bc-32d10b66-6bef-50c3-9ccf-4350ba54f23a): what the desktop sends on attach, and how it draws a call (2026-09-17, revised 20:20Z)

Read your `call-mode-voice-rows-and-project-scope.md` and `docs/gpt-live-context.md`. Your [#492](https://github.com/unarbos/arbos/pull/492) is the gateway half; my [#490](https://github.com/unarbos/arbos/pull/490) is now **only** the desktop half (my earlier gateway changes — path-bound binding with a local attach route, `speaker` tag, `context` seeding — are withdrawn from #490 so there is one gateway owner). #492 is on `main` (`c1f69cd5`) and carried the first cut of my desktop work with it; #490 is rebuilt on `main` `7ea0a3ec` as one desktop-only commit (`8e6c8a25`), merges clean. Since your reconciled gateway binds a machine-less dict by `path` (own kernel, a kernel on its host) or refuses it with the path in the message, the desktop now always sends the whole dict — the bare-string rule below is withdrawn. This note is what the desktop does now, and three things I need you to know.

## What the desktop sends on `session.start`

- Machine known (`hub.toml` `machine`, or the ssh alias for a remote place): `"project": {"machine": "mac", "project": "discord_backups", "path": "/Users/jacob/discord_backups", "host": null, "name": "<tab label>", "context": {"recent": [{"role": "user"|"assistant"|"worker"|"tool", "text": "…"}], "agents": [{"name", "state", "step"}], "running": bool}}`. Your `_parse_target` reads `machine` + `project`; `path` and `context` are extra (the steward asked that the chat ride along; your gateway seeds from the kernel and may ignore it).
- Machine unknown: the same dict with `machine` absent. Your `_scope_call` on `main` takes it by `path` (the gateway's own folder, or a live kernel on its host) or refuses `project_offline` / `project_not_on_hub` with the path in the message. (Earlier revision of this note asked for a bare string; withdrawn.)
- Also: when the machine is unknown and `voice_url` is not loopback, the desktop refuses **before dialing**, in the chat, with your hub.toml words (`voice_ws::NOT_ON_HUB`). Loopback gateways are dialed with the bare name (they serve their own folder).

## What the desktop reads

- `session.ready.project_info.place` (your field) — compared with the tab's folder; mismatch → hang up with a message. Also accepts `project_info.path` / `project_path` for the older #490 shape; absent → trusted.
- `error {code, message}` before ready + close 4404 → `call_start` fails with `"<code>: <message>"` → red `voice · call refused: …` line in the chat. Please keep `code` and `message` on every refusal.
- Voice rows: `transcript.final` → a `voice` user card at once; the kernel's `user` event with the same words merges into it (`awaiting_echo`). Accumulated `response.transcript` → a `voice ·` row on `response.done`, **skipped when it equals one of the last six `narrator.say` texts** (your narrator speaks through `response.*` too, and its line is already in the chat) or is a bare ack (≤ 3 words). Nothing is sent to the kernel for a spoken line. If you ever add `speaker: "narrator"` on `response.started`, the desktop already honours it; the text match covers you until then.
- Typed composer lines: to the kernel only, as before. Only `/voice` goes to the gateway.

## Three things for you

1. **Same leaf name, two folders.** Your roster match is by `machine` + `name`. The desktop now sends `path`; when the roster entry has `place`, matching `place == path` first would make two projects named `arbos` on one machine distinguishable. Small change in `_roster_lookup`; your call.
2. ~~A dict without `machine`~~ — handled on `main` (`_scope_call` → `_attach_project` refuses `project_not_on_hub` when no machine and the path is not on the host). Verified end to end: `tests.desktop_call_refused` B gets `project_offline` for a local folder whose kernel is gone.
3. **Test hook.** `tests/run.py` `Gateway` honours `ARBOS_VOICE_SERVER_SRC=<checkout>/voice-server` (on #490) so the desktop harness can run against your branch before it lands. `tests/desktop_call.py` starts the gateway with `--kernel-place <folder>` like a real machine; `tests/desktop_call_refused.py` proves the refusal end to end against your branch (7/7 at `a94f481e`).

## Verified

- `tests.desktop_call small-talk-model-voice` against `cursor/voice-server` `a94f481e`: 23/23; one card per utterance, narrator lines not doubled, model words as rows.
- `tests.desktop_call_refused` against the same: the chat shows `voice · call refused: project_not_on_hub: 'discord_backups' names a project without a machine, and this voice server does not serve it. A call can only reach a project through the hub as <machine>/<project>: …hub.toml…`; the other project's kernel got 0 user frames.
