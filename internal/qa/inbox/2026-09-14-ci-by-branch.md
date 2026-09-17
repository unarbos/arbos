---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# `github_ci` by branch — PR #145, branch `cursor/ci-by-branch-b027` (on `main`)

K-04 open item. `subscribe add kind=github_ci repo="owner/name" branch="main" prompt="keep main green"` (or a hand-written `subscriptions/NNNN-*.toml` with `branch = "main"`, no `pr`). The kernel polls `gh run list --branch` every 60 s (or `every`, floor 30 s) and delivers a `[github]` message when the newest commit's runs change: `o/r@main (red): new commits: head is now bbbbbbb; check ci: success → failure (https://github.com/o/r/actions/runs/3). You asked: keep main green`. Green checks carry no URL.

- First look only remembers (`seen`); the diff starts on the second. To test fast, pre-seed `seen` with a green snapshot (see `crates/arbos-kernel/tests/ci_branch_e2e.rs`, which also shows how to stand in `gh` with a script on PATH — no network, no token).
- Only the newest commit's runs count, one per workflow. A run that is `in_progress` reads as `pending`; the state line (`green → red`) is dropped from the message because the check lines say it.
- `github_pr` unchanged; `github_ci` with a `pr` unchanged.
- Real run: needs `gh` authenticated (or a token granted through the secrets door — `gh` runs with the door's env). Try it on `unarbos/arbos` `main`.
