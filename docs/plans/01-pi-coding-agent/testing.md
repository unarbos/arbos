# Testing and verification strategy

Back to [overview](./overview.md).

The project rule is no unit tests unless asked. This plan proves behavior with a
runtime harness on the real surface, with one granted exception for pure
functions whose edge cases are impractical to cover at runtime.

## Control-cli runtime harness

The harness is the artifact a reviewer reruns and the runtime gate for every
phase.

- It roots the coding tool registry at a throwaway temp workspace seeded with
  known files.
- It drives behavior through the real `ToolRuntime.Dispatch` path. For tool-level
  checks it dispatches calls directly. For full-session checks it scripts arbos's
  existing `fake.Provider` to emit specific tool calls and (Phase 3) reasoning
  levels, so a complete pi session runs deterministically with no real LLM and no
  network.
- It asserts observable outcomes: resulting filesystem state, the model-facing
  result text (continuation and truncation notices, edit diffs and error
  strings), emitted events, and `TurnComplete`. Result-text fidelity is a
  first-class assertion, since that text is itself pi intelligence.
- It is one harness extended per phase, not rebuilt.

## Granted unit-test exception

Two pure functions carry focused case-table unit tests.

- The `edit` matcher (Phase 8), now including the fuzzy normalization cases:
  exact, smart quotes, en/em dash, NBSP and other special spaces, trailing
  whitespace, not-found, non-unique, overlap, empty, no-change, CRLF and BOM.
- The `bash` truncation helper (Phase 9): line cap, byte cap, last-line-partial.

Everything else is proven through the control-cli harness.

## Foundational phases (1 to 3)

The kernel shape changes additionally assert that existing engine, projection,
and adapter tests stay green, since these phases touch shared types every other
package depends on.

## Static gate

Every phase boundary runs `go build ./...`, `go vet ./...`,
`go test ./... -race`, `make generate-check`, and `golangci-lint run`.
