# “Choosing the next step” — what that line waits on

Jacob on Mac 2036 (#647, slug `~typesafe/jev-latest`) asked why **Choosing the next step** takes so long. He expects Jev to be nearly instant. Shot: [media/jev-latency/01-choosing-next-step.png](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/jev-latency/01-choosing-next-step.png) — typed “what files are in this folder?”, then `Working` `Choosing the next step`.

This is a **text** turn, not a call. The voice-path page is [voice-path-latency.md](voice-path-latency.md). Jev is still not on the gateway.

---

## Words

- **Jev**: TypeSafe System One on OpenRouter. Family alias `~typesafe/jev-latest` (Jev 1.13). About $0.042 per million input tokens. Output is free. Window 32,000. Structured decisions, not chat.
- **Choosing the next step**: the derived status the kernel writes when it starts `jev::ask` (`crates/arbos-engine/src/jev.rs`). The desktop draws it as the live line under `Working`.
- **First-byte cap**: how long `jev::ask` waits for OpenRouter’s first response byte. After that the turn **fails**. The chat model does not run. One try. No retry. No fall-through.

---

## What sits under that label

In order, on every kernel step while Jev is on:

1. Build the situation card from the transcript (goal, last user line, last tool glances). Cheap. No vault. No file bodies.
2. Set the live line to **Choosing the next step**.
3. One OpenRouter chat-completions call to `~typesafe/jev-latest`. JSON in, JSON out. Cap below.
4. Then either run the named tool, invoke the chat model, or end the turn.

The brief file (`.arbos/voice-brief.md`) is packed at **turn end**, not under this line. The gateway does not call Jev.

The line was staying up after step 3. If Jev said `llm`, failed, or timed out, `model_step` ran and the desktop still showed **Choosing the next step** for the whole chat-model wait (seconds). That is the shot. Jacob does not want that fallback. If Jev fails, the turn stops. The person is told.

---

## Waits

| | ms | Where |
| --- | --- | --- |
| **Expected** (healthy Jev) | **200–400** | OpenRouter RTT from the Mac to `~typesafe/jev-latest`. System One. |
| **Allowed before this fix** | **15,000** | `FIRST_BYTE` in `jev.rs` was 15 s — the chat-model first-byte cap, copied onto the router. |
| **Allowed now** | **1,500** | Then the turn fails. The choosing line clears. A failed notice is written. No chat model. |

Derived status is also debounced 300 ms (`STATUS_DEBOUNCE_MS` in `hooks.rs`). That is not the seconds.

`brief::gather` (notes, workers, transcript, git) ran on this hop before the OpenRouter call. It is off that path now. Packing stays at turn end.

---

## What he should see

- A flash of **Choosing the next step**, usually a few hundred milliseconds.
- Then the real work: **Listing …** / the tool name, or **Thinking**.
- `act=llm` is a valid Jev pick. Thinking is that pick. It is not a fail.
- If Jev fails, times out, or returns junk: the line goes away. A failed notice says so. The turn ends. The chat model does **not** answer.
- `jev = false` never shows this line. Today’s one-model loop.

Mac update channel. Do not publish `v0.2.0`. Do not start slices A–G.

---

## Why Jev failed that turn

Jacob already has the 15 s line. He asked which fail it was: a **400** on the old slug, a **timeout**, **parse junk**, or Jev **chose `act=llm`** on “what files are in this folder?”.

**We cannot know from this seat.** That Mac’s kernel log is not here. The four cases leave different marks. One file names them.

### The proving file

On the Mac desktop, kernel stderr is `.arbos/runtime/kernel.out.log` (not the JSON `kernel.log`). Search that file for the turn.

The line the code prints on a real fail (also a failed notice on the transcript):

```
Jev did not choose the next step: .... The turn stopped. The chat model did not run.
```

That print is only for `AskError::Failed` or `AskError::Junk`. A parsed `act=llm` does **not** print it. `act=llm` is a successful decision. The turn then calls the chat model. There is no silent fall-through.

### How to read the parentheses

| What is in `(...)` | What happened |
| --- | --- |
| `400 bad request` and “not a valid model ID” (or the old id `typesafe/jev-latest` without `~`) | OpenRouter refused the slug. The old 400. |
| `no answer from provider: no response headers for 15s` | Timeout. First-byte cap was 15 s on #647. `as_secs()` prints whole seconds. |
| `not a JSON object` | Junk. The body was not one JSON object. |
| `unknown act "..."; want tool, llm, or done` | Junk. JSON parsed; `act` was not `tool`, `llm`, or `done`. |

Any other `Failed(...)` text is a transport or HTTP error. Still a fail, not `act=llm`.

### If that line is missing

Then Jev did **not** fail.

Look at the same turn in the root transcript (`.arbos/agents/root/transcript.jsonl`).

- A tool line whose id starts `jev-` means Jev picked `act=tool` and the kernel ran it.
- No fail notice, and no `jev-*` tool, means Jev picked **`act=llm`**. The chat model ran. That is a valid parse, not a fail.

### Did the code force the LLM on a file-list ask?

No. `first_step` is a flag on the situation card only. The system text says: a **new open-ended** ask (plan, design) with no tools yet → `act=llm`. A file-list ask is mechanical. Jev is supposed to pick `act=tool` and `ls` (or `find`). The kernel does not rewrite that to `llm`.

So we do **not** open a new PR for “always send first-turn file-list asks to the LLM.” The code does not do that.

### What we do not have

Jacob said he sent the rollout from the failing Jev turns. The body is not on this seat.

Checked: this Project transcript, Jev child `bc-1f7d62b4` transcript, the message queue (empty), workspace assets (empty), and store `docs/jev-speed-design-space.md` (226 lines here; his editor shows 422 — a local paste did not sync). No `kernel.out.log` excerpt. No transcript with `jev-*` tools vs `act=llm`.

We do not name a cause. Do not invent 400, timeout, junk, or `act=llm`.
