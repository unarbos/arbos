# qa-004: any attach client can write an arbitrary string into `.arbos/focus`, and a focus that points nowhere is kept and re-sent

status: pr-open — https://github.com/unarbos/arbos/pull/11 (branch `cursor/fix-qa-004-focus-validation-de28` -> `rust`)
severity: low-medium (local loopback only today; becomes a real problem once attach is network-reachable per the "reach agents from anywhere" goal)
scenario: malformed-frames, malformed-folder
rollout: /cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/qa/rollouts/20260912T223723Z-malformed-frames (write); .../20260912T223640Z-malformed-folder (kept)
fingerprints: a31efd9056 47e5f7e435

## Repro

1. Attach and send `{"type":"focus","path":"../../../../etc/passwd"}`.
2. Read `.arbos/focus`.
3. Separately: pre-write `.arbos/focus` as `.arbos/agents/does-not-exist`, start the kernel, attach, read the `snapshot` frame.

## Expected

- The kernel rejects a focus that is not an existing agent folder under the place and tells the client.
- On start, a dangling focus is reset to `.arbos/agents/root`.

## Actual

- `.arbos/focus` now holds `../../../../etc/passwd` (`result.json` notes `focus_after_traversal`).
- The dangling focus survives boot and is sent to every client in `snapshot.focus` (`state-after/focus` in the malformed-folder rollout). The desktop then tries to open that path.

## Suspected location

- `crates/arbos-kernel/src/serve.rs:349-351`: `Frame::Focus { path } => write_focus(place, &path)` with no check.
- `crates/arbos-core/src/files.rs:136-139` `write_focus`, `files.rs:78-80` bootstrap only writes focus when the file is missing.
- `serve.rs:422-432` `snapshot` forwards whatever the file says.
