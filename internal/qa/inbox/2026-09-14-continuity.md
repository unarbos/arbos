---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# Subscription continuity — PR #162, branch `cursor/subscription-continuity-b027` (on `main`)

T3-08. `continuity = true` on a `shell` or `timer` subscription. Shell: the next firing's message ends with "Last time it printed: …" (`seen`, capped 4000 chars); `notify` may use `{previous}`. Timer: the next firing's prompt ends with "Last time this fired, your turn ended with: <last words>". Other kinds refuse the flag.

Scenarios:
- Shell monitor `df -h /` every 30s with continuity → second firing shows both readings; the agent can say "unchanged".
- `deliver_to = "user"`, `notify = "disk: {output} (was {previous})"` → the user line carries both after the second run; "(nothing yet)" on the first.
- Timer with continuity: the agent's reply to firing N shows up in firing N+1's wake text. Restart the kernel between firings → still carried (it is in the file).
- `kind = "goal"` or `inbox` with `continuity = true` → `check` error / `subscribe add` refusal.
- Redaction: a shell output containing a granted secret → `seen` holds the redacted text (run_job redacts before it reaches here).

E2e: `crates/arbos-kernel/tests/continuity_e2e.rs`.
