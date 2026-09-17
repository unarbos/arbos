---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# For QA: `screenshot` tool (K-06)

From the features agent. Branch `cursor/screenshot-tool-b027` → `rust`.

## What I am building

A `screenshot` tool the model can call to capture the machine's screen (not a browser page — `browser screenshot` already does that). Arguments: `target` = `screen` (default) or `window` (frontmost window on macOS; on Linux the focused X11 window); optional `display` index. The PNG lands in `.arbos/agents/<id>/images/screen-<ms>.png`, is attached to the turn as pixels (the model sees it), and is cited by path so the desktop can open it. Backends by platform, first that exists wins: macOS `screencapture -x`; Linux `grim` (Wayland), `import -window root` (ImageMagick, X11), `scrot`, `gnome-screenshot -f`. No backend or no display → a clear error naming what to install and `$DISPLAY`/`$WAYLAND_DISPLAY`. Read-only: it stays in the readonly allowlist and does not count as a write.

Benchmark item 3: "send back a screenshot the user can see".

## How to exercise it

Under Xvfb (`Xvfb :99 &; export DISPLAY=:99`) with ImageMagick installed: `arbos-kernel run "Take a screenshot of the screen and tell me its size."` Expect a tool line with the path and dimensions, a PNG in the agent's `images/`, and the model describing a black/empty screen. Without `DISPLAY`: expect the error, no file.

## What could break — attack here

1. No display at all (ssh session, `DISPLAY` unset) → error, exit code of the turn is still a normal completion.
2. Backend present but fails (X server gone mid-call, permission denied on macOS Screen Recording) → error text includes the backend's stderr; no zero-byte PNG left behind.
3. Very large screens (5K, multi-monitor) → PNG size; the image budget keeps only the newest 8 images as pixels; check the model still gets it and the transcript cite is right.
4. `target=window` with no focused window; `display=7` that does not exist.
5. Readonly child agent calling it: allowed (it is a read). A child with `screenshot` removed from its allowlist: refused.
6. Ten screenshots in one step (parallel tool calls): distinct file names (ms timestamps can collide — the tool appends a counter if the name exists).
7. Timeouts: `screencapture` waiting on a permission dialog; the call has a 15 s cap.
