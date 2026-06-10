# 08 — Agent delegation ("agents all the way down")

> Status: foundational pattern. See ADR-0013 (refines ADR-0002). This is how
> arbos passes work off to Codex, Claude Code, Cursor, other Arbos agents,
> Hermes — and how a plain LLM query rides the same machinery.

## The one-line pattern

> An **`Agent`** runs a **`Task`** and streams **`KernelEvent`**s. **Delegation**
> is one agent invoking another as a child, parameterized by a **`Grant`**
> (tools + environment + budget). A plain LLM query is the degenerate Agent.

Everything composes because a parent never cares *what kind* of agent the child
is. It hands off a `Task` and consumes events. The kernel boundary (Intent in /
Event out, [01-architecture](./01-architecture.md)) is unchanged — delegation
happens *inside* a turn, as a capability.

## Vocabulary

```go
type Agent interface {
    Run(ctx context.Context, t Task, emit func(core.KernelEvent)) (Result, error)
}

type Task struct {
    Instruction string          // the prompt / coding goal
    Context     []core.Message  // optional handed-down context
    Backend     BackendRef      // openai:gpt-5 | codex | claude-code | cursor | arbos | arbos@host
    Grant       Grant           // what the child may use
}

type Grant struct {
    Tools       []string        // toolset/name allowlist for the child
    Env         EnvironmentRef  // WHERE it runs; COW-isolated by default
    Budget      Budget          // iterations / tokens / wall-time; bounds the subtree
    Permissions ApprovalPolicy  // auto | ask-parent | sandbox-auto
}

type Backend struct {
    Name    string
    Kind    BackendKind         // completion | transport | recursive
    Probe   func() Availability // CLI/SDK present and authed?
    Install Recipe              // how to self-provision when absent
}

type Result struct {
    Text         string
    Artifacts    []Artifact     // diffs, files changed, produced paths
    Usage        core.Usage
    ChildSession string
}
```

`Environment` and `Budget` are now load-bearing (not "future"); they exist as
types even while their implementations start as stubs.

## The Agent taxonomy

| Agent | Loop owner | Talks to | Tools | Env |
|---|---|---|---|---|
| `LLMAgent` | **our** engine loop | `LLMProvider` | our `ToolRuntime` (filtered by Grant) | shared in-proc |
| `TransportAgent` | the runtime | `AgentTransport` (Codex/Claude/Cursor) | its own + ours via MCP | same cwd/container |
| `ArbosAgent` (local) | a nested arbos engine | itself | our `ToolRuntime` (filtered) | COW worktree |
| `ArbosAgent` (remote) | a remote arbos | Phase-7 control seam | remote's, granted | remote's env |
| `HermesAgent` | Hermes | its protocol | exposed via MCP | shared/sandbox |

`LLMProvider` and `AgentTransport` are leaf **ports**; `Agent` is the
composition layer above them.

## How the four product goals map

- **Kick off a coding session** —
  `delegate(Task{Backend: claude-code, Grant:{Tools: coding, Env: ./repo}})`. A
  Task with a working-dir Environment and coding tools, routed to a
  coding-capable Agent.
- **Create a new Arbos agent** — `delegate(Task{Backend: arbos, Grant:{...}})`.
  The child is our own kernel (a nested session actor). Recursion is free.
- **Share tools and envs** — the `Grant` *is* the sharing primitive (below).
- **Self-install backends** — `Backend.Probe()` fails → the agent runs ordinary
  install tool calls to provision it (below).

**Normal LLM query** = `delegate(Task{Backend: openai:gpt-5, Grant:{Tools: none,
Budget: 1 turn}})`, or simply the top-level session being an `LLMAgent`. Same
machinery, degenerate parameters — this is the cohesion test, and it passes.

## Delegation surface

The model sees **one tool, `delegate`**, parameterized rather than fanned out
into per-use-case sugar:

```
delegate(instruction, backend?, tools?, cwd?) ─▶ Task{Instruction, Backend, Grant{Tools, Env}}
```

- `backend` selects the runtime; omitted, it defaults to the primary (`pi`), so
  a bare `delegate` is "kick off a coding session."
- `tools` is the child's allowlist; omitted, the child gets the full toolset.
- `cwd` roots the child's workspace (`Grant.Env.Path`) — any path on the machine,
  not just under the parent's tree; omitted, it inherits the parent's cwd.

All delegations fan out: siblings advertise an empty footprint and run
concurrently whether they read or write (see Concurrency below). This is the
"explore or edit a repo with N sub-agents at once" path.

One tool, one dispatch path, no duplication. A turn that calls `delegate`
instantiates the child Agent, runs it, and relays its `KernelEvent`s upward.
Each relayed event carries an **envelope** (`SessionID` + `Depth`) so frontends
render nested sub-agent activity. (Envelope is the one recorded data-model
addition; see ADR-0013.)

## Sharing tools and environments (the `Grant`)

One concept, two transports:

- **In-process** (Arbos / LLM children): the child runs against our
  `ToolRuntime` filtered to `Grant.Tools`, in the granted `Environment`. No
  serialization needed beyond the store, because each child is its own session
  actor.
- **MCP + shared workspace** (external runtimes): we expose our `ToolRuntime`
  over an **MCP server** the external runtime connects to (the arbos analog of
  Hermes's `hermes_tools_mcp_server`), and run the runtime in the **same
  cwd/container** so it shares the environment. The runtime keeps its own native
  tools *and* gains ours.

### Environment isolation (opt-in, not default)

Delegated children **share the live directory by default**. arbos assumes a human
is watching and git is the net, so parallel coding sub-agents edit the same tree
at once — the way Cursor runs parallel agents — and you reconcile with version
control. There is no filesystem sandbox in the default path (see the file tools'
`tool.Resolve`: it resolves paths, it does not confine them).

The net is not bare git, though. Three mechanisms make shared-tree work survivable:
the read-ledger staleness guard (a write/edit is refused if the file changed
since the agent read it this turn, so one actor cannot silently clobber
another), the `changes` tool (status plus diff since this session's restore
point, so the agent can inspect/re-read/retry when it collides), and automatic
per-turn restore points — before a file mutation the coding toolset snapshots
the git repo that owns that target path to a per-session ref
(`refs/arbos/checkpoints/<session>`, reflog as history), which `undo path=...`
restores. If a target is outside any git repo, the edit still proceeds but the
tool says it is not undo-covered. `undo` is a repo restore, not a surgical
per-agent rollback. Recoverability and rebase/retry, not confinement, are how
arbos stays both free and safe.

**Isolation is something you add when a real case demands it.** If parallel
writers that cannot race are ever needed — untrusted backends, unattended fleets
where a collision can't be eyeballed — that is a `Grant` flag giving each child
its own git worktree, built then, against that spec. Likewise true confinement
of an untrusted child is OS-level isolation (a container, a dedicated user), not
a path check inside the tools. Neither exists today because nothing needs them
yet: freedom is the baseline, safety is the addition.

## Self-provisioning backends (the differentiator)

Backend availability is **probed**, not assumed. When a backend is missing and
consent allows (`allow_provisioning`, the arbos analog of Hermes's
`allow_lazy_installs`), the agent provisions it by **running normal tool calls**:

```
probe claude-code → absent
  → terminal: npm i -g @anthropic-ai/claude-code   (in an Environment)
  → probe again → present
  → delegate as planned
```

Provisioning is *not* special-cased core logic — it is the agent using its own
`terminal`/package tools in an `Environment`, surfaced in the system prompt as
"backend X is installable." This is exactly "the agent actually runs the tool
calls." Offline / unattended sessions degrade gracefully (no provisioning;
fall back to an available backend).

## Remote Arbos = the control seam doubles as delegation transport

A remote `ArbosAgent` is reached over the **Phase-7 control seam** (intents/events
over socket/stdio). Delegating to `arbos@host` sends a `Task` as intents and
consumes the remote's `KernelEvent`s — identical to a local child, just over the
wire. The seam we build for frontends *is* the seam for remote agent pass-off.

## Concurrency & budget

- Each delegated agent is a child **session actor**; events stream up, no shared
  mutable state crosses the boundary except the filesystem, which is shared live
  by default.
- `Grant.Budget` bounds the **whole subtree** (iterations/tokens/time), so a
  runaway delegation can't exceed the parent's allowance.
- A parent interrupt cancels the child via `context.Context` propagation.
- Sibling delegations run concurrently (empty footprint). The filesystem race
  that creates is accepted by default (human-monitored, git-backed) and removed
  only when you opt into COW/worktree isolation.

## Relationship to ports and phases

- Ports unchanged: `LLMProvider` (built), `AgentTransport` (ADR-0002),
  `ToolRuntime`, `SessionStore`, `Clock`, `Environment` (now load-bearing).
- Phase placement: the `Agent`/`delegate` layer lands after the direct-LLM
  provider (Phase 4) and the tool runtime (Phase 5); `TransportAgent`
  (Codex/Claude/Cursor) and remote `ArbosAgent` follow once the control seam
  (Phase 7) exists. Self-provisioning rides on the tool runtime + `Environment`.

## Open items to settle in implementation

- Exact `AgentTransport.RunTurn` signature (interrupt via ctx; how `Result`
  artifacts are reported).
- `Environment` worktree mechanics (git vs overlay) and artifact merge-back UX.
- Approval mapping: external runtimes' permission prompts → our approval
  `KernelEvent`s vs sandbox-auto.
- Per-runtime context/skills/memory injection fidelity (system prompt vs
  AGENTS.md-style file vs MCP resources).
