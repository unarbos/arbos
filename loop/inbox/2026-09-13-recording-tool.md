# A-01 recording artifacts — QA note (features agent, 2026-09-13)

Branch `cursor/recording-tool-b027`, stacked on #16 (`cursor/screenshot-tool-b027`), base `rust`.

## What it does

A `record` tool with three ops:

- `record start` (optional `max_secs`, default 120, cap 600; optional `display`): starts a screen recording in the background and returns at once. One recording per agent at a time.
- `record stop`: ends it, waits for the encoder to finish the file, returns the path, length, size, and attaches a **poster frame** (last frame as PNG) so the model and the transcript show what was recorded.
- `record status`: running or not, elapsed seconds.

Backends: Linux X11 `ffmpeg -f x11grab` (mp4, 15 fps, h264 yuv420p); Wayland `wf-recorder`; macOS `screencapture -v` (mov). The file lands in `.arbos/agents/<id>/recordings/record-<ms>.mp4|mov`. `max_secs` is enforced by the encoder itself (`-t` / `-V`), so a forgotten recording ends on its own.

Prompt line: "record start … record stop makes a screen recording (video file + last frame) for the user; use it to show a flow, screenshot for one moment."

Allowlist: `record` is on `ALL_TOOLS`; an agent with `screenshot` (or `browser`) may record too (same read-only-screen surface). `readonly` keeps it.

## Attack ideas

1. `record start` twice: second call must say a recording is already running with its elapsed time, not start a second encoder.
2. `record stop` with nothing running: clean message, no panic.
3. Kill the kernel while recording: the encoder must die with it (child of the kernel; `-t` cap as the backstop). Check no orphan ffmpeg after `pkill arbos-kernel`.
4. `max_secs: 0`, negative, 10000, string: rejected or clamped to 1..600 with a message.
5. No DISPLAY in the kernel env: `record start` fails with the same hint as screenshot ("no DISPLAY or WAYLAND_DISPLAY…").
6. Stop 0.3 s after start: file may be tiny or empty; the tool must report the actual size and not claim a poster it could not extract.
7. Two agents recording at once on one display: both files fine (x11grab allows it).
8. Turn cancelled (user stops the turn) mid-recording: the recording keeps going until stop or `max_secs`; check the next turn's `record status` still sees it.
9. Path in `ToolOut.paths` is the video, `images` is the poster only — the desktop must not try to render the mp4 inline.
10. Recording on the QA VM display `:1` during a `bench-screenshot` run: the video shows the desktop; the poster PNG opens.

## How to run

Display `:1` on the VM (`internal/parity/env.sh`). `cargo build -p arbos-kernel`; prompt: "record start, open a terminal and run ls, wait 5 seconds, record stop, tell me the file path and size". Check `.arbos/agents/root/recordings/` for the mp4 + poster PNG, `ffprobe` the duration.
