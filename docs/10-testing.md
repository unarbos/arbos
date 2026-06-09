# 10 — Testing & TDD conventions

> Status: scaffold. Built early because test infrastructure benefits every later
> phase. The goal is that adding an adapter (SQLite store, real provider,
> transport agent, …) is test-driven: wire it into the contract suite, watch it
> fail, implement until green.

## The model: contract suites over a ports architecture

arbos is ports + adapters, so the highest-leverage test scaffold is a
**contract suite per port**: one adapter-agnostic suite that any implementation
must pass. The reference fakes pass them today; every real adapter copies a
one-line entry point.

Suites live in `internal/ports/porttest/`:

| Port | Suite | What it pins |
|---|---|---|
| `SessionStore` | `RunSessionStoreContract` | Seq monotonicity, ID assignment, `Validate` enforcement, ordered + copied reads, update/lookup errors, cross-session concurrency |
| `LLMProvider` | `RunLLMProviderContract` | non-empty `Name`, stream always closes, honored `ctx` cancellation (no leak/hang) |
| `ToolRuntime` | `RunToolRuntimeContract` | well-formed schemas, no panics, error-as-data, `CallID` round-trip |

Usage from an adapter's `*_test.go`:

```go
func TestSQLiteStoreContract(t *testing.T) {
    porttest.RunSessionStoreContract(t, func() ports.SessionStore { return sqlite.New(t.TempDir()) })
}
```

The factory must return a **fresh, empty** instance per call. The reference
wiring is `internal/fake/contract_test.go` — copy it.

## TDD loop for a new adapter

1. Add the contract entry point in the adapter's test file (it won't compile or
   will fail — red).
2. Implement the adapter behind its port until the contract is green.
3. Add adapter-*specific* tests only for behavior the contract can't express
   (e.g. SQLite migrations, a provider's SSE parsing). Keep these few.

## Conventions

- **Hand-written fakes, no mocking framework.** `internal/fake/*` are the
  doubles. Go's idiom is small fakes; a mock framework would close doors.
- **Determinism.** Inject `Clock` (fixed clock in tests); never call
  `time.Now()` in `core`/`engine`. Seed any randomness. This is what makes
  golden/replay tests stable.
- **Purity where it counts.** `core.Project` is a pure function with a fuzz
  target (`FuzzProject`); prefer pure functions for anything fold/transform so it
  is fuzzable with no engine/store.
- **Race always.** Anything touching the session actor or shared state runs
  under `-race`; CI runs the whole suite with it.
- **Behavior, not line counts.** Test edge cases and invariants. There is no
  hard coverage gate (it rewards line-count tests); coverage is reported as an
  informational number only.
- **Golden replays.** Engine scenarios assert the ordered `KernelEvent` stream
  and the resulting event log (see `internal/engine/engine_test.go`), since both
  are deterministic given the fakes.

## CI gates

- `go build ./...`, `go vet ./...`
- `go test -race -cover ./...` (coverage printed, not gated)
- a short active fuzz run of `FuzzProject`
- `golangci-lint run`

## What we deliberately do NOT have yet (avoid premature scaffold)

- E2E against real LLM APIs (flaky; arrives with real providers).
- Gateway/platform integration harnesses, benchmarks, UI snapshot infra — no
  such code exists yet. Add the harness when the code it tests exists.
