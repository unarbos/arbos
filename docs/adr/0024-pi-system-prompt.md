# ADR-0024 — pi system prompt: port mechanics, rewrite persona

- Status: Accepted
- Date: 2026-06-08

## Context

pi's coding quality depends in part on its system prompt: a snippet-driven tools
list, per-tool guideline bullets, a project-context block (AGENTS.md / CLAUDE.md),
a skills index, and a trailing date/cwd. The arbos port must reproduce the
assembly faithfully, but pi's persona block hardcodes "you are inside pi" and
pi-repo documentation paths that are wrong inside arbos.

## Decision

Port the assembly mechanics verbatim and rewrite only the persona.
`internal/agent/pi.BuildSystemPrompt` reproduces pi's system-prompt.ts order:
persona, `Available tools:` (only tools with a snippet), the custom-tools note,
`Guidelines:` (per-tool bullets de-duplicated in first-seen order plus the two
always-on bullets and the bash-without-search special case), an optional appended
block, the `<project_context>` block, the `<available_skills>` index (only when
the read tool is active), and the trailing `Current date` / `Current working
directory` lines. Per-tool snippets and guidelines live with the tools in
`codingspec.PromptInfos()`, reproduced verbatim from pi.

The persona is rewritten for arbos and pi's "Pi documentation … read pi docs at
<path>" block is dropped entirely.

## Consequences

- The prompt's structure and the model-facing guideline text match pi; only the
  persona identity differs.
- Skills and project-context files are injected by the assembly phase, which
  passes loaded `Skill`s and `ContextFile`s (AGENTS.md, CLAUDE.md) into
  `PromptOptions`.
- Pure function, deterministic with an injected clock; no kernel change.
