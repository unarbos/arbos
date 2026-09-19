---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# The grep index never learns of files written after the kernel started

Found by the features agent's own probe, 2026-09-19, on `main` `249ddb5f`.

## What happens

The kernel builds the trigram index once, at start (`PlaceGrep::start`),
and `grep` uses it whenever it is ready. `PlaceGrep::upsert` exists and is
called from nowhere. So the set of files the index knows is frozen at the
kernel's start, for the life of the kernel — days, for a kernel a desktop
keeps serving.

`search` re-reads each candidate file before matching, so an edited file
the index already knew *can* still hit if its old trigrams still cover the
pattern. But a file **created** since start is never a candidate, and text
**added** to a known file is missed when its trigrams were not there
before. Probe, one turn:

```
grep zebra                                 → (no matches)
write  src/new.rs   "fn zebra() {}"        → wrote
edit   a.txt        hello → hello zebra     → edited
bash   echo 'zebra in shell file' > shell.txt
grep zebra                                 → (no matches)
```

Three files hold the word the agent just wrote; the search says none. The
model's own edits are invisible to its own grep, `git checkout` of another
branch in bash the same, a person's edits in their editor the same. A
"no matches" is read as "not in the codebase" — a misreport on the tool
that answers "where is X?" all day.

The walk (`grep_walk_with`) is always right and is used only while the
index is not yet built.

## What should hold

The kernel knows when the tree changed: every tool that writes (`write`,
`edit`, `apply_patch`, `delete`, a non-read-only `bash`) ends with the
paths it touched. After one, the index is stale: `grep` walks (correct,
slower) until a background rebuild lands, then the fresh index takes over.
No rebuild storm: one rebuild at a time, started when the tree is dirty
and none is running.

Fixed in the same cycle: [#778](https://github.com/unarbos/arbos/pull/778).
