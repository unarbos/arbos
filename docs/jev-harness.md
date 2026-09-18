# Jev in the Arbos harness

Jacob wants Arbos to be fast when he wakes up. He always has an OpenRouter key. He always has a Jev-like model. Do not wait for him to choose the split.

## What Jev is

OpenRouter slug: `typesafe/jev-latest` (page: [Jev Latest](https://openrouter.ai/~typesafe/jev-latest)).

- Family alias: always the newest Jev.
- Cost: about $0.042 per million input tokens. Output is free.
- Window: 32,000 tokens.
- Shape: text in, **structured decisions** out. It is not the chat model.

The normal LLM stays the model in `config.toml` (`model`). That model writes, plans, and talks.

## The split (default, no ask)

**Jev decides the next mechanical move.** The LLM does work that needs language.

Jev handles, by default:

- Which tool to run next, and with what arguments, when the next move is mechanical (read, grep, glob, list, run a test, git status).
- Whether the turn is done.
- Whether the tree already does what the request asks (`no change:`), after a recorded `repro:true` run.
- Whether to compact or fold (yes/no only).
- Whether a child should spawn, and with which short title.

The LLM handles:

- The first reply on a new open-ended ask (plan, design, “what should we do”).
- Writing or rewriting file contents (`write`, `edit`) when the change is not a one-line mechanical fix.
- Anything the user will read: `say`, ask cards, explanations, review comments.
- A step Jev cannot parse, refuses, or marks `need_llm`.

Jev is the default for “what next”. The LLM is the exception.

## One turn, in order

1. Build a **situation card** (goal, last user line, last 3 tool results, files already touched). Keep it under Jev’s 32k window. Never send the full transcript.
2. Ask Jev for one JSON object:

```json
{
  "act": "tool" | "llm" | "done",
  "tool": "grep",
  "args": {},
  "why": "short"
}
```

3. `tool` — run that tool. Do not call the LLM this step. Then loop.
4. `llm` — one normal `model_step` with tools, as today. Then loop.
5. `done` — end the turn. If there is no user-visible sentence yet, one short LLM `say` so the window is not blank.
6. If Jev errors, times out, or returns junk: **fall through to the LLM**. The turn still finishes.

Barge-in, cancel, and spend caps stay on the kernel. Jev cannot override them.

## Config

In `config.toml`, all optional:

- `jev_model` — default `typesafe/jev-latest`. Empty string turns Jev off.
- `jev` — default `true` when the provider is OpenRouter and a key exists. `false` keeps today’s one-model loop.

No new key. The OpenRouter key already on the host is enough.

A `jev-like` model means: OpenRouter, structured JSON out, cheap, ≤32k. If the slug changes, only `jev_model` changes.

## What the window shows

- A Jev tool step looks like any other tool step. Do not add a second brain in the UI.
- If we show a model name, show `jev` on those steps so a stall is honest.
- Voice and Live stay on the LLM. Jev does not speak.

## What we will not do tonight

- Train or host Jev.
- Send vault keys or whole files into the situation card.
- Replace fallbacks (`fallback_models`). Jev is a router, not a fallback.
- Block a turn on a human choice about the split.

## Done when

- A turn with an OpenRouter key uses Jev for mechanical tool steps and the configured LLM for writing and talking.
- Killing Jev (bad slug, timeout) still completes the turn on the LLM.
- Tests pin: parse of the three `act` values; fall-through on junk; situation card stays under 32k; `jev = false` is the old loop.
- PR on `unarbos/arbos`. Do not publish `v0.2.0`.
