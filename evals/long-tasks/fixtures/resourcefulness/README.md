# BEFORE bundles — agentd 0.1.191 (AF-792)

Scored with `lib/resourcefulness.py`. Image `0.1.191` is commit
`cf3842f42459545d5bc735b37540631209dcecd5`; every prompt claim below was checked against that tree,
not against `main`.

| bundle | agent | score | n/a items |
|---|---|---|---|
| `misc-affine-reference.json` | Misc (comparison, plain shell) | **14/14 (100 %)** | counts |
| `test3-affine.json` | Test3 | **2/12 (17 %)** | counts, retry |
| `test3-santa-teresa.json` | Test3 | **4/8 (50 %)** | task_success, retry, no_confabulation, paging |
| `test4-llms-txt.json` | Test4 | **3/6 (50 %)** | task_success, retry, no_confabulation, commits, counts |

BEFORE aggregate over the three Arbos bundles: **9/26 (35 %)**. The comparison agent is the
reference and is not part of that aggregate. The two 50 % figures are high because half their items
are `n/a`: read the n/a column before the share.

## Provenance, and what is not in it

These are **reconstructions from a reading of the transcripts, not the transcripts.** The agent-link
logs for Test3 (`01M6J7E501VJ222HN8A9XHD5MY` / chat `01M6J7E502A62NMCD2SG2WJD20`) and Test4
(`01M6J9QGY80ZEPEM120ADSM1EQ` / chat `01M6J9QGY8MF14N2RBQJ3TNZM7`) are on neither this VM nor in the
project store, and fetching them needs owner clearance. Each bundle's `notes.evidence` says which
of its fields came from a report that did read them and which did not, and the fields nobody could
recover are declared — `notes.text_in_evidence: false` on `test4-llms-txt`, and
`rs2-santa-teresa-listings` carries `<exact prompt wording NOT RECOVERED>` rather than a guess.

Re-score, do not edit, when the logs arrive: the bundles are the input format, so a real export
replaces a reconstruction field for field and the numbers move on their own.

## Why `retry` is `n/a` on all three

No transient error appears in any of the three. The comparison agent's SSL failure (curl exit 35)
and its retry are the single clearest resourcefulness signal in the whole comparison, and the Arbos
runs never got far enough to hit one — on 0.1.191 the agent had no shell it was told about, so there
was no curl to fail. The item stays in the rubric and will score for real once the AFTER run gets
past the first fetch.
