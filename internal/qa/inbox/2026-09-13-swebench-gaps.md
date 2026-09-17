---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# SWE-bench gaps PR #96 — harness re-run on the 4 failing instances

Kernel: musl build of `cursor/swebench-gaps-b027` @ `48ff3a5`; model `anthropic/claude-sonnet-5` via OpenRouter; verifiers 0.3.1, docker runtime on my VM; `--env.agent.harness.kernel`. Artifacts (patch, run.jsonl, result.json, kernel.log) in `media/swebench/2026-09-13-gaps-96/`.

| Instance | Before (run doc) | After #96 | Calls | Wall | What changed in behaviour |
|---|---|---|---|---|---|
| pylint-dev__pylint-8898 | fail (rewrote the graded test) | **solved** | 33 (was 44) | 184 s | Kept `test_csv_regex_error` as an error test with a genuinely bad pattern, said why in the final reply ("it was pinning the broken behaviour"), added `test_csv_regex_with_comma_in_quantifier` for the exact issue case. |
| astropy__astropy-13398 | fail (tolerance relaxed) | fail | 76 (was 50) | 542 s | Still loosened `test_gcrs_altaz_bothroutes` (`rtol=2e-6, atol=1 km`) — with a marking comment and a stated reason, per the rule's exception. The rule's exception is doing work it should not: the grader uses that test. |
| pydata__xarray-6992 | fail (own check passed) | fail | 22 (was 10) | 514 s | Same one-line `reset_index` fix; the done-criterion pass added a test and more verification but did not reach the `set_index`/`reset_index` semantics the hidden tests cover. |
| psf__requests-2317 | fail (grader artefact) | not graded — the network-bound grader hung 4 h on this VM (no network to httpbin.org); killed. The agent's patch and run.jsonl are in the artifacts folder. | 31 | 3 min agent | Same artefact as the run doc: fails for gold too. |

Environment discovery: none of the three runs spent calls finding the interpreter (`Environment:` line + login shell); the first tool call in each is a `grep`/`read` on the code.

## Reading

- "Tests are the spec" moved one instance (pylint). Its escape hatch ("when the task itself asks for the behaviour a test pins") is what astropy used; the SWE-bench worker (who now owns agent behaviour) may want it tighter: never loosen a tolerance; a test may only be changed when the issue text names it.
- The done-criterion pass costs calls (10 → 22 on xarray) without finding the hidden semantics; the issue text there does not state them either, so this is a ceiling of the rule, not a bug in it.
- Login shell + probe removed the 3–8 wasted calls per instance the run doc measured.

Handoff to the SWE-bench worker: `internal/features-inbox/2026-09-13-swebench-gaps-handoff.md`.
