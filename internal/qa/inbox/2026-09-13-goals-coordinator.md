---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# Cursor agent model, slice 1 — PR #98, branch `cursor/goals-coordinator-b027` (on #92)

## What changed

- `.arbos/GOALS.md`: template at bootstrap; injected first after CONTRACT when it has content (cap 16 000 chars, note over that). `write`/`edit`/`apply_patch` on it from a child agent are refused at plan time; a top-level agent may. `bash` is not stopped (it can write any file).
- `.arbos/project.toml` with `[root] role = "coordinator"` for a place that has never had a root. Existing places: unchanged until the line is added. With the role, a top-level agent's tools narrow in memory (no write/edit/apply_patch/bash/await/terminal/record/secret/undo) and its prompt says `Role: coordinator — …`. Children keep the saved (full) allowlist.
- `kind = "done"` inbox file to the parent at every child turn end (wake = true; body = last words + transcript path), except when the parent was blocked in `spawn wait=true`.

## How to check

1. Fresh place → `arbos-kernel run`: `.arbos/project.toml` and `.arbos/GOALS.md` exist; root's transcript shows no edit tool use; a `spawn` child edits and its turn end appears as `root/inbox/*-agent-<child>-*.md` with `kind = "done"` (or `root/turns/tNNNN/cause.md` once claimed).
2. Child `write path=.arbos/GOALS.md` → tool error "owned by the main chat (root); propose the change with `say to=root`".
3. Existing place with no `project.toml` → root keeps `edit`/`bash`.
4. Put real text under `## Goal` in GOALS.md → the next turn's prompt includes it (see `rollout export` or `ARBOS_TRACE`).

## Watch for

- A coordinator + worker chatting in circles: the parent wakes on each `done`; if it answers with `say mode=request` each time, the pair loops. The hop budget bounds agent-to-agent chains at 3, but `done` is from the kernel (hops 0). Tell me if you see a runaway pair.
- The CONTRACT still says `Focus = .arbos/focus` while the file lives in `runtime/`; a root's first `read .arbos/focus` fails (seen once). Cosmetic, on the list for the plan-engine PR.
