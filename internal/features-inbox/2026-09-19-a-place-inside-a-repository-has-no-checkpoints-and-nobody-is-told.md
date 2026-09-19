---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# A place that is a subfolder of a repository has no checkpoints, and nobody is told why

Found by the features agent's own probe, 2026-09-19, on `main` `249ddb5f`. A
machine we did not choose: the person opens `repo/packages/app` as the
project — the common monorepo shape — not the repository's root.

## What happens

`snapshot_turn_record` writes a checkpoint only when `<place>/.git` exists.
A subfolder of a repository has no `.git` of its own, so every turn there
starts with **no checkpoint**: `rewind` and `undo` are off, and the record
says nothing about why. Probe: a repository with `packages/a` (the place)
and `packages/b`; a turn's record for `packages/a` returned `None`.

What the person is then told:

- `rewind`: *"root has no checkpoints yet (they are written when a turn
  starts, from this version on)"* — turns did start; none was written, and
  the reason is the folder, not the version.
- `undo`: *"no checkpoint for this turn (its mark was never written, or its
  write failed and was said on the transcript)"* — nothing was said on the
  transcript.
- At start: nothing. `git_missing` speaks when the binary is absent; a
  folder inside a repository is silent.

The same silence holds for a place that is no repository at all (common and
often deliberate — a notes folder), where the honest line is one sentence.

Not touching the whole-repository restore is right: a rewind of
`packages/a` that reset the repository would have reached the person's
uncommitted work in `packages/b`. The refusal is correct; its words are not.

## What should hold

One classification — the folder is a repository, has a `.git` with no
commit yet, is inside a repository rooted elsewhere, or is no repository —
and its sentence in `rewind`'s and `undo`'s refusals. For the
inside-a-repository case, said once at start on the main chat: where the
root is, that checkpoints work there, and the two ways (open the root as
the project, or `git init` here to make the folder its own).

Fixed in the same cycle: see the PR linked from `docs/features-backlog.md`
(row dated 2026-09-19, "inside a repository").
