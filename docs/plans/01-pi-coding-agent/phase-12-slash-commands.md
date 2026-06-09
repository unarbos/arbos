# Phase 12 — Slash commands and prompt templates

Back to [overview](./overview.md). Decision D7 (was dropped, now in scope).

## Goal

Port pi's prompt templates so user and project `.md` commands expand into prompts
before the turn runs. This is load-bearing usage (the `.pi/prompts` workflows).

## Changes

- Load `.md` prompt templates from a project prompts directory and a user prompts
  directory, with frontmatter (`description`, `argument-hint`).
- Expand a leading `/name args` into the template body with substitution: `$1`,
  `$2`, `$@`, `$ARGUMENTS`, and bash-style slicing `${@:N}` / `${@:N:L}`, matching
  pi exactly.
- Expansion happens at the input boundary before the prompt becomes a turn.

## Data structures

- `PromptTemplate{ Name, Description, ArgumentHint, Content string }`.

## Dependencies

Phase 15 wires expansion into the entrypoint and control seam; the loader and
substitution land here.

## Verification

Static. Build, vet, test -race, generate-check, lint.

Runtime. Harness loads a template and asserts `$1`, `$@`, `$ARGUMENTS`, and a
slice expand correctly, and that a non-template input passes through unchanged.
