# Phase 7 — write tool

Back to [overview](./overview.md).

## Goal

Full-file `write`, faithful to pi's create-or-overwrite semantics.

## Status: done

## Correction to the earlier sketch

Reading pi's `write.ts` shows `write` does NOT preserve BOM or line endings. It is
a verbatim create-or-overwrite (`writeFile(path, content, "utf-8")`) that creates
parent directories and returns `Successfully wrote N bytes to {path}`, where N is
pi's `content.length` (Unicode code points). Only `edit` preserves BOM and line
endings. The implementation matches pi: verbatim write, `mkdir -p`, that exact
message, the sandbox guard.

## Changes (as implemented)

- Added `write` to `codingspec` (not read-only): resolve through `tool.Resolve`,
  `MkdirAll` the parent, write content verbatim, return the success message.

## Data structures

- `WriteArgs{ Path, Content string }`.

## Dependencies

Phase 4.

## Verification

Static. Build, vet, test -race, generate-check, lint.

Runtime. Harness writes a new file and overwrites an existing one, asserting
exact bytes including preserved line endings, and asserts traversal is refused.
