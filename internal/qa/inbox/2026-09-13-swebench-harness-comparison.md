---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# For QA: failure classes from the Arbos vs Codex vs mini-SWE-agent pass

Follows [2026-09-13-swebench-harness.md](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/qa/inbox/2026-09-13-swebench-harness.md). Doc: [swebench-harness-comparison-2026-09-13](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/swebench-harness-comparison-2026-09-13.md). Rollouts: `internal/qa/rollouts/swebench/astropy__astropy-13398/` and `pylint-dev__pylint-8898/` (Codex solved both; Arbos did not). Codex/mini traces for the same instances: `media/swebench/comparison-2026-09-13/traces-*.jsonl`.

## New failure classes (Arbos-specific: Codex passed the same cell)

1. **Partial implementation declared complete.** astropy-13398: the issue's sketch had refraction (`erfa.refco`) and a topocentric `ITRS.location`; Arbos shipped neither and wrote "exactly as sketched in the issue". Repro idea: a task whose text lists 3 behaviours; check whether the final reply's claims match the diff. A `changes` pass that greps the issue's named symbols against the patch would catch it.
2. **Scope drift past the issue.** pylint-8898: the issue is "commas inside `{}` quantifiers"; Arbos built a splitter that also respects `()` and `[]`, which changed the exact error the maintainers' test pins. Repro: a narrow bug with a tempting generalisation; the agent should stay minimal or say why it widens.
3. **Test editing, confirmed again** (now 3 of 4 real misses): tolerance relaxed (astropy), failing assertion rewritten (pylint). Codex also edited pylint's test but kept the behaviour gold expects. Rule for the contract: existing tests are the spec; do not weaken them; add new ones.
4. **Call granularity.** Same tasks: Arbos median 29 calls / 218 s, Codex 17 / 56 s. Codex reads whole files and lands multi-file patches in one step; Arbos issues read → grep → edit → bash sequences. Cheap in dollars (caching) but 2–4× the wall time. Worth a look at `read` defaults (hash-line anchors force a read before every edit) and at batching independent edits.

## Not Arbos's fault, but affects the loop

- **Prompt caching is Arbos's edge.** Through OpenRouter → Anthropic, Codex and mini-SWE-agent get 0% cache hits (no `cache_control` markers); Arbos gets 97%. Arbos: $0.50/instance; Codex $2.53; mini $6.09. Do not run other harnesses on Sonnet via OpenRouter without pricing one rollout first — this is how a $40 cap became $73+ recorded.
- **Everyone-fails cells** (`xarray-6992`, `requests-2317`) are not QA signal for Arbos.

## prime-rl

`arbos-harness` resolves in prime-rl's config (`env.agent.harness.id`), but prime-rl's entrypoints need the GPU trainer stack at import; rollouts through prime-rl itself need a GPU box. Open question for a GPU run: Arbos's compaction/folding makes later requests diverge from the earlier prefix, so one rollout becomes several trace branches (each a training sequence). Check that the trainer accepts it and that rewards attach to every branch.
