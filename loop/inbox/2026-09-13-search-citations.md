# P-13 web search with citations — QA note (features agent, 2026-09-13)

Branch `cursor/search-citations-b027`, base `rust`.

## What it does

`search` picks a backend, in this order, first one that is configured:

1. `search_url` in `config.toml` (a custom JSON endpoint: `GET <url>?q=` returning `{results:[{title,url,snippet}]}`; `search_key` as bearer) — as before, now actually wired.
2. **Exa** when `EXA_API_KEY` is set (`POST https://api.exa.ai/search`, highlights as snippets).
3. **Brave** when `BRAVE_API_KEY` is set.
4. **Tavily** when `TAVILY_API_KEY` is set.
5. **OpenRouter web plugin** when the model provider is OpenRouter (`plugins: [{id: "web", max_results: 8}]` on a small non-streaming call; the answer's `annotations[].url_citation` become the sources). Uses the same key as the model.
6. **DuckDuckGo HTML** with no key: titles, URLs and snippets parsed from the result blocks (was: bare hrefs plus a text dump).

Result shape is the same for all: `[n] Title — URL` then the snippet, then one line: "Cite with [n]; end your reply with a Sources list of the URLs you used." `fetch` results now start with `Source: <url>`.

CONTRACT line: after `search` or `fetch`, claims from the web carry `[n]` and the reply ends with `Sources:` and the URLs. The desktop renders URLs as links already.

## Attack ideas

1. No key, OpenRouter provider: the web plugin path costs a model call per search (~$0.001–0.02). Check the cost shows in #44's Cost row and that a search-heavy turn does not spiral.
2. OpenRouter web plugin returns no annotations (model answered without searching): fall back to DuckDuckGo, say so in the body.
3. DuckDuckGo blocks with a CAPTCHA page (rate limit): the body must say "no results (backend returned no result blocks)" rather than dump the CAPTCHA HTML.
4. Query with quotes and unicode: URL-encoding for every backend.
5. `EXA_API_KEY` set but invalid (401): error names the backend and status; no fallback loop hides the misconfiguration — decide: fall through to the next backend or fail? Implemented: fail with the message (a configured backend that is broken should be visible).
6. Sources list in the reply: the model should not invent URLs; compare each `Sources:` URL against the tool results in the transcript.
7. `search_url` custom endpoint returning HTML: parse error names the endpoint.
8. Result cap 8; `max_results` parameter 1..20.
9. Snippets over 400 chars are clipped; titles over 120.
10. Redaction (#30): `EXA_API_KEY` etc. are env vars — bash can `echo $EXA_API_KEY`. Add them to the protected set when #30 lands (note in the PR).

## How to run

`arbos-kernel run . 'search the web for "Arbos agent kernel rust" and tell me the top 3 with sources'` with (a) no keys on OpenRouter, (b) `EXA_API_KEY` exported, (c) `api_base` pointing at OpenAI (DuckDuckGo path). Check the numbered sources and the Sources list.
