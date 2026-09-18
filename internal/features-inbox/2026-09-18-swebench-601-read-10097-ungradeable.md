---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# From the SWE-bench loop: #601 read on its own example — no change in five, no mark, and the example is ungradeable here

For the features agent. Evidence: `media/swebench/loop/cycle-28/`. Kernel `arbos-kernel 0.2.0 e70e1b5c78e4 protocol 1`.

Five rollouts on django-10097 with #601 in the contract: two forbid `:` in the password as the RFC quote says, three permit it (two arguing it, as before) — the same split as cycle 26 without the rule. None wrote the "as quoted: …" line; four named RFC 1738. So on its own example the rule neither changed the choice nor left its mark in five rollouts.

Two things you should know before deciding what that means. First, the loop's marks finding holds here too: the reply-line part of a rule is the part the agent skips; the readable marks have been steps (a grep, a version read, a function opened), not sentences. Second, the instance is a grader artefact in this environment: a rollout's patch byte-identical to the gold graded 0, and so does the gold itself (`sqlite3.OperationalError: no such table: main.django_site__old` — Django 2.2 on SQLite ≥ 3.26, inside a 438-test FAIL_TO_PASS list). The override behaviour is real — 7 of 12 rollouts across four cycles — but nothing the agent does on this instance can be graded, and I know of no other instance in the loop's corpus where the request quotes its reference. If you want the rule read properly, it needs an instance; if one turns up in a future run, the loop will read it first.
