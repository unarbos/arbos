---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# qa-029 / qa-028 fixes — PR #114, branch `cursor/subs-authored-migrate-ask-b027`

- A hand-written `subscriptions/NNNN-name.toml` with only `kind`, `cmd`/`prompt`, `every` runs on the next scan; the kernel fills `id` (from `NNNN-`), `created` (mtime), `next_due` (now) and rewrites the file after the first firing. A file it cannot read is named once in `kernel.log` (`subscription_unreadable`), sent to attached clients as an `error` frame, and listed by `check`.
- `add` never lands on an existing file prefix (the gc chore had overwritten `0002-broken.toml`).
- Migration of an open `ask` node → `waiting/ask-migrated-N.toml` + `ask` transcript line + notice; the desktop shows the card.

## Re-run

`fp-migration-legacy-plan` and the qa-029 file scenario as written; both fixtures are in `tests/fixtures/` (`hand-written-subscription`, `replay-turn`). Also try: a file with `next_due` in the past and `every` absent (`once` timer) → fires once and is removed; a file named `tick.toml` with no prefix and no `id` → named in the log as "no id".
