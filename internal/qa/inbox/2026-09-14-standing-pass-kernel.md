---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# Layout worker's standing pass, kernel items — PR #140, branch `cursor/standing-pass-kernel-b027` (on `main`)

Four fixes, one PR. E2e: `crates/arbos-kernel/tests/standing_pass_e2e.rs` (3 tests, replay provider, no key).

1. **Fork claims no worker; no agent is its own ancestor.** `arbos_core::files::fork_chat` copies the transcript and drops `child` from every spawn record (body gets "(spawned by the chat this one was forked from; not this chat's worker)"). The desktop's local fork uses it. On the wire the kernel scrubs `child` from replayed and tailed spawn records unless the named agent exists *and* names this agent as parent, so forks made before this read right too. `tree` reports `parent` only when the chain from it ends without looping (self-parent, missing parent, or cycle → top level).
   - Scenario: spawn a worker, fork root, ask `history` for the fork → no `child` on the spawn record. Edit `w1/agent.md` to `parent: w1` → snapshot tree shows w1 top-level; root's replayed spawn record loses its `child`.
2. **Workers do not fan out.** A kind-less child is saved with `role: worker` in `agent.md` and its prompt gets a short "Role: worker" line (do the task yourself; a brief naming other workers is the parent's plan; no spawning unless the brief splits your part). Kinds set `role:` in front matter; `role: none` gives no line. Kind-less children from before get the worker role in memory. The kickoff text now says `task` is *this worker's own piece* of the ask, not the user's whole words (that was the source: each worker got the whole delegation plan).
   - Scenario to re-run: the 3-worker ask from the pass; expect 3 children, 0 grandchildren.
3. **Empty-reply line is a `nudge`.** `[kernel] Your reply was empty…` and `[kernel] That was a tool call written as text…` are `nudge` events now (the model still sees `[kernel] …`); no `user` line the user never typed.
4. **`rewound` at once.** The frame used to wait for `restore_files` (git, seconds on a large store). Now: cut → `rewound {pending: true}` immediately → second `rewound {restored}` (or `error`) when the restore ends. Desktop shows "restoring files…" then swaps the line in place. Test asserts the first frame arrives < 3 s (measured ~30 ms) with the transcript already cut.

Not changed: coordinator-place root still gets the coordinator role only in memory (never on disk).

## Update 03:44 — qa-032 fixed on the same PR (`288c9c5`)

The cut landed at the turn's `user` line and left its `wake`; a restart fired an empty turn. `rewind::cut` now walks back from the checkpoint to the previous `turn_complete` and cuts at the first wake after it. The e2e restarts the kernel after the rewind: transcript ends on `turn_complete`, one wake left, no `turn` frame on restart (the assertion fails on the old cut). Re-run `mt-29-rewind-latency` for `mt-29-dangling-wake` / `mt-29-rewound-prompt-refired`.
