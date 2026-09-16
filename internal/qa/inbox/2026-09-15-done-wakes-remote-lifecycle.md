---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# For QA: done wakes, PR links, remote child lifecycle, git author (2026-09-15)

Six PRs on `main`, each with an e2e. Bug notes updated: `ba2262db79`, `qa-037`, `qa-038`.

## What changed (attack these)

1. **[#236](https://github.com/unarbos/arbos/pull/236) done wakes.** A worker's done opens a `done` wake (transcript `wake: "done"`, text names who reported and who is still working). An empty reply on it ends the turn with no `empty reply` nudge. The same paragraph (40+ chars) three times in one turn ends it (`repeated reply` nudge at the second, notice at the third). A reply linking a github.com PR that no tool output or `prs.jsonl` backs gets one `pr link not from a tool` nudge; `pr create` in a repo with no remote refuses. Probe: (a) three workers finishing seconds apart — count root's messages to the user (expect one, at the end); (b) a worker parked on an `ask` while a sibling finishes: is it listed as "still working"? (c) a real PR from `bash gh pr create` mentioned in the reply — no nudge; (d) a PR URL in the user's own message repeated back — no nudge; (e) a legitimate long answer repeated because the user asked "say that again" — the second is nudged (known cost; say if it bites); (f) the nudged reply now appears once on the transcript, not twice — check older rollouts' scorers do not rely on the double.
2. **[#239](https://github.com/unarbos/arbos/pull/239) remote lifecycle** (qa-037, qa-038). `serve --leash <span>`; remote child kernels start with `--leash 10m`; SIGINT/SIGTERM on the parent stops its ssh children's kernels; a remote reply is a `done` message; archiving a remote child stops its kernel and drops its `remotes.json` record; `restore` drops records whose agent folder is gone; `spawn host=… wait=true` blocks on the report. Probe: (a) `rm-01` re-run: `pgrep` after the parent's SIGINT; (b) kill -9 the parent — the remote kernel should exit ~10 min later on its own (log `kernel_stop … leash`); (c) parent restart with the child unfinished — the kernel over there is started again and re-attached; (d) `wait=true` with `wait_secs=5` on a slow worker — "still working after 5s", then the report as a later message, once; (e) a remote reply landing on the parent's transcript exactly once.
3. **[#237](https://github.com/unarbos/arbos/pull/237)** remote child `status`/`turn` frames under the local id; step on the tree row. Probe with the attach protocol only (no files): `turn running` → `status` → `turn idle` + empty `status` for the local child; nothing under `root` from the remote.
4. **[#238](https://github.com/unarbos/arbos/pull/238)** roster `kind: worktree`, `parent`. Probe: a hub restart mid-claim — the name-derived path; a project literally named `a--b` on a machine with no `a` — left alone.
5. **[#241](https://github.com/unarbos/arbos/pull/241)** commits in a repo with no identity are authored `Arbos <unarbos@users.noreply.github.com>`; the git guard no longer asks. Probe: a repo with `user.name` but no `user.email` (default applies to both); `GIT_AUTHOR_NAME` exported to the kernel wins; the identity check is cached 60 s per cwd — configure git mid-session and commit within a minute (the default may still apply once).
6. **[#235](https://github.com/unarbos/arbos/pull/235)** a stray `agents/<x>/` without `agent.md` is logged `agent_unlisted` at boot.

## Not done

Mobile item 5 (a `step` on `assistant_delta` matching the `assistant` event): deferred, reasons in `features-inbox/2026-09-15-mobile-cycle-1-kernel-answers.md`.

## Added later the same day (attack these too)

7. **[#243](https://github.com/unarbos/arbos/pull/243)** remote transcript mirrored every 3 s while the turn runs (hub: `history` frames; ssh: a tail over ssh). Probe: a remote worker that prints for 30 s — the local child's transcript grows in 3 s steps; the report to the parent arrives once, at the end, with the last words (not a mid-turn sentence); after archive no "link lost" message to the parent.
8. **[#244](https://github.com/unarbos/arbos/pull/244)** `bash background:true` honoured for a server or watcher only. Probe: (a) the p13 loop with `background:true` — all six lines, no job; (b) `python3 -m http.server 8000` with background — a job at once; (c) a 3-minute build marked background — attached to the 120 s floor, then a job (the note says why); (d) `sleep 600` background — a job (a bare sleep is a wait).
9. **[#245](https://github.com/unarbos/arbos/pull/245)** coordinator tool descriptions carry the spawn-first note. Probe with a real model on a code task: does the first call become `spawn`? Count refusals per run before/after.
10. **[#247](https://github.com/unarbos/arbos/pull/247)** `step` on `assistant_delta`/`thinking_delta` and on `assistant`/`thinking`/`tool` lines. Probe: old transcripts read (`step` absent → 0); a turn with a cut-off reply (`Cut`) — the partial and the continuation carry steps N and N+1; ACP workers (Codex kind) — steps advance per tool call.
11. **[#248](https://github.com/unarbos/arbos/pull/248)** `max_children` default 24, ceiling 256. Probe: 25 spawns in one response → the 25th refused with the cap message; `max_children = 300` in config → falls back to 24.
12. **[#249](https://github.com/unarbos/arbos/pull/249)** `correction not kept` reminder. Probe the heuristic for false positives with real prompts: "No problem, go ahead" (starts with "no"), "Never mind" ("never "), "Instead of X do Y" as a fresh task; and misses: a correction phrased gently ("Hmm, I think we agreed on drafts?"). Report the ones that bite.
13. **[#250](https://github.com/unarbos/arbos/pull/250)** `github_prs`. Probe: a repo with 100+ PRs (the window) — a PR that leaves and re-enters the window; `author: "@me"` when `gh` is not signed in (error said once); rate — the default period against `gh`'s rate limit with three such subscriptions.
