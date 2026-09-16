# Arbos Project store — mirror of `docs/`, `notes.md` and `internal/` (within a boundary)

This branch is a backup, not code. It has no shared history with `main` and is never merged.

It mirrors two things from the Arbos Project's Cursor Agent Store
(`/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983`):

- `docs/` — the Project's written deliverables, at the same names and paths the store uses,
  so a link of the form `docs/<name>.md` means the same file here and there.
- `notes.md` — the Project status page, for context on what the documents refer to.
- `internal/` — working tooling, reports, pending instructions and small state, at the store's
  own paths. Since 2026-09-16 (the second loss took `internal/parity/` and
  `internal/features-inbox/`). **Boundary:** every file under `internal/` except run output and
  caches (any folder named `rollouts`, `staging`, `state`, `node_modules`, `.venv`,
  `__pycache__`, `target`, `.git`), binaries (images, audio, video, archives, compiled files),
  and files over 2 MB. So bug files, inbox notes, scripts, the parity rig, history `.jsonl`
  files and reports are protected; rollout bundles, screenshots and big logs are not — their
  owners keep their own copy.

Not mirrored: `media/` (large binaries), `artifacts/` (platform folder), and the excluded
paths above.

## The convention

**One branch, `store-docs`, with `docs/` at the root.** Not inside a code branch, not under a
`store/` prefix, not one branch per author. The whole point is a single place a person can look.

## Mirror after you write a document

```bash
bash /cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/mirror-docs.sh
```

The script is also at the root of this branch, so it survives a store loss:

```bash
git show origin/store-docs:mirror-docs.sh > /tmp/mirror-docs.sh
```

It pushes only when something changed, refuses to push a store view that looks broken or that would
shrink the mirror, and never touches your checkout's branch, index or working tree.

## Restore the store from this branch

```bash
bash mirror-docs.sh restore /tmp/mirror-restore
cp /tmp/mirror-restore/docs/*.md /cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/
```

## Why it exists

On 2026-09-16 the store dropped `docs/` and `artifacts/` with no event, no audit trail the owner can
read and no undo. The only documents recovered whole were the four that happened to be mirrored into
the repository. Account: `internal/store-docs-loss-2026-09-16.md` in the store.
