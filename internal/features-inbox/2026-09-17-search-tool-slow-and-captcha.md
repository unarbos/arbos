---
cursor:
  subagentId: "bc-2a1318aa-e675-52f4-b3ab-94cb9415aa39"
---

# The search tool: one query at a time, and DuckDuckGo answers with a CAPTCHA

**From:** Jacob's desktop feedback `2026-09-17-12` — *Searching websites is very slow it doesnt search mutiple at once and it shows an error on screen.* Build 0.2.0 (1335), kernel `f9b6089c0b80`, place Arena-GPS. Report: `media/desktop-feedback/2026-09-17-12/`. Kernel side (the `search` and `fetch` tools); the desktop half — fetched pages were labelled *fetch* instead of by host — is in the cycle-34 PR.

## What the trajectory shows (quoted)

```
tool search  {"query": "Anthropic news latest"}
tool search  {"query": "Anthropic announcement this week"}
             → refused: OpenRouter web returned no sources; DuckDuckGo answered
               with a bot check (CAPTCHA) instead.
tool fetch   {"url": "https://www.anthropic.com/news"}
…
tool search  {"max_results": "10", "query": "Canada EU partnership"}
             → OpenRouter web returned no sources; DuckDuckGo answered with a bot check (CAPTCHA)
tool secret  {"action": "list"}
tool fetch   ×3 (wikipedia, eeas.europa.eu, international.gc.ca)
```

The model's own summary to him: *"Web search providers are unavailable here (no search key, DuckDuckGo blocked), so I read Anthropic's newsroom directly."*

## What he saw

The two `search` rows one under the other with the refusal text inline, in red, inside the turn's fold; then three `fetch` rows drawn as *Fetched fetch* (the desktop's label bug, fixed). The turn took 1m 41s.

## Asks

1. **Parallel searches** — the model issued the queries one at a time because each is a tool round-trip; a `search` that takes several queries (or the prompt telling the model it may issue several `search` calls in one step) would halve the wait he saw.
2. **The DuckDuckGo fallback** — a bot check is not a search result; when the provider is absent the tool could say so once, up front (*"no search key on this kernel; set BRAVE_API_KEY / EXA_API_KEY / TAVILY_API_KEY"*), rather than returning a CAPTCHA as an error the model has to interpret and the person has to read in red. The model reached that sentence itself two calls later.
3. **The refusal's wording** — *"OpenRouter web returned no sources; DuckDuckGo answered with a bot check (CAPTCHA) instead"* is two providers' failures in one line; the person needs the one thing to do (add a key).
