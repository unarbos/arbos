# Phase 4 — Coding toolset scaffold and codegen

Back to [overview](./overview.md). Status: done.

## Goal

Stand up the coding-tool packages and prove the build-time schema codegen path
end to end with one tool, targeting the Phase 1 result shape.

## Decision (deviation from the original sketch, recorded as D10)

The proof tool is `ls`, not `grep`. grep depends on ripgrep and is only fully
specified in Phase 6, so scaffolding with grep would have meant throwaway code.
`ls` is trivial, read-only, dependency-free, and a real tool, so scaffolding with
it delivers a kept tool. `ls` lands here; Phase 5 covers `read` and `find`.

## Changes (as implemented)

- New `internal/tool/codingspec`: coding tool arg structs, handlers, and
  metadata, standalone so the generator imports it without a bootstrap cycle.
  Mirrors `builtinspec`. Holds `ls` so far.
- New `internal/tool/coding`: a `NewRuntime(root)` assembler pairing each
  `codingspec` spec with its generated schema. Mirrors `builtin`.
- New shared `tool.Resolve` (`internal/tool/sandbox.go`): the workspace sandbox
  guard every file tool needs, so the security-critical path-resolution logic has
  one home. Ported verbatim from the proven `builtinspec` logic.
- `cmd/gen-tool-schemas` gained a `-specs builtin|coding` flag; the `coding`
  package's `go:generate` directive points it at `codingspec`. One generator
  serves every tool package.

## Data structures

- `LsArgs{ Path string }`.

## Dependencies

Phases 1 to 3.

## Verification (done)

Static. build, vet, test -race, generate-check clean, scoped lint 0 issues.

Runtime. A control-cli harness built `coding.NewRuntime`, asserted the generated
`ls` schema (read-only, valid JSON), dispatched `ls` through the real runtime
(correct listing), confirmed lexical traversal clamps to root, and confirmed a
symlink escape is refused by the shared sandbox guard.
