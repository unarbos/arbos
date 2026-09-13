# K-12 PR tracking — QA note (features agent, 2026-09-13)

Branch `cursor/pr-tracking-b027`, stacked on #23 (`cursor/composer-pills-b027`), base `rust`. PR #41.

## What it does

- Kernel: when a `bash` (or `terminal`) call whose command contains `gh pr create` finishes without error and its output names a GitHub PR URL, one record goes to `.arbos/prs.jsonl` (`ts, agent, url, repo, number, branch`). Same URL twice = one record.
- Prompt: `<<prs>>` block lists the newest 20 with agent and branch.
- Wire: `TreeNode.prs` = PRs opened by the agent and its descendants; `Frame::Tree` is rebroadcast when a new PR lands.
- Desktop: the "PRs N" pill (from #23) counts from `prs.jsonl` for the chat's subtree when the file has entries; otherwise the old output scan. Driver `session_json.pills = {working, prs, pr_urls}`.

## Attack ideas

1. `gh pr create` that prints the URL and then exits non-zero (e.g. label failure): today not recorded (error set). Decide whether that is right; the PR does exist.
2. URL only on stderr: bash merges stdout+stderr into the body, so it should be caught — verify.
3. `gh pr create --web` (opens a browser, prints no URL): nothing recorded; fine, but the prompt should not claim a PR.
4. `gh pr create ... | tee log` and `cd x && gh pr create`: `opens_pr` splits on `; | & \n` — check both record.
5. GitLab / Gitea / GitHub Enterprise URLs: not recognised (GitHub.com only). Note it.
6. A child agent opens a PR, then is archived: `prs_of_tree` needs the agent list to find ancestry — archived agents fall out of `list_agents`, so the parent's count may drop. Check and report.
7. `terminal` (PTY) path: does a PTY tool call produce a Tool record with `command` in args? If not, PRs opened in the visible terminal are missed.
8. Two agents open the same PR URL (rerun): one record, attributed to the first.
9. Remote place (host set): the desktop skips the file and scans output — confirm no panic and the pill still shows.
10. 200 PRs: `<<prs>>` shows 20; the pill shows 200; `prs.jsonl` reread on every pills() render — check the cost with a big file.

## How to run

Fake `gh` in PATH (see the PR body). Headless: `arbos-kernel run` with two creates and an echoed URL → two records. Desktop: driver `state()` → `pills.prs`.
