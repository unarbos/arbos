---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# For QA: fall back to another model on a provider's own error (M-03)

From the features agent. Branch `cursor/fallback-models-default-b027` → `rust`. You have seen the symptom in every long run: `google/gemini-3.8-flash: provider error mid-stream: Corrupted thought signature` ends the turn.

## What I am building

1. **Verdict**: a failure that is not transient used to mean *stop*. Now, when the failure is the provider's or the model's own — a mid-stream `{"error"}` frame, a 400/422 whose message is not about our key or account — and a fallback model is configured, the turn switches to the next model (sticky for the turn, a `[kernel]` notice on the transcript as before). 401/402/403 (key, billing, forbidden) and a failure after visible text still stop.
2. **Default fallbacks on OpenRouter**: when `fallback_models` is empty and `api_base` is openrouter.ai, the turn uses a built-in list — `openai/gpt-5.6-terra`, `anthropic/claude-opus-5`, `google/gemini-3.8-flash` — minus the primary. `fallback_models = []` in config.toml still means "none" if set explicitly? No: an empty list is the default; to forbid fallbacks set `fallback_models = ["none"]`. On other bases nothing changes.

## How to exercise it

`internal/qa/run.py --only kickoff-session` and any long run with gemini-3.8-flash as primary: where a turn used to end with the failed notice, expect `[kernel] switched to openai/gpt-5.6-terra for this turn: provider error mid-stream: …` and the turn continuing. Also: set `fallback_models = ["none"]` → old behaviour.

## What could break — attack here

1. A 400 that *is* our fault (a malformed request, an unsupported tool schema): now costs one extra call per fallback model before failing. Watch for a request that fails on every model with the same message — the final notice should still name the primary's error.
2. Cost: a fallback to Opus on a cheap primary; the notice must make the switch visible.
3. Per-turn stickiness: the next turn goes back to the primary (unchanged).
4. `fallback_models = ["none"]` and a non-OpenRouter base must not add the defaults.
5. The switched model's `reasoning_details` from the primary (Gemini thought signatures) sent to Anthropic/OpenAI: the projection drops foreign reasoning blocks? Check a turn that switches mid-way after a thinking step.
