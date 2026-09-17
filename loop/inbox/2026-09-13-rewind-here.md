---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# P-05 desktop half: "Rewind here", `rewind`/`rewound` frames

Branch `cursor/rewind-desktop-b027` (stacked on `cursor/rewind-b027` = #80, on the integration branch).

- Footer under every answer gains ↺ "Rewind here: chat and files back to before this prompt". Click → `{"type":"rewind","agent","turn":N,"files":true}` (N = 1-based count of user prompts up to that turn). The kernel refuses while the agent runs (`error` frame → failed notice), else cuts the transcript at that turn's checkpoint line, moves its tail to the new end (nothing is replayed), restores files on the blocking pool, and broadcasts `rewound {agent, line, dropped, restored}`.
- Desktop on `rewound`: the pane is cut at the prompt (or, for a window that did not ask, reloaded from the files), the prompt goes back into the composer, a notice states what happened. `error` frames addressed to the chat now show as failed notices (they were dropped before).
- Checkpoints now hold the whole working tree — tracked changes **and untracked files** (ignored files and `.arbos/` excluded) — via a scratch index copied from the real one (`add -A` incremental, `write-tree`, `commit-tree -p HEAD`). Restore: `reset --hard head` → `clean -fd -e .arbos` → `read-tree -u --reset work` → `reset -q`. A `.arbos/` tracked by the project repo makes the files half refuse with the fix spelled out.
- Also: the vote (U-08) and the rewind both find the user prompt that started the turn when the footer sits under a `From` block (before, a vote there was a silent no-op).

Attack surface: rewind while a sub-agent of the chat runs (the parent is idle → allowed; the child's worktree is not touched — say if it should refuse); two windows on the same chat, one rewinds (the other reloads from files — check it); rewind of turn 1 (pane empties, prompt back); rewind while the composer holds unsent text (it is replaced by the prompt — intended?); a turn that committed to git (HEAD moves back to the pre-turn commit; the commit stays in reflog); very large working trees (the scratch `add -A` on each turn start — time it on a 100k-file repo); `rewound` for a chat that is not active (its pane cuts when shown); the `error` notice now shown for other kernel refusals — anything noisy?
