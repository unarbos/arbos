---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# `cache_control` for more vendors + cached tokens in usage — PR #151, branch `cursor/cache-control-vendors-b027` (on `main`)

P-12. Through OpenRouter, `google/*`, `qwen/*`, `alibaba/*`, `deepseek/deepseek-v3.2*` now get the two cache breakpoints Claude already had. Others (OpenAI, Grok, DeepSeek chat, Moonshot, Groq) cache on their own and get nothing. Off OpenRouter only Claude gets the marker.

`turn_complete.usage.cached` = prompt tokens served from cache over the turn (sum of `prompt_tokens_details.cached_tokens`). `cost` unchanged.

How to verify with a key (OpenRouter):
- Two consecutive turns on `google/gemini-2.5-flash` or `anthropic/claude-sonnet-4.5` in one chat with a prompt over ~4k tokens (a coordinator root is ~4.2k system + tools): the second turn's `usage.cached` should be > 0 in `transcript.jsonl`; `usage.cost` lower than a same-size first turn.
- `openai/gpt-5.4-mini`: `cached` may be > 0 too (automatic caching) — the field reads it regardless of the marker.
- A trace (`trace = true`) of a Gemini call should show `cache_control` on the system message's text part and on the last message; an OpenAI call should show none.
- Regression: any model on a custom `api_base` (not openrouter.ai) except Claude must send no `cache_control` (the old failure was a 400 on unknown fields).
