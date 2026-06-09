# Phase 6 — grep parity

Back to [overview](./overview.md). Mirror `coding-agent/src/core/tools/grep.ts`.

## Goal

Full grep parity over ripgrep, respecting `.gitignore`, with pi's exact notices.

## Status: done

Implemented in `codingspec/grep.go`: `rg --json` parse, the `path:line:` and
`-line-` context formats, `--ignore-case`/`--fixed-strings`/`--glob`, match-limit
early stop, byte and long-line truncation, and the verbatim notices. `rg` is
hard-required (D11). Runtime-proven against a `git init` fixture: row format,
`.gitignore` exclusion, context blocks, the `Use limit=2` notice, `No matches
found`, and glob filtering. (rg honors `.gitignore` only inside a git repo, pi's
normal environment.)

## pi behavioral spec

Input `{ pattern, path?, glob?, ignoreCase?, literal?, context?, limit? }`.
`DEFAULT_LIMIT = 100`, `GREP_MAX_LINE_LENGTH = 500`, byte cap `50KB`.

rg invocation: base args `["--json", "--line-number", "--color=never", "--hidden"]`,
plus `--ignore-case` when `ignoreCase`, `--fixed-strings` when `literal`,
`--glob {glob}` when set, then `-- {pattern} {searchPath}`. Parse the `--json`
stream, counting `match` events; stop the child once `limit` matches are reached.

Row format. No-context match: `{relPath}:{lineNo}: {text}`. With `context > 0`:
a block where the match line uses `{relPath}:{lineNo}: {text}` and context lines
use `{relPath}-{lineNo}- {text}`. `relPath` is relative to the search dir when it
is a directory, else basename. Each line is `truncateLine`-capped to 500 chars
with a `... [truncated]` suffix.

No matches: `No matches found`. Path missing: `Path not found: {path}`. rg
absent: `ripgrep (rg) is not available and could not be downloaded` (we instead
fail clearly; no auto-download per the retained position).

Notices appended as `\n\n[{joined}]`:
- match limit hit: `{limit} matches limit reached. Use limit={limit*2} for more,
  or refine pattern`.
- byte cap hit: `{50KB} limit reached`.
- lines truncated: `Some lines truncated to 500 chars. Use read tool to see full
  lines`.

`promptSnippet: "Search file contents for patterns (respects .gitignore)"`. Tool
description ported verbatim. Honor ctx cancel by killing the rg child.

## Data structures

`GrepArgs{ Pattern string; Path, Glob string; IgnoreCase, Literal bool; Context, Limit int }`.

## Dependencies

Phase 4 (supersedes the placeholder grep).

## Verification

Static. Build, vet, test -race, generate-check, lint.

Runtime. Harness runs grep over a fixture with a `.gitignore` and asserts: ignored
paths excluded, the `path:line:` row format, a context block's `-line-` form, the
`Use limit=200` notice at the cap, and the clear error when `rg` is off PATH.
