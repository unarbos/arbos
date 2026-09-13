---
cursor:
  subagentId: "bc-32d10b66-6bef-50c3-9ccf-4350ba54f23a"
---

# For QA: voice server (branch `cursor/voice-server`, PR #56)

## What it is

`voice-server/` in the arbos repo. A Python gateway that gives the phone and desktop one full-duplex speech WebSocket. Two engines behind the same protocol: `duplex` (NVIDIA NemotronLabs VoiceChat 11B, one speech-to-speech model that also calls tools) and `pipeline` (Silero VAD + faster-whisper + Kokoro, any machine). Tools `send_agent`, `agent_status`, `ask_arbos` run against `arbos-kernel serve` over the attach socket. Text channel `text.input` streams text back. Kernel events mirrored as `agent.*`.

## Endpoint

- Live now (interim): `wss://inline-voice-occupations-ultram.trycloudflare.com/ws?token=<VOICE_TOKEN>` (quick tunnel; the hostname changes when the tunnel restarts; read the current one with `deploy/stack.sh status` on the pod). Health: `GET /healthz` on the same host (no auth).
- Planned: `wss://voice.arbos.life/ws?token=…` once a DNS CNAME `voice.arbos.life -> 7012ad90-12a5-4347-acc4-5b1f2760f399.cfargotunnel.com` exists (the vault Cloudflare token cannot write DNS).
- Token: 1Password item `jmldktl7rrc4rw4sm2akej4qne` (vault Arbos, field `credential`): `op item get jmldktl7rrc4rw4sm2akej4qne --vault Arbos --fields label=credential --reveal`. Server side it is `VOICE_TOKEN` in the pod's `/root/arbos-voice/env`.
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
11. Pod loss: Lium pod has a 7 day termination (2026-09-20 05:29 UTC). After that the endpoint dies; `stack.sh up` on any 80 GB GPU box with the same layout brings it back.
