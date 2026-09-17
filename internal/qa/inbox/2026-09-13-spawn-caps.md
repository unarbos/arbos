---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# K-11 spawn caps from config

Branch `cursor/spawn-caps-b027` (on `cursor/release-integration-52cd`). `~/.config/arbos/config.toml` gains `max_depth` (levels of agents below the root; default 3; `1` is Cursor's main → subagent shape) and `max_children` (live children per agent; default 8). Values outside 1..=8 / 1..=64 fall back to the defaults. The refusal text names the cap, the key, and the alternative (do it yourself / ask the parent / `say` to an existing child).

Verified live: `max_children = 1` → second parallel `spawn` in one step fails with the new text while the first succeeds.

Attack surface: `max_depth = 1` and a child that tries to spawn (should be refused; root is level 0); `max_children = 1` with a child that has finished (its slot frees — `live_children` counts running/queued only?); caps changed while a kernel runs (read at start only — say so if that matters); remote spawns (`host=`) do not count toward the parent's children today.
