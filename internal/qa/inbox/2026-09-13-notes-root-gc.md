---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# Project page + gc chore + check lints — PR #107, branch `cursor/notes-root-gc-b027` (on #106)

- Root's `plan` tool writes **`.arbos/notes.md`** (the project page, Cursor's `notes.md`). Children write `agents/<id>/notes.md`. A child's `write`/`edit`/`apply_patch` on `.arbos/notes.md` is refused with a pointer to `say to=root`; `bash` is not stopped (as with GOALS.md).
- Every kernel start makes sure root has one `shell` subscription `git -C .arbos gc --auto --quiet`, `every = "7d"`, `deliver_to = "none"` (quiet: nothing on success; a failure wakes root). It is `#1` in a fresh place. Removing it makes the next start re-add it — say if that is wrong.
- `arbos-kernel check` now covers `.arbos/notes.md`, `waiting/*.toml`, and `access.toml` (no client rows; token file mode).

## Scenario ideas

- Fresh place: `subscriptions/0001-weekly-git-gc-….toml` exists; `check` clean. Set `ARBOS_NOW` eight days ahead and start with `--until-idle`: the chore runs (a `jobs/jN` folder under root with exit 0), `last = "exit 0 — quiet"`, no inbox file, no turn.
- Break the chore: edit its `cmd` to `git -C .arbos gc --nope`; force with `ARBOS_NOW` → root wakes once with "Subscription #1 (`…`) failed: exit 129".
- Child tries to edit the page (`write path=.arbos/notes.md`) → refused; root can.
- Migrate an old place whose root had a plan: the open lines land in `.arbos/notes.md`, not under `agents/root/`.
