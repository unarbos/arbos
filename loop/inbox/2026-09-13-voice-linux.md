# V-01 voice on Linux — QA note (features agent, 2026-09-13)

Branch `cursor/voice-linux-b027`, base `rust`. Benchmark item 1.

## What it does

The desktop talks to the self-hosted speech server over one WebSocket, protocol = `ios/Arbos/Voice/SelfHostedVoiceSession.swift` on `origin/cursor/ios-app-scaffold`: binary frames are PCM16 mono 24 kHz both ways; JSON text frames carry control (`session.start`, `speak`, `interrupt`, `session.end` → `session.ready`, `speech.started/stopped`, `transcript.delta/final`, `response.started`, `response.transcript`, `response.done`, `error`).

- Config (`~/.config/arbos/config.toml`): `voice_url = "wss://host/voice"`, `voice_token = "…"` or `voice_token_env = "ARBOS_VOICE_TOKEN"`. The token goes as `Authorization: Bearer`. No `voice_url` → the old path (macOS Swift helper; "voice dictation only works on this Mac" elsewhere).
- Mic: an external capture process writing raw PCM16 24 kHz to stdout, first found of `pw-record`, `parec`, `arecord`, `rec` (sox), `ffmpeg` (avfoundation on macOS). `ARBOS_VOICE_MIC_CMD` overrides (a shell command; QA feeds a file).
- Playback: `pw-play`, `paplay`, `aplay`, `play`, `ffplay`; `ARBOS_VOICE_PLAYER_CMD` overrides. No player → replies are still transcribed, audio dropped, one notice.
- Dictation: mic press (or Fn) starts; words stream into the composer as `transcript.delta` arrives (ghost text after the caret, as before); release/stop waits ≤ 2 s for `transcript.final`, then inserts the text. Server VAD `speech.stopped` also ends a take.
- Replies: after a voice-started turn ends, the answer text is sent as `speak`; the reply audio plays; `response.transcript` is shown live in the under-composer row. **Barge-in**: the mic press, a new prompt, Stop, or the server's `speech.started` during playback sends `interrupt` and kills the player.
- Status animation: the under-composer row shows `Listening` with a level meter (three bars from the mic's RMS) while recording and `Speaking` with a shimmer while a reply plays; the mic glyph is accent-coloured in both states. Element ids: `composer-voice-status`.

## Attack ideas

1. Server down: mic press → one notice "voice server unreachable (…)", state back to idle within 3 s, no zombie capture process (`pgrep pw-record`).
2. Token wrong (401 on the handshake): message names the URL, never the token.
3. Server sends `transcript.final` before `speech.stopped`, or two finals: text inserted once.
4. Delta text that revises earlier words (server rewrites): the ghost shows the latest delta, finals accumulate; check no duplicate words after stop.
5. Barge-in race: `speak` sent, then `interrupt` 50 ms later: no audio after the interrupt; `response.done` late does not restart anything.
6. 10-minute take: memory stays flat (deltas replace, finals append); no unbounded buffers.
7. Kill the player process externally: playback state clears on the next frame; no panic.
8. `voice_url` with `http://`: rejected with "ws:// or wss://".
9. Two windows / two chats: one client per app; the second mic press while speaking = interrupt then listen.
10. Fn key hold-to-talk path still works on macOS with `voice_url` unset.

## Stub

`internal/voice/voice-stub-server.py` speaks the protocol with scripted transcripts and a tone as reply audio; `internal/voice/README-stub.md` says how to run it. The real endpoint arrives in `internal/features-inbox/<date>-voice-server-endpoint.md`; rerun the same checks against it.
