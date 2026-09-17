---
cursor:
  subagentId: "bc-32d10b66-6bef-50c3-9ccf-4350ba54f23a"
---

# For QA: voice server (branch `cursor/voice-server`, PR #56)

## What it is

`voice-server/` in the arbos repo. A Python gateway that gives the phone and desktop one full-duplex speech WebSocket. Two engines behind the same protocol: `duplex` (NVIDIA NemotronLabs VoiceChat 11B, one speech-to-speech model that also calls tools) and `pipeline` (Silero VAD + faster-whisper + Kokoro, any machine). Tools `send_agent`, `agent_status`, `ask_arbos` run against `arbos-kernel serve` over the attach socket. Text channel `text.input` streams text back. Kernel events mirrored as `agent.*`.

## Endpoint

- Live now (interim): `wss://inline-voice-occupations-ultram.trycloudflare.com/ws?token=<VOICE_TOKEN>` (quick tunnel; the hostname changes when the tunnel restarts; read the current one with `deploy/stack.sh status` on the pod). Health: `GET /healthz` on the same host (no auth).
- Planned: `wss://voice-api.arbos.life/ws?token=…` once a DNS CNAME `voice-api.arbos.life -> 7012ad90-12a5-4347-acc4-5b1f2760f399.cfargotunnel.com` exists (the vault Cloudflare token cannot write DNS).
- Token: 1Password item `jmldktl7rrc4rw4sm2akej4qne` (vault Arbos, field `credential`): `op item get jmldktl7rrc4rw4sm2akej4qne --vault Arbos --fields label=credential --reveal`. Server side it is `VOICE_TOKEN` in the pod's `/root/arbos-voice/env`.
- Kernel behind the tools: the phone kernel (`arbos-kernel` 0.2.0 with #72 + #74, user `arbos-phone`, `/home/arbos-phone/arbos-phone`, port 7788), see `internal/features-inbox/2026-09-13-phone-kernel-endpoint.md`. The earlier root-run `rust` kernel on the pod (which had the kill(-1) bug) is stopped.
- Host: Lium pod `arbos-voice-duplex`, RTX PRO 6000 Blackwell 96 GB, `ssh root@216.243.220.25 -p 40300` with the agents' SSH key. Everything under `/root/arbos-voice` (`src/`, `.venv/`, `kernel/place`, `logs/`, `env`, `tunnel.token`) plus the 44 GB model repo at `/tmp/arbos-voice-nim` (Docker-in-Docker cannot bind-mount `/root`). `src/deploy/stack.sh status|logs|down|up`.

## How to exercise

```bash
cd voice-server && uv venv .venv && source .venv/bin/activate && uv pip install -e .
python client.py "$URL" --wav q.wav --reply                       # one question, server answers (duplex)
python client.py "$URL" --wav q.wav --reply --barge-in q2.wav      # talk over the reply
python client.py "$URL" --wav q.wav --reply --text "hi" --agent-wav ask-for-agent.wav   # text channel + send_agent + agent.done
```

`client.py` prints every event with timestamps and a latency table; `out/metrics.json` has the numbers. Make WAVs with Kokoro (see `deploy/fetch-models.sh` for the weights) or record real speech; 16-bit PCM any rate.

Local pipeline run without a GPU: `VOICE_TOKEN=t deploy/run.sh --engine pipeline` then `ws://127.0.0.1:8765/ws?token=t`.

## What could break (please try)

1. Silence chatter: in long silences the duplex model sometimes invents follow-up work ("fix the bug", "email the project manager"). The gateway refuses a second `send_agent` until the user has spoken again (`refused send_agent` in the log) and the prompt tells it to wait quietly. Try 60 s of silence after a dispatch; count `tool.call` events. A real mic's noise floor may behave differently from digital silence.
2. Auto-approve: the gateway answers every kernel `ask` that starts with `allow ` with approve (`--no-auto-approve` turns it off). A voice request like "delete everything in the repo" goes through. Test the blast radius of `send_agent` with a dangerous task.
3. Turn gating: `response.started/done` are derived from the model's audio level (RMS > 0.004, 0.7 s tail). Quiet voices or noisy output could flap. Watch for started/done pairs with no audio between them.
4. Barge-in tail: after `interrupt` the gateway mutes model audio until it goes quiet. If the model never goes quiet (keeps talking over the user) the mute lifts after 0.7 s of silence only; check `late_audio_frames`.
5. ASR misses the first syllable of an utterance that starts hot ("Hey Arbos" -> "y arbos"). Real speech with a breath before it is fine; try clipped starts.
6. Kernel disconnect: kill `arbos-kernel` while a call is up. Tools should answer "The Arbos kernel is not connected" and the call should continue; `agent.*` events stop. Reconnect is not implemented.
7. Two callers at once: the model container serves several streams, the kernel is shared, so two callers' agents mix in `agent_status`. Report-back goes to the caller who dispatched.
8. Wrong sample rate / stereo / 8 kHz in `session.start`: resampling is stateful and untested outside 24 kHz and 16 kHz.
9. Token in the URL ends up in Cloudflare and gateway logs (the gateway does not log URLs; Cloudflare does). Rotate by editing `env` and restarting `voice`.
10. Long calls: the duplex container's session has no idle timeout that I found; the gateway pings every 20 s. Try a 30 minute call.
11. Supervisor: `deploy/supervise.sh` restarts dead tmux sessions, the gateway after 3 failed `/healthz`, the model container after 10 min unhealthy, and the quick tunnel after 3 failed public checks (new URL gets published). Kill things and watch `logs/supervisor.log`.
12. Echo gate (new, 2026-09-13 14:20 UTC): while reply audio is playing the gateway cross-correlates every uplink frame with the reply audio of the last 3 s and silences matches; frames with clearly more energy than the predicted echo (two in a row) pass, then everything passes for 0.4 s so the VAD/model sees one run. Tested with `client.py --loopback 0.3|0.5|0.8` (pipeline) and `--loopback 0.6` (duplex): no self-transcripts, barge-in 0.4-0.5 s (pipeline) / 1.8 s (duplex). Try: real phone on speaker at full volume; music or a second voice in the room while Arbos talks (should pass as speech); whispering over a loud reply (may be gated); `--no-echo-gate` to compare. Server log prints `echo gate: {frames, echo, passed_over_echo}` per session.
13. Output level (2026-09-13 15:00 UTC): reply audio from both engines is now normalised toward -3 dBFS peak with a soft limiter (`--out-target-dbfs`, `--no-normalize`). Measured duplex reply: before -17 dBFS peak / -35.5 RMS, after -3.0 / -18 to -20; pipeline TTS -4.7/-22 -> -3.0/-15. Try: very loud model output (should not clip; limiter bends above 0.6), a whisper-quiet reply (gain caps at +24 dB), and a `speak` right after a model reply (separate gain states, no pumping). Known limit: with the louder replies, a speakerphone echo at more than ~0.4x the reply level blocks barge-in in duplex until the reply pauses (`--echo-margin`, default 1.6, lower lets more through and more echo in).
14. Call mode (2026-09-16): duplex turns that are not small talk are answered by the kernel (Kokoro voice); the model only handles greetings etc. Try: routing of project questions vs small talk (`voice_server/routing.py`; `user (kernel|model|fragment)` in the log), a question with a long mid-sentence pause (gateway holds up to 4 s for VAD quiet and merges fragments), barge-in 1.5 s into a 3 s kernel answer (expect ~0.4 s to `response.done interrupted`), speakerphone echo at high volume (the gate can pass a loud Kokoro echo as talk-over: look for kernel turns whose text is Arbos's own words), headset route (`client.speaking route=airpods` disables the gate). Details: `internal/features-inbox/2026-09-16-voice-call-mode-kernel-answers.md`.
15. Pod loss: Lium pod has a 7 day termination (2026-09-20 05:29 UTC). After that the endpoint dies; `stack.sh up` on any 80 GB GPU box with the same layout brings it back.
