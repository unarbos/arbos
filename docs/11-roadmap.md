# 11 — Roadmap

> Status: living. Phase boundaries are the contract (see
> [01-architecture](./01-architecture.md)): at each boundary `go build/vet/test
> -race` is green, the codegen tree is clean, and the binary runs. Breakage
> *within* a phase is fine.

## Done — the kernel is a runnable walking skeleton with real adapters

Every port has its reference adapter, each behind the same interface the fakes
satisfy and proven by the shared `porttest` contracts.

| Area | Package | State |
|---|---|---|
| Core data model (event log, sealed Intent/KernelEvent/EventPayload, projection, compression fold) | `internal/core` | done, fuzzed |
| Turn engine (actor-per-session, interrupt = ctx cancel, FIFO queue, budgets) | `internal/engine` | done |
| Durable store (SQLite, FTS5, per-event version + upcast seam) | `internal/sqlite` | done |
| Parallel read-only tool dispatch | `internal/engine` | done |
| Reference LLM provider (OpenAI-compatible HTTP/SSE, tool-call reassembly) | `internal/provider/openai` | done |
| Secret broker (host-bound credential injection) | `internal/secret` | done |
| Tool runtime + build-time schema codegen + built-in coding tools | `internal/tool*`, `cmd/gen-tool-schemas` | done |
| Suspend-and-await (approval) | `internal/engine` | done |
| In-place context compression + token-budget policy | `internal/engine`, `internal/compaction` | done |
| Memory provider (retrieve → log → render fenced → observe) | `internal/memory` | done |
| Observability (Observer port, slog, secret-redacting handler) | `internal/obs` | done |
| Agent delegation (Agent/Task/Grant, local ArbosAgent, delegate tool, depth relay) | `internal/agent` | done |
| Control seam (interaction codec + stdio JSON transport + resume) | `internal/control`, `core/interaction.go` | done |
| Host binary (assembled kernel: one-shot + `-serve`) | `cmd/arbos` | done |

## Next — adapters and capabilities, each behind an existing port

These extend the kernel without changing its boundary. Ordered by leverage.

1. **Native LLM providers** — Anthropic (thinking-mode `ProviderMeta`), Gemini,
   Bedrock — each an `LLMProvider`. Hand-rolled SSE/quirks; no engine change.
   Only advertise `Vision: true` once the multimodal content type lands
   (ADR-0012).
2. **TransportAgent backends** — Codex app-server / Copilot ACP implementing the
   now-declared `ports.AgentTransport`; the engine delegates a turn instead of
   running it. Plus the engine turn-step branch that calls a transport.
3. **MCP** — client (consume external tool servers) and server (expose our
   `ToolRuntime` to external runtimes), via the official Go MCP SDK.
4. **Remote ArbosAgent** — reach a remote kernel over the control seam; the
   delegation transport is the same seam frontends use.
5. **Live nested delegation** — engine-level streaming-tool support so the
   `delegate` tool relays child events upward (RunDelegated already provides the
   depth-tagged relay where an emit sink exists).
6. **Environment isolation** — git-worktree / COW for delegated coding sessions;
   artifact merge-back in `Result`.
7. **Frontends** — Bubble Tea TUI, then the React dashboard backend, then
   desktop — each a thin translator onto the control seam.
8. **Config & self-update** — comment-preserving config (ADR-0006) and
   atomic-binary-swap update with a `--dev` source mode (ADR-0007).

## Invariants every phase upholds

- The kernel takes `Intent` in and emits `KernelEvent` out; it never learns
  about a transport, a provider SDK, or a storage engine.
- The event log is the source of truth; a failed append aborts the turn.
- One goroutine owns each session; interrupts are context cancellation.
- New adapters pass the existing `porttest` contract before they are trusted.
