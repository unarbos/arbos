# Law results

| Law | Seed | Result | Cases | Seconds |
| --- | ---: | --- | ---: | ---: |
| redaction-idempotence | 81001 | Passed | 256 | 0.338 |
| confident-token-removal | 81002 | Passed | 256 | 0.315 |
| utf8-key-byte-ranges | 81003 | Passed | 256 | 0.299 |
| json-scrub-compositionality | 81004 | Passed | 256 | 0.305 |
| url-visible-text-roundtrip | 81005 | Passed | 256 | 0.012 |
| confident-token-nonword-ending-removal | 81006 | Passed | 256 | 0.082 |
| nonsecret-text-preservation | 81007 | Passed | 256 | 0.077 |

All 7 required laws pass256 seeded cases each (1792 total), with max 256 shrink candidates and concurrency 1. Native outputs come from actual source functions. The before boundary failed, shrank once and replayed; it passes after the focused boundary fix. Ten transport checks and deterministic four-variant/all 17-family coverage also pass. See README for precise domains, before/after evidence, mutation review and blocked proof/integration scope.
