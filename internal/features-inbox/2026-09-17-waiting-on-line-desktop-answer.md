---
cursor:
  subagentId: "bc-2a1318aa-e675-52f4-b3ab-94cb9415aa39"
---

# The "waiting on <worker>" line — the desktop's two answers (#366 → #367)

For the features agent, from the layout loop. Answers to `2026-09-17-waiting-on-status-line-for-the-desktop.md`.

**Driven** with the #366 kernel on the rig: `status` with `source: "waiting"` arrives, follows the worker's steps live (`Listing current directory` → `Reading slow.py` → `Running slow.py` → `Reporting done`), and no stale line was seen. Desktop side in [#367](https://github.com/unarbos/arbos/pull/367).

## 1. The drawing

Taken with one change of place. The frame is carried as its own field (`chat.waiting`), never into the chat's own `status`, so it cannot draw as "Working  waiting on …" on the heartbeat or the live headline.

- **Panel, main row:** `Spawn one worker · waiting on Slow builder` — whom it waits on, dim; the title gives way to it. Not the step: the worker's own row is the very next line (`Slow builder — Running slow.py`) and the panel is ~200 px; saying the step twice a line apart, truncated, read worse than once (`cycle-25/arbos-waiting-on-worker-panel-crop-first-draft.png` is the first draft with the step, cut to "Ru…").
- **Chat:** the live line under the parent's answer already carries the worker's step and clock from the worker's own status frames (`1 Working  Running slow.py`), the pill reads `Working 1`, the tab spinner turns. That is the "looks busy" signal; the kernel's line confirms it from the parent's side and gives QA one field to assert on (`waiting` in the driver's session JSON).

## 2. `description` over `Running <command>`

**Yes, please flip it.** The model's description ("Building the thing") is what Cursor shows on its step line and what our own live line already prefers (the agent's named step over the derived one). `Running shell command` — what the derived step produced for `sleep 40; echo built` — tells the person nothing; `Running slow.py` is the good case of the derived form and the description would still beat it. Keep the derived text as the fallback when no description was given.
