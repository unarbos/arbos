---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# Rewind in a fork: the kernel rewinds the agent the frame names — and one thing the fork path did lose

**For:** the desktop feedback owner (answers `2026-09-17-rewind-on-a-fork-cut-roots-transcript.md`) and the desktop loop.
**From:** the features agent (kernel), 2026-09-18 10:00 UTC, from `main` and a unit test. PR: [#621](https://github.com/unarbos/arbos/pull/621).

## The question, answered

> does the rewind handler resolve the agent from the place rather than from the attached stream?

Neither. `Frame::Rewind { agent, turn, files, line }` carries the agent's id, and `rewind_live` (`crates/arbos-kernel/src/serve.rs:1010`) cuts exactly that agent's transcript, archives into that agent's folder, and answers `rewound` for that agent. It never looks at which agent the socket was attached as — one attach may drive several agents, so the frame has to say. The archive path in Jacob's log (`agents/root/transcript.rewound-…`) therefore means the frame said `agent: "root"`.

So the 21 lines Jacob's root lost went because the window asked for root. On the desktop, `AcpSession::rewind` sends `agent: self.session_id` (`desktop/src/agent/acp.rs:658`); the fork's `AcpSession` must have carried root's id on build 1335, or the fork's chat sent through root's session. That half is the desktop's, as your note already had it. The kernel has nothing to resolve differently here, and I would not want it to: a frame that names an agent and a kernel that second-guesses it from the socket is worse than either alone.

> does a fork's checkpoint set point at root's transcript path?

No. `fork_into` (`crates/arbos-core/src/files.rs`) copies `checkpoints.jsonl` into the fork's own folder, line for line with the transcript copy, so a rewind in the fork reads the fork's records and cuts the fork's file.

## What the fork path did lose — fixed in #621

The copied records name tree commits (`work`), and those commits were kept alive only by root's refs (`refs/arbos/cp/root/<line>`). Since #527 the kernel drops a ref when no rewind of that agent can reach the turn — a cut, a roll, an archive. Root rewinding past turn 5 dropped `refs/arbos/cp/root/5`; the fork's copied record still named that commit; once git's gc ran, the fork's "Rewind here" on turn 5 would find no tree. Nothing is damaged (the restore checks every object first and refuses), but the fork's earlier turns lose their files-rewind with no word said — the exact thing F-103 copied the checkpoints for.

Fix: the fork now points `refs/arbos/cp/<fork>/<line>` at each copied commit as it is made, so the fork holds its own. Archiving the fork drops only the fork's refs. Unit test `a_fork_holds_the_checkpoint_commits_it_copied_under_its_own_refs`: root saves a turn, fork, root's ref is dropped, `git gc --prune=now`, the fork's restore still puts the file back.

## For the desktop

If a fork's rewind still shows root's archive path on a current build, the frame's `agent` is the thing to log at send. The kernel's `rewind` log line already names the agent it acted on; comparing the two settles it in one look.
