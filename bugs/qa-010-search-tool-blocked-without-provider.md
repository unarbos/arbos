# qa-010: without a configured search provider, the `search` tool is blocked by the engine and reports the block as a normal result

status: pr-open — https://github.com/unarbos/arbos/pull/22 (branch `cursor/fix-qa-010-search-block-de28` -> `rust`)
severity: low-medium (research tasks fail on a fresh install; the agent has no way to tell "blocked" from "no results")
scenario: bench-research-links (benchmark item 4)
rollout: /cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/qa/rollouts/20260913T001642Z-bench-research-links
fingerprints: 2e5d9b92fa

## Repro

Config with no `search_url`/`search_key`; prompt: research a topic from web sources and write a linked document.

## Expected

Either a working default search, or a tool error the model can act on ("no search provider configured; use fetch on a known URL").

## Actual

Two `search` calls "succeed" (no `error` on the tool record) with a page saying automated requests are blocked; the model gives up: "The search engine is blocking automated requests." In another run it fetched Wikipedia and wrote `pages/research.md` without any markdown links.

## Suspected location

`crates/arbos-engine/src/tools/web.rs` `Search` (scraping fallback when `search_url` is empty); `crates/arbos-engine/src/host.rs:22-23` `search_url`/`search_key`. Recommendation: a search key in the vault + default `search_url`, and treat a block page as a tool error.
