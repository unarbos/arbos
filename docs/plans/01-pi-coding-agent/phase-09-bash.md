# Phase 9 — bash tool

Back to [overview](./overview.md). Mirror `coding-agent/src/core/tools/bash.ts`.

## Goal

Port pi's bash: stream output, tail-truncate safely, time out, kill the process
tree, with pi's exact notices.

## Status: done

Implemented in `codingspec/bash.go`. Runs `bash -c` in the workspace root, merges
stdout+stderr, tail-truncates to the last 2000 lines / 50KB, spills full output
to a temp file with the verbatim notice, surfaces exit code / timeout / abort
with pi's exact status strings, and kills the whole process group (`Setpgid` +
`kill(-pgid)`) on timeout or ctx-cancel. Output is not trimmed (matching pi's raw
`snapshot.content`). The tail-truncation helper carries the granted unit tests.
Runtime-proven: success, stderr capture, exit-code surfacing, a 30s sleep killed
at its 1s timeout, large-output truncation + a real spill file, and ctx-cancel
aborting in 0.5s. Unix-only process semantics (the kernel targets Linux).

## pi behavioral spec

Input `{ command, timeout? }` (timeout in seconds, no default). Run in the
workspace cwd via the login shell. Stream stdout and stderr through one
accumulator. Truncation is **tail** (`truncateTail`), keeping the END:
`DEFAULT_MAX_LINES = 2000`, `DEFAULT_MAX_BYTES = 50KB`, whichever hits first.
When truncated, the full output spills to a temp file (prefix `pi-bash`; for
arbos, spill under the workspace temp) and the result appends one of:
- lines: `[Showing lines {start}-{end} of {total}. Full output: {path}]`
- bytes: `[Showing lines {start}-{end} of {total} ({50KB} limit). Full output: {path}]`
- partial last line: `[Showing last {size} of line {end} (line is {lineSize}).
  Full output: {path}]`

Exit handling. Non-zero exit throws an error result whose text is the output
followed by `\n\n` + `Command exited with code {code}`. Timeout: kill the tree,
`Command timed out after {secs} seconds`. Abort (ctx cancel): kill the tree,
`Command aborted`. Empty output is `(no output)`.

Process control. Spawn detached (non-Windows) so the whole tree can be killed;
on timeout or abort, `killProcessTree(pid)`. Honor ctx cancellation.

`promptSnippet: "Execute bash commands (ls, grep, find, etc.)"`. Tool description
ported verbatim. Not read-only.

## Data structures

`BashArgs{ Command string; Timeout int }`. Result uses Phase 1 `Details` for the
truncation record and spill path.

## Dependencies

Phase 4.

## Verification

Static. Build, vet, test -race, generate-check, lint.

Runtime. Harness asserts: a >2000-line command tail-truncates with the exact
`Full output:` notice and a real spill file; a non-zero exit surfaces
`Command exited with code N` with captured output; a `sleep` past its timeout
yields `Command timed out after Ns`; an interrupted long command kills the tree
and ends the turn.

Unit (granted exception). A case table for the pure tail-truncation helper: line
cap, byte cap, last-line-partial (UTF-8 boundary safe).
