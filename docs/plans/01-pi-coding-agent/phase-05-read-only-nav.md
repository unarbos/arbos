# Phase 5 — Read-only navigation tools

Back to [overview](./overview.md). `read`/`find`/`ls`, all read-only (parallel
dispatch). Result-text fidelity is an acceptance criterion.

## Goal

Port `read` and `find` with pi's exact model-facing output, including images.
(`ls` already shipped in Phase 4 as the scaffold proof tool, per D10.)

## Status: done

`read`, `find`, and an upgraded full-parity `ls` are implemented in `codingspec`
(read.go, find.go, ls.go, truncate.go). `find` hard-requires `fd` (D11). Three
deliberate, documented deviations on `read` image handling: image type is
detected by file extension (pi uses magic bytes), images are not auto-resized
(no image library in the kernel), and the non-vision downgrade note is omitted
because the tool layer does not know the active model's vision capability (that
gating belongs to the model registry / adapter). Runtime-proven: line truncation
notice ("Showing lines 1-2000 of 3001. Use offset=2001 to continue."),
offset/limit continuation, the >50KB sed fallback, an image content block, full
`ls` parity, and `find` returning matches via `fd`.

## pi behavioral spec — read (`coding-agent/src/core/tools/read.ts`)

Input `{ path, offset?, limit? }`. `offset` is 1-indexed. Thresholds from
`truncate.ts`: `DEFAULT_MAX_LINES = 2000`, `DEFAULT_MAX_BYTES = 50KB`,
whichever is hit first. read uses head truncation (`truncateHead`, never partial
lines).

Text path:
- Split into lines. `offset` beyond EOF throws
  `Offset {offset} is beyond end of file ({n} lines total)`.
- Apply `limit` if given, then `truncateHead`.
- Single line exceeds the byte limit: `[Line {L} is {size}, exceeds {50KB}
  limit. Use bash: sed -n '{L}p' {path} | head -c {50KB}]`.
- Truncated by lines/bytes: append `\n\n[Showing lines {start}-{end} of {total}.
  Use offset={end+1} to continue.]` (the byte variant adds the `({50KB} limit)`
  note).
- User `limit` stopped early with more file left: `\n\n[{remaining} more lines in
  file. Use offset={next} to continue.]`.

Image path: detect mime; auto-resize to <=2000x2000 by default; return a text
note `Read image file [{mime}]` plus an image block (Phase 1/2). When the model
lacks vision, append `[Current model does not support images. The image will be
omitted from this request.]`.

Tool description ported verbatim. `promptSnippet: "Read file contents"`.
`promptGuidelines: ["Use read to examine files instead of cat or sed."]`.

Note: pi's compact-render classification of SKILL.md / AGENTS.md / docs is TUI
rendering only, not model-facing. Skip it.

## find and ls

`find` does name globbing under a root; `ls` a rich listing. Both read-only,
sandbox-guarded. (No exotic model-facing format beyond a clean path list.)

## Data structures

`ReadArgs{ Path string; Offset, Limit int }`, `FindArgs{ Pattern, Path string }`,
`LsArgs{ Path string }`.

## Dependencies

Phases 1, 2, 4.

## Verification

Static. Build, vet, test -race, generate-check, lint.

Runtime. Harness asserts: a 3000-line file truncates at 2000 with the exact
`Use offset=2001 to continue` notice; a huge single line emits the `sed`
fallback; an image read returns an image block; a non-vision model gets the
omitted-image note; a two-call read-only batch dispatches concurrently.
