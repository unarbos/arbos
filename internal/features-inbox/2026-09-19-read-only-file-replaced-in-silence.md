---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# A read-only file is replaced in silence by three editors and refused with a bare "Permission denied" by the fourth; a write under a file says "File exists"

Found by the features agent's own probe, 2026-09-19, on `main` `5501a2f6`. A
file the user (or a generator) marked read-only — `chmod 444`, as generated
code, vendored trees and lock files often are, precisely so editors refuse.

## What happened

`ro.py`, mode 444, one line `x = 1`:

| tool | what it did |
|---|---|
| `edit` (old_string/new_string) | **replaced the file** (`replace_file`: temp + rename over it; the mode 444 kept, the content gone) |
| `edit_all` | same path, same result |
| `write` (whole file) | **replaced the file** |
| `apply_patch` (Update File) | **replaced the file** |
| `edit` by `LINE:HASH` (hashline) | `Permission denied (os error 13)` — no path, no what |

Three editors override a protection the person set, in silence; the fourth
refuses with a bare OS error. Cursor and VS Code refuse to save a read-only
file without an explicit override.

Beside it: `write` to `ro.py/child.txt` (a parent that is a file) failed
with `File exists (os error 17)` — the folder-creation error, naming
nothing.

## What should hold

Every editor refuses a read-only file with the path, the mode, what such a
file usually is, and the way through: `chmod u+w <file>` in bash first
(and a word on why), or change its source. `write` under a file names the
parent that is a file.

Fixed in the same cycle: see the PR linked from `docs/features-backlog.md`
(row dated 2026-09-19, "read-only file").
