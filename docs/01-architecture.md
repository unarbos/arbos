# 01 — Architecture

> Status: foundational. This is the map. Read [02-data-model](./02-data-model.md)
> and [03-ports](./03-ports.md) alongside it.

## One sentence

A small, typed Go **kernel** that takes `Intent` in and emits `KernelEvent` out,
persists each session as an **append-only event log**, and depends only on a
handful of **ports**; everything else — providers, tools, memory, platforms,
frontends — is an **adapter** that attaches at the edge.

## The principle that drives every boundary

Hermes (the Python original) grew capability-first: each new platform, provider,
and tool was added wherever it was easiest, so the same logic ended up
re-implemented across four frontends and core files reached 14–16k lines. The
arbos rewrite exists to make *where a thing belongs* obvious and cheap. The
mechanism is **hexagonal architecture, taken literally**: the kernel knows
nothing about transports, provider SDKs, or storage engines.

```
        Adapters (edge)                  KERNEL                Adapters (edge)
  ┌───────────────────────┐     ┌────────────────────────┐
  │ CLI / TUI / gateway / │     │  engine (turn loop)     │     ┌─ LLMProvider
  │ web / socket          │────▶│   - actor per session   │────▶├─ ToolRuntime
  │  translate transport  │     │   - Intent in           │     ├─ SessionStore
  │  → Intent             │     │   - KernelEvent out      │     ├─ Clock
  │  render KernelEvent   │◀────│   - event-sourced log   │     └─ (AgentTransport, future)
  └───────────────────────┘     └────────────────────────┘
```

## The kernel boundary (what is in, what is out)

**In the kernel:** the turn engine, the `core` data types, the port interfaces,
exactly one reference adapter per port, and (Phase 7) the control seam.

**Out of the kernel — added one-by-one later, each as an adapter:**

| Out | Attaches as |
|---|---|
| Messaging platforms (Telegram, Discord, Signal…) | inbound transport → `Intent` |
| TUI / Web / Desktop | frontend → renders `KernelEvent` over the seam |
| Native providers (Gemini, Anthropic, Bedrock) | `LLMProvider` adapter |
| Agent-runtime transports (Codex, Copilot ACP) | `AgentTransport` adapter (ADR-0002) |
| The tool catalog | `ToolRuntime` (tools register into it) |
| Memory backends, MCP, skills, curator | tools / seam clients, not kernel ports |
| Plugins, execute_code tiers | extension layer above the kernel |

If a thing has `port interface + 1 reference impl`, it is kernel. If it is "the
Nth impl," it is later. This keeps the kernel a runnable walking skeleton.

## Anatomy of a turn

1. A frontend translates input into a `PromptIntent` and sends it to the
   session.
2. The **session actor** (one goroutine, sole owner of the session) picks it up.
3. The engine projects the event log into `[]Message` and builds an
   `LLMRequest`.
4. It calls `LLMProvider.Stream`, forwarding `MessageDelta`/`ReasoningDelta`
   events as chunks arrive.
5. If the model requested tools, it dispatches them via `ToolRuntime`
   (`ToolStarted`/`ToolFinished`), appends results to the log, and loops.
6. Otherwise it emits `TurnComplete` and the actor returns to waiting.
7. An `InterruptIntent` arriving mid-turn cancels the turn's `context.Context`.

Everything the loop writes goes to the event log; the conversation is always a
projection of that log, never a separately mutated structure.

## Process model

- **One process.** The kernel, the engine, and (eventually) frontends run in one
  Go process — no Python subprocess bridge, no JSON-RPC-to-self.
- **One goroutine per session** (actor). See [05-concurrency](./05-concurrency.md).
- **Adapters that are genuinely external** (signal-cli, a sandboxed
  `execute_code` runtime, an MCP server) are subprocesses — by necessity, not by
  architecture.
- **Frontends attach to the control seam** (Phase 7) rather than embedding the
  engine, so "run on a VM, talk from Telegram" is structural, not a special path.

## How we get there (and where temporary breakage is allowed)

This is a planned migration with explicit phase boundaries (see
[11-roadmap](./11-roadmap.md)). Per outcome-oriented execution: we converge on
this target and verify at each boundary (`go build/vet/test -race` green +
skeleton runs), rather than keeping every intermediate state shippable. Breakage
*within* a phase is fine; the phase boundary is the contract.

The two assumptions most expensive to get wrong are settled up front in ADRs:
the provider port shape (ADR-0002) and assistant-message round-trip metadata
(ADR-0003). Everything else is isolated behind a port and reversible.
