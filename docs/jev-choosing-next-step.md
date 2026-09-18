# “Choosing the next step” — what that line waits on

Jacob on Mac 2036 (#647, slug `~typesafe/jev-latest`) asked why **Choosing the next step** takes so long. He expects Jev to be nearly instant. Shot: [media/jev-latency/01-choosing-next-step.png](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/jev-latency/01-choosing-next-step.png) — typed “what files are in this folder?”, then `Working` `Choosing the next step`.

This is a **text** turn, not a call. The voice-path page is [voice-path-latency.md](voice-path-latency.md). Jev is still not on the gateway.

---

## Words

- **Jev**: TypeSafe System One on OpenRouter. Family alias `~typesafe/jev-latest` (Jev 1.13). About $0.042 per million input tokens. Output is free. Window 32,000. Structured decisions, not chat.
- **Choosing the next step**: the derived status the kernel writes when it starts `jev::ask` (`crates/arbos-engine/src/jev.rs`). The desktop draws it as the live line under `Working`.
- **First-byte cap**: how long `jev::ask` waits for OpenRouter’s first response byte. After that it falls through to the chat model. One try. No retry.

---

## What sits under that label

In order, on every kernel step while Jev is on:

1. Build the situation card from the transcript (goal, last user line, last tool glances). Cheap. No vault. No file bodies.
2. Set the live line to **Choosing the next step**.
3. One OpenRouter chat-completions call to `~typesafe/jev-latest`. JSON in, JSON out. Cap below.
4. Then either run the named tool, invoke the chat model, or end the turn.

The brief file (`.arbos/voice-brief.md`) is packed at **turn end**, not under this line. The gateway does not call Jev.

The line was staying up after step 3. If Jev said `llm`, failed, or timed out, `model_step` ran and the desktop still showed **Choosing the next step** for the whole chat-model wait (seconds). That is the shot.

---

## Waits

| | ms | Where |
| --- | --- | --- |
| **Expected** (healthy Jev) | **200–400** | OpenRouter RTT from the Mac to `~typesafe/jev-latest`. System One. |
| **Allowed before this fix** | **15,000** | `FIRST_BYTE` in `jev.rs` was 15 s — the chat-model first-byte cap, copied onto the router. |
| **Allowed after this fix** | **1,500** | Then fall through. The choosing line is cleared so the LLM or the tool owns the headline. |

Derived status is also debounced 300 ms (`STATUS_DEBOUNCE_MS` in `hooks.rs`). That is not the seconds.

`brief::gather` (notes, workers, transcript, git) ran on this hop before the OpenRouter call. It is off that path now. Packing stays at turn end.

---

## What he should see after the fix

- A flash of **Choosing the next step**, at most about **1.5 s**, usually a few hundred milliseconds.
- Then the real work: **Listing …** / the tool name, or **Thinking**, not that line for the rest of the turn.
- If Jev is silent past 1.5 s, today’s one-model loop. The line goes away. The chat model answers.
- `jev = false` never shows this line.

Mac update channel. Do not publish `v0.2.0`. Do not start slices A–G.
