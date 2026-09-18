---
cursor:
  subagentId: "bc-32d10b66-6bef-50c3-9ccf-4350ba54f23a"
---

# The voice path, wait by wait — and whether Jev is on it

Jacob says the voice model feels slower than it should, and asks whether Jev updates are on that path. This page traces the real path as of `main` `f97bb348` (2026-09-18 13:00Z) and the production gateway (running [#594](https://github.com/unarbos/arbos/pull/594)'s tree since 07:33Z). Every wait is named with its file and its cap. Measured numbers come from Jacob's own call at 12:57Z–13:01Z against `mac/.arbos` and from my probes today. Read `docs/gpt-live-context.md` first for what the gateway tells GPT Live; this page is only about time.

## Words used here

- **Cap**: the longest a stage may wait before it gives up or moves on. Most stages finish far under their cap; the cap is what you hit when something is broken.
- **Gateway**: our voice server (`voice-server/`), on the Lium pod. Desktop and phone connect to it; it connects to GPT Live and to a kernel.
- **VAD**: voice activity detection. Decides when the caller started and stopped talking.
- **Join window**: after the caller stops, the gateway waits a little longer before answering, in case the pause was a breath mid-question (#562).
- **Hold**: since #594, the gateway keeps GPT Live's first words back until our own transcript says whether the utterance is small talk or work.
- **Delegation**: GPT Live hands a question to the backend (our gateway → the kernel) and later speaks the answer it gets back.
- **Jev**: a small router model asked by the **kernel** before each mechanical step of a turn (#546, `crates/arbos-engine/src/jev.rs`). Not part of the gateway.

## The short answer

1. **Jev is not on the gateway.** `voice-server/` contains no Jev code (checked: no match for "jev" anywhere under `voice-server/`). Nothing in this PR series puts it there.
2. **Jev is on the path anyway, through Jacob's Mac kernel.** The Mac's kernels (`mac/.arbos`, `mac/Clawdbot`) are built from `a8678ac1` (2026-09-18 12:44Z), which contains #546 (`58105882`). Jev defaults **on** for OpenRouter when a key exists (`crates/arbos-core/src/host.rs`, `jev_enabled`). So every kernel step of every delegated voice question on the Mac first asks Jev. The ArbosLife kernels (`demo`, `phone`, … built `c3247332`, 01:06Z) do **not** contain #546.
3. **What Jev costs today: one failed HTTPS round trip per kernel step.** The default slug `typesafe/jev-latest` is not a valid model on OpenRouter: the API answers `400 "typesafe/jev-latest is not a valid model ID"` (measured from the pod, three calls: 43–117 ms each). `jev::ask` makes one request with no retry and falls through to the configured LLM (`turn.rs` line ~1227, "jev fell through"). So Jev adds roughly **0.05–0.3 s per step** (network from the Mac to OpenRouter), and a "Choosing the next step" status line on the desktop, and does nothing useful. A "what are we working on" turn is one to three steps, so **0.1–1 s** of the answer time. Not the reason the call feels slow, but an extra hop that must not be there. The fix is the kernel owner's: `jev = false` in `config.toml`, an empty `jev_model`, or a slug that exists.
4. **What actually makes it feel slow**, from his call (below): the kernel's answer takes **8–11 s** per question, and since #562 + #594 the gateway waits **1.8 s** after the caller's last word before it even asks (VAD end silence 0.6 s + join window 1.2 s). Plus one routing fault: short conversational lines like "Oh." and "do you think he's a good guy?" are not on the small-talk allowlist, so they go to the kernel and take those 8–11 s too.

## Path 1: `session.start` — from the tap to a live session

| # | Stage | Where | Cap | Measured |
|---|---|---|---|---|
| 1 | Desktop opens the WebSocket to the gateway, sends `session.start`, waits for `session.ready` | `desktop/src/voice_ws.rs` `CONNECT_TIMEOUT` | 6 s | ~0.8 s connect (phone measured 817 ms earlier) |
| 2 | Phone: same, no explicit timeout (URLSession default) | `ios/Arbos/Voice/SelfHostedVoiceSession.swift` | system default | — |
| 3 | Gateway binds the call to the project: hub roster read | `voice_server/base.py` `_roster_lookup`, `httpx.AsyncClient(timeout=8.0)` | 8 s | < 0.2 s (pod → ArbosLife hub over the ssh forward) |
| 4 | Attach to the project's kernel through the hub (`hello` frame back) | `base.py` `_attach_project`, `wait_for(kernel.connect(), 15)`; `kernel.py` `hello_timeout=15` | 15 s | ~0.3–1 s (Mac kernel via hub) |
| 5 | Local attach variant (folder on the gateway host) | `base.py` `_attach_local`, `wait_for(..., 10)` | 10 s | n/a for the Mac |
| 6 | `session.ready` sent | `base.py` `_send_ready` | none (immediate) | at ~1 s |
| 7 | On the first audio frame: open the GPT Live session | `voice_server/openai_live.py` `_ensure_upstream`, `websockets.connect(open_timeout=20)`, then `session.started` awaited with `wait_for(recv, 20)` | 20 s + 20 s | 464–782 ms alone |
| 8 | In parallel with 7: read the project's chat for the history seed | `openai_live.py` `_history_seed`, `transcript_tail` `wait_for(..., 4.0)`; the connect waits for it at most `wait_for(seed_task, 1.5)` | 4 s read, 1.5 s wait | session ready 1,067–2,205 ms with the seed (Jacob's call: **1,797 ms**, 41 lines) |
| 9 | Engines warm-up (Whisper, Kokoro) | `voice_server/engines.py` `warm_up` | once at server start | 0.6 s at boot, not per call |

Total to a listening session: about **1 s** to `session.ready`, about **2 s** until GPT Live can answer. Nothing on this path waits on a kernel turn, and Jev is not involved.

## Path 2: "what are we working on right now?" — from the last word to the first word back

| # | Stage | Where | Cap | Measured |
|---|---|---|---|---|
| 1 | Mic frames to the gateway (40 ms frames) | desktop `voice_ws.rs` / phone `AudioEngine.swift` | — | network only |
| 2 | Echo gate on the uplink | `voice_server/echo.py` via `base.py` `_gate_uplink` | none (per frame) | µs |
| 3 | VAD hears the end of speech | `voice_server/duplex.py` `_uplink`; `base.py` `Tuning.end_silence_ms` | **600 ms** of quiet | 600 ms |
| 4 | Whisper transcribes the segment | `voice_server/asr.py` `FasterWhisperASR` (large-v3-turbo, GPU) | none | 42–80 ms |
| 5 | Join window: wait in case the pause was a breath | `duplex.py` `_joined`; `Tuning.join_ms` (#562) | **1,200 ms** | 1,200 ms (the log's "asr 1205ms" is 4 + 5 together) |
| 6 | Hold: GPT Live's first words kept back until 5 decides (#594) | `openai_live.py` `HOLD_MODEL_WHILE_DECIDING`; `duplex.py` `model_hold` | ~3 s of audio, then the buffer trims | costs nothing extra: it runs inside 3–5 |
| 7 | Route: small talk → release; work → kernel, at once | `openai_live.py` `_route_final`, `voice_server/routing.py` `is_small_talk` | none | µs |
| 8 | Kernel turn | `openai_live.py` `_delegate`, `kernel.turn(question, timeout=120)`; `voice_server/kernel.py` | **120 s** | **8.0–11.2 s** in Jacob's call (1.0 s on a quiet demo kernel, 24 s when it did real work) |
| 8a | — inside the kernel, per step: Jev asked first (Mac kernels only) | `crates/arbos-engine/src/turn.rs` ~1165, `jev.rs` `ask`, `FIRST_BYTE` | 15 s | fails at once with 400: 0.05–0.3 s per step |
| 8b | — inside the kernel, per step: the configured LLM, then tools | `turn.rs`; provider first-byte/idle caps in `provider.rs` | provider caps | the bulk of 8 |
| 9 | Answer to GPT Live as commentary | `openai_live.py` `_append("session.commentary.append")` | none | ms |
| 10 | GPT Live speaks the commentary; first audio frame back | OpenAI, then `duplex.py` `_on_model_audio` → client | none | ~0.2–0.5 s after 9 |
| 11 | If the model was mid-sentence with its own words: stay muted until its transcript reaches the answer's words or it pauses | `openai_live.py` `_kernel_spoke`, `_heard_answer_words`, `UNMUTE_WATCH_S` | 6 s watchdog | fired once in his call ("never paused; unmuting") |
| 12 | Client plays the frames | desktop `voice_ws.rs` / phone `AudioEngine.swift` | — | buffer ≤ `Tuning.max_lead_ms` 1,500 ms ahead |

**Sum for a work question:** 1.8 s of gateway waiting (3 + 5) + the kernel's 8–11 s + ~0.3 s to the first spoken word ≈ **10–13 s**. His log agrees: "first reply audio 5,952–21,083 ms after speech.stopped", most runs 10–13 s.

**Sum for small talk:** 1.8 s (3 + 5) + GPT Live's own latency (~0.5 s) ≈ **1.6–2.3 s** to the first word. Measured 1,598 ms this morning; it was ~560 ms before #562's join window and #594's hold.

Waits that are **not** on the critical path (they run beside it): `ACK_GRACE_S` 2.5 s (only the "let me check without delegating" watch), `CONTEXT_MIN_GAP_S` 3 s (quiet context appends), `TAIL_S` 0.7 s (closes a spoken burst → `response.done`), desktop `ECHO_TAIL` 400 ms, `ACTIVITY_STALE` 12 s (working sound), `FINAL_WAIT` 700 ms (dictation only).

## What is extra on the path, and how long

| Extra | Where it came from | Cost | Whose to change |
|---|---|---|---|
| Jev round trip before each kernel step, failing with 400 | #546 in the Mac kernel build; default on for OpenRouter | 0.05–0.3 s per step, 0.1–1 s per answer, plus a "Choosing the next step" flicker | Kernel owner: `jev = false` / empty `jev_model` in the Mac's `config.toml`, or a real slug. Not the gateway. |
| Join window | #562, `Tuning.join_ms = 1200` | 1.2 s on every utterance, before anything is asked | Gateway (mine). It bought "one breath, one answer". Could be shortened (e.g. 800 ms) or made adaptive; not done here. |
| Hold of the model's first word | #594 | 0 s extra on top of the join for work questions; on small talk, the first word now waits for 3 + 5 | Gateway (mine). The priced cost of choice B. |
| Over-routing to the kernel | `routing.py` allowlist is narrow | "Oh." (576 ms of audio), "what are the types of things you can do?", "do you think he's a good guy?" went to the kernel: 8–11 s each instead of GPT Live's ~0.5 s | Gateway (mine). A routing fix, not a latency knob; noted, not changed here. |
| History seed at session start | #500 (41 lines) | ≤ 1.5 s, once, beside the connect | Gateway. Fine. |

## What is not on the path

- No Jev in `voice-server/`. No Jev in the GPT Live session. No Jev on the ArbosLife kernels (their build predates #546).
- No OpenRouter hop in the gateway for a GPT Live call: `--reply openrouter` is the pipeline engine's reply hop and is idle under `--engine openai`; the narrator model (`google/gemini-2.5-flash`) is used only for highlights on the hosted engine.
- No Kokoro TTS on the spoken path: GPT Live's own voice speaks everything, the kernel's answers included.

## Not done, on instruction

Jev stays off the gateway. `v0.2.0` is not published. Jev slices A–G are not started. The two gateway-side items above (join length, the allowlist) are findings for a decision, not changes.
