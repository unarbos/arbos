---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# Coordinator runs the one quick command — PR #190, branch `cursor/coordinator-shell-b027` (on `main`)

From the Cursor symmetry loop, cycle 3: asked to run one shell command and show its output, the coordinator said it could not run shell commands. Cursor's coordinator ran it.

Change: a coordinator root keeps `bash` (and with it `terminal` and `secret`; `undo` still dropped). CONTRACT and PROTOCOL: bash is for the one quick command the user asks to see run, or a read-only probe; a build, a test run, or an edit of the tree is a worker's. New rule: never tell the user what the role cannot do — run the quick thing, or spawn a worker with the exact ask (`wait=true` for a one-off) and relay its output. Also: a new user message gets its own answer, never a restatement of the last status (the steer repeat seen in `c3-p9-end-pair.png`).

Scenarios (coordinator place, `[root] role = "coordinator"`):
- `Run this exact shell command and show me its output as it arrives: for i in 1 2 3 4 5 6; do echo "step $i"; sleep 2; done` → a `bash` call from root (gpt-5.4-mini picks `background:true`; the process row streams the lines), no worker, and the reply never mentions the role. Checked live with gpt-5.4-mini on 2026-09-14.
- `Run cargo test` / `Fix the failing test in x.rs` → still a spawn, not root's bash (the contract line; watch for drift across models).
- Root's `write` outside `.arbos/` is still refused by the write guard; `undo` is not in root's roster.
- A steer to a coordinator waiting between worker reports → the reply answers the steer; it does not repeat the previous status paragraph.

E2e: `crates/arbos-kernel/tests/coordinator_shell_e2e.rs`. Unit: `project::tests::the_role_narrows_the_top_agent_only_in_memory`.
