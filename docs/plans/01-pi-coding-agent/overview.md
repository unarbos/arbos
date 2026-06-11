# Plan 01 — pi coding agent, faithful full port into arbos

> Status: proposed (revised for full-fidelity bar). The goal is a world-class
> in-house coding agent in arbos that brings across pi's intelligence faithfully,
> integrated into the Go turn loop. This plan does not authorize implementation.

## Context

pi (`/home/const/arboshermes/pi`, pinned `2edd6b432a4e1eed0a70270540d9a78d12aea7e9`)
is a TypeScript coding agent harness. We are rewriting its coding agent in Go and
shipping it inside arbos. The bar is a faithful full port of pi's intelligence,
not a lean headless subset.

This revision follows a deep re-read of pi against
`principle-foundational-thinking`. The central finding changed the plan: pi's
coding intelligence lives as much in **data shapes and result text** as in the
loop, and arbos's current kernel shapes are too thin to carry it without loss.
Getting those shapes right is foundational and must come first.

## Reference

Ported from pi `2edd6b43` (2026-06-08). Fork-and-own. Record the SHA on each
behavior re-sync.

## What makes pi smart (the intelligence inventory)

The sources of pi's coding quality, concrete and enumerated. These are the
acceptance bar; the plan exists to bring every one across.

1. **System prompt construction.** Persona, an "Available tools" list built from
   per-tool one-line snippets, de-duplicated per-tool guideline bullets,
   `<project_context>` blocks from AGENTS.md / CLAUDE.md-style files, an
   `<available_skills>` XML index, and date plus cwd appended last.
2. **Model-facing tool result text.** The strings tools return shape behavior
   heavily. read emits actionable continuation notices (`Use offset=N to
   continue`, a `sed` fallback for huge lines). grep emits `path:line:` rows,
   context blocks, and match/byte-limit notices with `Use limit=2x`. bash emits
   truncation notices with a full-output file pointer and exit-code/timeout
   text. edit returns a line-numbered diff and a unified patch.
3. **The edit contract.** Exact match first, then a **fuzzy** fallback (NFKC,
   smart-quotes and Unicode-dash and special-space normalization, trailing-
   whitespace strip). Uniqueness enforced. Overlap rejected. Reverse-order
   application. Tuned error messages for not-found, non-unique, empty, no-change,
   and overlap. This is the single most behavior-shaping tool.
4. **Context compaction intelligence.** A structured checkpoint summary prompt
   (Goal / Constraints / Progress[Done, In Progress, Blocked] / Key Decisions /
   Next Steps / Critical Context), an UPDATE variant that preserves and merges
   across repeated compactions, a turn-prefix split summary, read/modified file-
   operation tracking appended to the summary, a cut-point algorithm that never
   splits a tool_call from its result, and budgets driven by the model's context
   window.
5. **Skills (agentskills.io).** SKILL.md discovery across project and user
   scopes, gitignore-aware, frontmatter (`name`, `description`,
   `disable-model-invocation`), an XML index injected into the prompt, and the
   instruction text telling the model how and when to load a skill body.
6. **Slash commands / prompt templates.** Project and user `.md` prompts with
   `$1` / `$@` / `$ARGUMENTS` and bash-style slicing, expanded before the prompt
   is sent. This is load-bearing usage (the `.pi/prompts` workflows).
7. **Model intelligence in pi-ai.** Reasoning/thinking levels (off..xhigh) with
   per-model thinking-level maps and token budgets, prompt caching (retention,
   session id), and per-model metadata (context window, max tokens, cost, vision)
   that drives both request building and the compaction trigger.
8. **Vision.** read returns images as content blocks, with a note when the model
   lacks vision.
9. **Loop semantics.** Parallel preflight then concurrent execute,
   before/after tool hooks, a `terminate` hint that can end the loop after a tool
   batch, and steering and follow-up queues.

Audited as NOT core intelligence (safe to defer): the `todo` and `subagent`
tools are example extensions, not built-ins. The TUI, HTML export, and the
extension plugin loader carry no model intelligence. pi has no built-in planning
tool and no built-in sub-agent delegation.

## The foundational finding (why the bar reopens a constraint)

The prior plan assumed zero changes to `core`, `ports`, or `engine`. A faithful
port breaks that assumption in exactly three places, and per
`principle-foundational-thinking` these are data-shape decisions that are a
one-time change now and a rewrite later. They are scaffold: every later phase
depends on them, so they come first.

- **`core.ToolResult` is string-only** (`Content string`). pi tool results are
  `content: (text | image)[]` plus a structured `details` payload (the diff,
  patch, truncation record). Images and rich rendering have nowhere to go.
  Faithful read (images) and edit (diff details) require structured result
  content. Trace: every tool phase writes results; the engine persists and
  projects them; a late change rewrites all of it.
- **`core.LLMRequest` has no reasoning level.** pi maps a thinking level onto
  each request per model. Without a field, reasoning control is lost.
- **`core.Message.Content` is text-only; `Capabilities.Vision` is deferred**
  (ADR-0012). Vision needs multimodal message content.

Position: take all three as small, additive, ADR-backed kernel extensions, done
first. They are the difference between a faithful port and a lossy one.

### Confirmed by the access-pattern trace (foundational design pass)

The deep trace of every read/write site confirms all three are **purely additive**
and need no event-version bump or upcast rule, if we keep the existing string
`Content` as the canonical text channel and add fields beside it:

- `core.ToolResult` keeps `Content string` (text the model sees), and gains
  `Blocks []ContentBlock` (images) plus `Details json.RawMessage` (diff, patch,
  truncation record, spill path). Old persisted rows decode unchanged.
- `core.Message` keeps `Content string` and gains `Parts []ContentBlock`,
  reusing the same `ContentBlock` type born in Phase 1. This mirrors pi's own
  `content: string | (Text|Image)[]` union, so the additive shape is the faithful
  shape, not a compromise.
- `core.LLMRequest` gains `Reasoning` (ephemeral, never persisted, so trivially
  additive). `ports.Capabilities` already carries `Reasoning` and `Vision`.

Blast radius that is NOT free (localized, mechanical, no schema migration): the
OpenAI adapter must branch to emit a content array when `Parts`/`Blocks` are set
and map `Reasoning` to `reasoning_effort`; `tool.NewSpec` gains a sibling
constructor for tools that return blocks plus details (existing string-returning
handlers wrap unchanged); the token estimator adds a per-image constant. Phase 1
births `ContentBlock`, so Phase 2 depends on Phase 1. The sequence holds. Full
type sketches and the site-by-site trace live in the Phase 1 to 3 docs.

## Decisions

### Locked

- **D1. Reuse arbos's provider port and broker.** Do not port pi-ai's breadth,
  OAuth, or generated model registry. Add native adapters incrementally.
- **D2. Preserve pi's full-privileges behavior.** pi has no permission system.
  The pi assembly wires `ApprovalPolicy: nil` (engine treats nil as allow-all),
  matching pi. Gating stays an opt-in a user can wire via `engine.WithApproval`.

### Revised by this audit

- **D3. Faithful fidelity requires three additive kernel extensions** (ToolResult
  content blocks + details, LLMRequest reasoning level, multimodal Message
  content). Supersedes the prior "no core changes" stance. Each gets an ADR.
- **D4. Reuse provider port, but port a model-metadata registry.** D1 stands, but
  not porting pi-ai costs reasoning-level mapping, prompt-cache hints, and per-
  model context-window metadata. Recover these with a small Go model registry
  in the pi layer, seeded for the models actually used, not pi's full generated
  table.
- **D5. Compaction is core, not optional.** pi's structured summarizer, file-op
  tracking, and tool-pair-safe cut point are real intelligence. Promoted to a
  full phase with a ported `Summarizer` and a pi-faithful `ContextPolicy`.
- **D6. Skills support project and user scope in v1** (revises the earlier
  project-only position). Full fidelity needs both.
- **D7. Slash commands / prompt templates are in scope** (were dropped). They are
  the "usage" the goal calls for.
- **D8. Vision is in scope**, enabled by D3's multimodal content.
- **D9. Prompt caching is in scope, modeled faithfully.** pi carries a
  `cacheRetention` hint and a session id that drive provider prompt caching,
  cutting cost and latency on long coding sessions. arbos already has a stable
  per-session id (`core.SessionID`), so the session-affinity half is nearly free.
  Add a cache-retention hint and a session id to the request path, a caching
  capability to the model registry, and the mapping in the OpenAI-compatible
  adapter.   Folded into Phase 3 (request and registry shape) and Phase 15
  (assembly sets the retention and passes the session id).
- **D10. The toolset scaffold (Phase 4) proves the codegen path with `ls`, not a
  throwaway `grep`.** grep needs ripgrep and is only specified in Phase 6, so the
  scaffold uses `ls` (a real, dependency-free read-only tool). `ls` ships in
  Phase 4; Phase 5 covers `read` and `find`. A shared `tool.Resolve` sandbox
  guard was added so every file tool shares one security-critical path resolver.
- **D11. No external binaries are required, bundled, or auto-downloaded.**
  Originally `grep` and `find` hard-required `rg` and `fd` on PATH: pi shells
  these too but auto-downloads them, and an agent that fetches binaries
  contradicts arbos's broker-and-allowlist posture. *Amended:* hard failures
  broke basic navigation on clean machines, and bundling was rejected — arbos
  distributes via `go install`, so embedded rg/fd would mean committing
  per-platform third-party binaries to the repo plus runtime extraction, a
  supply-chain and codesigning burden for two tools whose access patterns
  (gitignore-pruned trees, early stop at the match/result limit) don't need
  ripgrep-class throughput. Instead both tools are native: a shared
  gitignore-aware walker (codingspec walk.go, ignore.go, glob.go) backs `find`
  entirely and `grep`'s fallback regexp scan, while `grep` still prefers `rg`
  when present for its throughput on huge trees. Go's regexp and rust's regex
  are both RE2-family, so the pattern dialect is consistent across paths.
- **D12. ST1005 (capitalized error strings) is excluded for
  `internal/tool/codingspec`.** The coding tools return capitalized, model-facing
  message strings as Go errors (e.g. "Path not found", "Offset N is beyond end of
  file"), reproduced verbatim from pi because the exact text is part of the
  agent's behavior. ST1005 is a false positive for these user/model-facing
  messages, so it is excluded for that package only in `.golangci.yml`.
- **D13. Added `golang.org/x/text` for NFKC.** edit's fuzzy matcher normalizes
  with NFKC first (pi's `normalizeForFuzzyMatch` step 1), which needs Unicode
  compatibility tables. `golang.org/x/text/unicode/norm` is a first-party Go
  module, low supply-chain risk, and the faithful choice over skipping NFKC.
- **D14. Extended `jsonschema` to reflect nested structs and struct slices.**
  edit's `edits []Edit` (array of `{oldText,newText}`) could not be reflected by
  the original generator (which supported only `[]string`). The reflector now
  recurses through struct fields and slice elements. Additive: existing builtin
  and coding schemas are unchanged, confirmed by generate-check.
- **D15. edit's model-invisible `Details` carries `replacedBlocks` and
  `firstChangedLine`, not pi's full display diff / unified patch.** The model
  only ever sees the success message; the diff is for a renderer, and arbos has
  no TUI yet (deferred). Building pi's exact diff-format for a non-existent
  consumer would be premature, so `Details` stays minimal and useful until the
  TUI phase consumes it.

### Design positions retained from the prior pass

edit mirrors pi byte-for-byte and now explicitly includes the fuzzy fallback.
Hard-require ripgrep, clear error, no auto-downloader. bash spills to arbos temp.
Port prompt mechanics, rewrite the pi-specific persona. Use pi's short tool names
(`read`, `write`, `edit`, `bash`, `grep`, `find`, `ls`); arbos builtins stay a
separate registry pi never loads. Hardcode local tool execution, no per-tool
Operations seam (`Grant`/`Environment` own the remote story). Defer pi's session
tree and branch-summary navigation. Fork-and-own, pinned to the SHA above.

## Target package layout

- `internal/core` — additive shape extensions only (D3), each behind an ADR.
- `internal/tool/codingspec` — coding tool arg structs, handlers, metadata,
  standalone for the generator. Mirrors `builtinspec`.
- `internal/tool/coding` — the assembler pairing specs with generated schemas.
- `internal/agent/pi` — model registry, system-prompt assembler, skills loader,
  prompt-template loader, the coding `ContextPolicy` and `Summarizer`, the engine
  factory, and backend registration (reachable via the `delegate` tool).
- `cmd/arbos` — the pi entrypoint flag.
- Reused: `engine`, `ports`, `provider/*`, `secret`, `sqlite`, `control`, `obs`,
  `tool` (registry, jsonschema, codegen), `compaction` (extended).

## Applicable skills (for the implementer)

the **how** skill over `engine`, `core`, `tool`, and `provider` before changing
each. the **architect** skill on the D3 shape extensions and on `edit` and
`bash`. the **interrogate** skill on the D3 shapes and the `edit` fuzzy contract
before shipping. the `control-cli` skill for runtime verification each phase.
`/deslop` over each diff. the **unslop** skill on prose. the **babysit** skill
after a PR opens.

## Phases

Foundational kernel shapes first (scaffold), then tools, then the intelligence
layer, then assembly.

1. [ToolResult content blocks and details](./phase-01-toolresult-shape.md)
2. [Multimodal message content and vision](./phase-02-multimodal-content.md)
3. [Reasoning level and model metadata](./phase-03-reasoning-and-metadata.md)
4. [Coding toolset scaffold and codegen](./phase-04-toolset-scaffold.md)
5. [Read-only navigation tools](./phase-05-read-only-nav.md)
6. [grep parity](./phase-06-grep.md)
7. [write tool](./phase-07-write.md)
8. [edit tool (exact + fuzzy)](./phase-08-edit.md)
9. [bash tool](./phase-09-bash.md)
10. [System prompt assembler](./phase-10-system-prompt.md)
11. [Skills loader (project + user)](./phase-11-skills.md)
12. [Slash commands and prompt templates](./phase-12-slash-commands.md)
13. [Compaction intelligence](./phase-13-compaction.md)
14. [Loop fidelity (terminate, steering)](./phase-14-loop-fidelity.md)
15. [pi assembly and backend registration](./phase-15-pi-assembly.md)
16. [Entrypoint and sugar tool](./phase-16-entrypoint.md)

### Phases 17+ (the deferred bucket, brought into scope)

17. [Opt-in coding approval policy](./phase-17-approval.md)
18. [Grant.Tools toolset filtering](./phase-18-toolset-filter.md)
19. [RPC command surface (non-tree)](./phase-19-rpc-surface.md)
20. [Session tree (fork, clone, branch summaries, navigate)](./phase-20-session-tree.md)
21. [RPC tree commands (fork, switch_session)](./phase-21-rpc-tree.md)
22. [Native providers (Anthropic, Google) + OAuth](./phase-22-native-providers.md)
23. [MCP client and server](./phase-23-mcp.md)
24. [Extension mechanism (Go registration seam + event bus)](./phase-24-25-extensions-tui.md)
25. [TUI + edit display diff](./phase-24-25-extensions-tui.md)

## Status: complete (all 25 phases)

All 25 phases are implemented and verified — the full pi port, including the
previously deferred bucket. pi runs as a first-class coding agent in arbos across
every surface: the interactive Bubble Tea TUI (`arbos-tui`), headless one-shot
(`arbos -agent pi`), the control-seam RPC surface (prompt/events/set_model/
compact/abort/fork/switch_session), as a delegatable `pi` backend (via the
`delegate` tool), over MCP (client + server), and via the Go extension seam.

ADRs 0021-0031 record every kernel/seam change; decisions D1-D15 are recorded
above. The full static gate is green; no new lints (the 5 pre-existing issues in
older files are untouched). End-to-end runtime proof exists for each surface,
including a real edit rendered as a line-numbered diff in the TUI while the file
changed on disk.

## Verification

Static gate every phase. `go build ./...`, `go vet ./...`,
`go test ./... -race`, `make generate-check`, `golangci-lint run`.

Runtime gate every phase. The control-cli harness in [testing.md](./testing.md)
drives the real dispatch path and asserts outcomes. Granted unit-test exception
for the pure `edit` matcher (now including the fuzzy normalization cases) and the
`bash` truncation helper.

## Deferred bucket — now implemented (phases 17-25)

The bucket that was first deferred has been brought into scope and completed:
approval gating (17), toolset filtering (18), the RPC command surface (19, 21),
session tree fork/clone/branch summaries (20), native Anthropic/Google providers
(22), MCP client+server (23), the extension seam (24), and the TUI + edit
display-diff (25). Prompt caching was never deferred (D9; phases 3/15).

Genuinely NOT ported, by deliberate decision (subtract-before-you-add): pi's
runtime TypeScript plugin loader (incompatible with a static Go binary — replaced
by the build-time extension seam, ADR-0030); HTML transcript export (cosmetic);
and live provider OAuth/device flows — the Anthropic/Google adapters are complete
and proven against faithful fakes but were not exercised against live keys (none
available in this environment). pi-tui's bespoke differential renderer is replaced
by Bubble Tea's equivalent rather than reproduced byte-for-byte.
