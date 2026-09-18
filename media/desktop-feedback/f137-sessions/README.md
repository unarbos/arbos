# F-137 — the window's view of agents vs the kernel's, place `~/.arbos`

Read-only collection from Jacob's MacBook, 2026-09-17 07:30–07:40 local
(UTC-3), app build 1335 running, nothing restarted. Times below are local.
The place is `~/.arbos` (its store is `~/.arbos/.arbos/`). Nothing is
redacted: no key, token or password was found in any file (scanned for known
key prefixes, `token=`/`password=` shapes, PEM blocks and JWTs). The long
base64 runs in the transcripts are Anthropic thinking-block signatures, not
credentials.

## Files

- `desktop/sessions/*.json` — every session record the window keeps for this
  place (7 files). `desktop/feedback.json`.
- `agents-tree.txt` — `find -ls` of the kernel's `agents/` now.
- `agents/root/` — `agent.md`, today's transcript lines
  (`transcript-2026-09-17.jsonl`, 73 lines, 06:41:46–06:46:09), turns
  t0008–t0011, today's jobs j4–j10, listings of `inflight/`, `inbox/`,
  `images/`. There is no `status.toml` in `agents/root/` at capture time.
- `agents/chat-1789473184580/` — `agent.md` and `transcript.jsonl` of the
  agent the window draws as **Delegate 1** (see below).
- `archive/agents/{quicksort,mergesort,heapsort}-worker/` — the kernel's
  full record of this morning's three workers, which it archived at 06:43–06:44.
  `archive-listing.txt`, `archive-today.txt`.
- `runtime/kernel.json`, `kernel.out.log`, `lock`, `focus`,
  `kernel.log-0630-0705.jsonl` (470 lines, 06:30–07:05).
- `processes.txt` — the kernels and app alive at capture, and which binary
  the `~/.arbos` kernel executes.

## What the files say (facts, with times)

Correction to what I said earlier from the listing alone: the three worker
session files are named by their creation time in ms, and they decode to
**06:42:45.692, 06:42:49.724, 06:42:54.037** — not 06:36. 06:35:57 is when
the kernel for this place started (`runtime/kernel.json`, `lock`).

1. 06:42:23 — root's turn: "run 3 sub agents that all wrie their own sorting
   algorithm…". Notice: `inception/mercury-2.5` failing over to
   `anthropic/claude-opus-5`.
2. 06:42:53 — root's `spawn` tool returns three results: `quicksort-worker`,
   `mergesort-worker`, `heapsort-worker`. The window's three session files
   (`session` = those ids, `parent` = `root`, `delegate_number` 4, 5, 6,
   `name` = "quicksort worker" etc.) were created at 06:42:45, :49 and :54 —
   the first two **before** the spawn tool's result line is stamped in root's
   transcript (06:42:53).
3. 06:43:00 — root's `agents` tool lists all three as running. By 06:43:18
   the workers are done (their archived transcripts end with "Done…").
4. 06:43–06:44 — the kernel moves the three worker directories to
   `archive/agents/` (`agents/` mtime 06:44:49). So the kernel *does* have a
   record of the three workers; it is in `archive/`, not `agents/`.
5. 06:44:20 — root's `agents` tool now reports them idle with last turn
   success at 09:43:18Z. 06:44:49 root reports; 06:44:57 turn complete.
6. Each worker session in the window has 7 items ending with the worker's
   "Done…" message, `closed: false`, `updated` 07:01:50.

## Delegate 1

The row is the session record `desktop/sessions/1789473184578.json`:
created **2026-09-15 08:53:04**, `session: "chat-1789473184580"`,
`parent: "root"`, `delegate_number: 1`, `name: null` — so the window has no
name for it and labels it "Delegate N". Its only item is a `Notice`
"reconnected".

The kernel side: `agents/chat-1789473184580/` exists (created 09-15 08:53),
`agent.md` says `name: chat`, `parent:` empty, `role:` absent, model
`inception/mercury-2.5`; its transcript is one line, the "reconnected" notice
at 09-16 07:04. It never took a turn. Root's transcript and the kernel log
in the window never mention it. So the two sides disagree about what this
agent is: the window records it as a delegate of root (parent `root`), the
kernel as a parentless agent named `chat` — the shape the desktop's "New
Chat" creates — with no worker role, which is presumably why the roster the
kernel returns leaves it out while the window still draws a row.
