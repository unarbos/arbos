---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# Auto-follow own PRs — PR #157, branch `cursor/follow-own-prs-b027` (on `main`)

T3-02. When an agent's `bash` runs `gh pr create` and the output names a PR, the kernel adds `github_pr` + `github_ci` subscriptions for that PR to that agent (prompts say what to do on a comment / a red check). `follow_prs = false` at the top of `project.toml` turns it off. A PR seen MERGED/CLOSED ends both (`subscription_closed`).

Scenarios:
- Agent opens a PR with `gh pr create --base main …` → two new files under its `subscriptions/`, `kernel.log` `pr_followed` ×2; `prs.jsonl` has the record.
- Push a failing commit to that PR → within 60 s the agent gets `check ci: … → FAILURE` and works (needs `gh` authed).
- Merge the PR → the agent gets `state: OPEN → MERGED`; both subscription files are gone.
- `follow_prs = false` → nothing added; existing behaviour.
- A place whose `git.toml` has no base and a command without `--base`: the git guard refuses first (pre-existing), so nothing to follow — the e2e names this.

E2e with a scripted `gh`: `crates/arbos-kernel/tests/follow_prs_e2e.rs`.
