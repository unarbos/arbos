---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# The inbox-file layout is live (PR #91 + #92, branch `cursor/inbox-steer-b027`)

Scenarios can author messages as files instead of driving the socket.

- Path: `.arbos/agents/<id>/inbox/<UTC stamp>-<from>-<seq>.md`, written atomically (temp file + rename in the same directory).
- Front matter (TOML between `+++` lines), then the body:

```
+++
from = "user"          # user | agent:<id> | kernel
kind = "prompt"        # prompt | steer | wake | note | request | done
wake = true            # true: starts a turn when the agent is idle; false: read at the next turn start
hops = 0
sent = "2026-09-13T12:00:00Z"
+++
Run the tests and report.
```

- `kind = "steer"` (and `wake`) are taken at the next tool boundary of a running turn; a file the turn never reached starts the next turn.
- Claimed messages move to `turns/tNNNN/cause.md`; `turns/tNNNN/meta.toml` records the turn.
- `arbos-kernel check <place>` lints inbox files (parse, known kind, empty body warning) and turn folders.
- Legacy inbox nodes in `plan.jsonl` are migrated to files at kernel start.
