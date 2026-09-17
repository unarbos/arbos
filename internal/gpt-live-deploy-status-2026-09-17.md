---
cursor:
  subagentId: "bc-22d20d79-de36-524a-ae31-3e1c44c03b98"
---

# GPT-Live session context: deployed to the production gateway

Companion to `internal/gpt-live-status.md` (the #500 author's report; not
edited, per the ownership rule). Mesh worker, 2026-09-17 21:16 UTC.

**Deployed:** `main` `c40f39b0` (tip after [#500](https://github.com/unarbos/arbos/pull/500)
`7303f9ea`) on the production voice gateway, the pod `arbos-voice-duplex`
(`/root/arbos-voice/src`, tmux session `voice`, port 8765, behind the quick
tunnel the phone and desktop dial and `voice-api.arbos.life`).

**What changed on the pod:** exactly the three gateway files #500 touched —
`voice_server/base.py`, `voice_server/protocol.py`, `voice_server/openai_live.py`
— plus its tests (`tests/mock_openai.py`, `tests/run.py`,
`tests/scenarios/live-session-context.toml`). Before the copy the pod's tree
was byte-identical to `main` before #500 with no local edits (checked file by
file against `7303f9ea^`), so nothing else moved. Hashes on the pod now equal
`origin/main`'s for all three files.

**How:** files staged on disk at 21:13 while a call was live (session 78202,
a delegation answered at 21:13:24); the running process was not touched. The
call ended at 21:14:01; after 20 s with no client on 8765 the `voice` tmux
session was stopped at 21:14:4x and the stack's own supervisor brought it back
with the same command (`stack.sh up`) at 21:14:45. `/healthz` 200 two seconds
later; engines ready in 3.1 s (`engine=openai duplex=openai/gpt-live-1
asr=faster-whisper/large-v3-turbo tts=kokoro`), attached to the ArbosLife
phone kernel over its tunnel, `auth=token`. From outside: 200 via the quick
tunnel and via `voice-api.arbos.life`. Downtime for callers: about 30 s, with
no call in progress.

**Verified on the pod:** the running interpreter loads
`/root/arbos-voice/src/voice_server/openai_live.py` (editable install), which
now carries the three stores (PROJECT IDENTITY, ON-SCREEN CHAT, WORKERS AND
ACTIVITY); `python -m tests.run live-session-context` against the mock
upstream: **PASS, 21 checks, 3.4 s**, on the production tree.

**Not done, by instruction:** `v0.2.0` was not published. **Not mine:** the
desktop half (`call_context` sending 40 rows) ships with the desktop build;
`hub.toml` on Jacob's Mac for a Mac call. **Rollback:** the pre-#500 files are
`git show 7303f9ea^:voice-server/voice_server/{base,protocol,openai_live}.py`;
copy them over and stop the `voice` session again — the supervisor restarts it
in under 15 s.
