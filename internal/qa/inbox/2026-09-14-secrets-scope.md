---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# Secrets: scoped grants + stream redaction — PR #150, branch `cursor/secrets-scope-b027` (on `main`)

K-08 follow-ups.

- A grant belongs to the agent that ran `secret use NAME` and reaches its jobs and its descendants' jobs (parent links on disk). A sibling chat's `echo $NAME` prints empty. `secret revoke` acts on the caller's subtree; `secret list` names holders.
- Redaction now also covers the live job stream to the window (`Frame::Job` deltas — the process row) and shell-subscription output before it becomes an inbox message or a `notify` line. Tool results were already covered.

Scenarios:
- Two top-level chats; chat A `secret use X`, then chat B runs `echo $X` → `X=` (empty). Chat A spawns a worker that runs `echo $X` → `[REDACTED:X]` in its transcript.
- Worker runs `secret use Y` → root's own bash does not get `Y` (grants flow down, not up). Say if root should get its workers' grants; today it does not.
- `bash background=true` job that `echo $X` every second: the desktop process row must show `[REDACTED:X]`, never the value.
- Shell subscription `cmd = "echo $X"` with `deliver_to = "user"` and `notify = "{output}"` → the user line is redacted. Before this PR it was not (qa-worthy on `main`).
- After `secret revoke X`, a job that had written the value to a file and `cat`s it back still shows `[REDACTED:X]` (value stays tracked).

E2e: `crates/arbos-kernel/tests/secrets_scope_e2e.rs`.
