# ADR-0013 — Agent delegation pattern ("agents all the way down")

- Status: Accepted (pattern); implemented in its phase
- Date: 2026-06-08
- Refines: ADR-0002

## Context

We want a single cohesive mechanism to "pass off" work to: external agent
runtimes (Codex, Claude Code, Cursor), other Arbos agents (local or remote),
Hermes agents, **and** plain LLM queries. ADR-0002 modeled the backend choice as
two sibling ports (`LLMProvider` vs `AgentTransport`) that the engine branches
on. That is correct for "swap the backend" but too shallow for first-class
delegation, tool/environment sharing, and self-provisioning.

## Decision

Introduce an **`Agent`** composition layer above the ports:

> An `Agent` runs a `Task` and streams `KernelEvent`s. **Delegation** is one
> agent invoking another as a child. A plain LLM query is the degenerate Agent.

- `LLMProvider` and `AgentTransport` remain **low-level ports** (talk to a model
  / drive an external runtime). They are no longer the unit of composition.
- `Agent` is the unit of composition. Implementations: `LLMAgent` (our loop +
  `LLMProvider` + granted tools + environment), `TransportAgent` (Codex / Claude
  Code / Cursor via `AgentTransport`), `ArbosAgent` (nested kernel — local
  in-process child session, or remote over the Phase-7 control seam), and
  `HermesAgent` (a `TransportAgent` flavor).
- **Delegation is a capability**, exposed to the model as **one core `delegate`
  tool** (with a `backend` parameter) plus thin sugar tools
  (`start_coding_session`, `spawn_agent`) that desugar to it. One code path.
  > **Superseded 2026-06-09:** the sugar tools were dropped. `delegate` is the
  > single delegation tool; its parameters carry what the sugar used to —
  > `backend` defaults to the primary (`pi`), and `cwd` folds in the working
  > directory that `start_coding_session(repo, …)` set. Every extra near-duplicate
  > tool only diluted the model's tool selection, and the desugaring already
  > shared one dispatch path, so the sugar earned nothing.
- The unit of "what a child may use" is a **`Grant`** (tools + environment +
  budget + permissions). Sharing has one concept, two transports:
  **in-process** filtered `ToolRuntime` + shared `Environment` for
  Arbos/LLM children; **MCP server + same cwd/container** for external runtimes.
- **Environments are copy-on-write / git-worktree isolated by default** for
  delegated agents; sharing the live directory is explicit opt-in. This removes
  the shared-mutable-filesystem race for parallel coding sessions.
- **Self-provisioning is ordinary tool use, not core logic.** A `Backend` has a
  `Probe()` (is the CLI/SDK present and authed?) and an `Install` recipe; when a
  backend is missing and consent allows, the agent runs normal `terminal`/install
  tool calls in an `Environment` to provision it, then delegates.

### Data-model addition (recorded; implemented with the pattern)

`KernelEvent` gains an envelope carrying the originating `SessionID` and a
`Depth`, so frontends can render nested sub-agent activity. Decided now because
retrofitting after frontends exist is painful.

## Consequences

- Codex / Claude Code / Cursor / Arbos / Hermes / plain LLM all pass off through
  one mechanism; frontends and the event log are unaffected by the backend.
- `Environment` and `Budget` become load-bearing types (were "future"); they are
  defined now even if implementations start as stubs.
- Budget bounds the whole delegation subtree; interrupts propagate down via
  `context.Context`.
- Slight indirection: the engine's turn step runs an `Agent`, not a hard-coded
  LLM call. Accepted — this is the cohesion we want.

## Alternatives rejected

- *Distinct tool per backend, no shared core* (rejected): duplicates dispatch,
  drifts, and makes "normal LLM query" a special case instead of a degenerate
  one.
- *Share the live environment by default* (rejected as default): races on the
  filesystem across parallel children; available only as explicit opt-in.
