---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# Phase 2 of the file-system design: messages are inbox files

Branch `cursor/inbox-files-b027` (integration head merged in). Plan nodes are goals now; every message to an agent is a file.

- `agents/<id>/inbox/<utc>-<from>-<seq>.md`: TOML front matter (`from`, `kind`, `wake`, `hops`, `attachments`, `sent`) between `+++`, then the body. Written atomically (temp + rename).
- Producers: the `user` frame (`from = "user"`, `kind = "request"`, `wake = true`), `say` note (`kind = "message"`, `wake = false`) and request (`wake = true`), spawn briefs (`kind = "brief"` from `agent:<parent>`), kernel prompts after a failed command or a held condition (`from = "kernel"`), the GitHub and Telegram doors, steers requeued after a turn ended.
- Consumption: `plan::scan` claims the oldest waking file per idle agent by renaming it into `turns/tNNNN/cause.md` and writes `meta.toml` (`started`, `from`, `kind`, `transcript_lo`); at the turn's end `meta.toml` gets `ended`, `verdict`, `outcome`, `transcript_hi`. A peer's request is appended to the transcript as a `say` at claim time; a brief becomes the mission prompt; the user's words become the `user` line the turn writes. Notes (`wake = false`) go onto the transcript as any turn starts. A claim that fails (read-only `turns/`, full disk) is reported to clients once and backed off a minute.
- The window's queued rows come from the inbox: plan frames carry the files as `inbox: true` rows with ids above bit 40; `plan_op cancel` removes the file, `run` sets `wake = true`. Send now / Edit / Remove in the desktop work unchanged.
- Kernel restart: files in `inbox/` are still there and fire; a file written by hand while the kernel is down fires at start (verified).
- Not yet: steer (a message to a running turn) stays in memory; job-done notices still append directly; `TurnControl.steer`, `is_inbox`, `hops` on nodes remain compiled for the legacy plan-node path (old `plan.jsonl` inbox nodes still fire).

Verified: prompt → `inbox/…-user-0.md` → `turns/t0001/cause.md` → turn; a second prompt during a turn waits in the inbox (window shows it as a follow-up row) and runs after; spawn brief → child `turns/t0001/cause.md` with `kind = "brief"`; child's `say request` → root `turns/t0005` from `agent:<child>` and the `say` line; hand-written note + request with the kernel down → on start the note lands before the wake and the model quotes it. Kernel e2e: 18 pass; `recreate_e2e` fails on the integration head too. `stop_e2e` and `claim_backoff_e2e` rewritten for the turn folder.

## Attack surface

- Two kernels (old and new) on one place: the old one still writes inbox nodes into `plan.jsonl`; the new one fires both kinds.
- A file with malformed front matter in `inbox/` (skipped by `list`, silently — should `check` flag it? yes, follow-up).
- `hops` accounting across file-borne requests (same numbers as before: `DEFAULT_HOPS` on requeue, `hops_in - 1` on say).
- `turns/` growth: one folder per turn, two small files each; nothing prunes them (git holds them once Phase 1 is in).
- Rewind (#80/#82) cuts the transcript but leaves `turns/` folders for cut turns — stale `meta.toml` entries; say if that matters before Phase 3.
- The desktop's "1 follow-up queued" row ids change every kernel scan? No — the id is a hash of the file name, stable.
