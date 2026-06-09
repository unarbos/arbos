# Phase 10 — System prompt assembler

Back to [overview](./overview.md). Mirror `coding-agent/src/core/system-prompt.ts`
mechanics; rewrite only the persona.

## Goal

Reproduce pi's prompt assembly faithfully, with an arbos persona instead of pi's.

## pi behavioral spec — assembly order

When no custom prompt is supplied, build in this exact order:

1. Persona line, then a blank line. PORT the mechanics, REWRITE the text. pi's is
   "You are an expert coding assistant operating inside pi…"; arbos uses its own
   coding persona. DROP pi's "Pi documentation (read only when the user asks
   about pi…)" block entirely; it hardcodes pi-repo paths and is wrong here.
2. `Available tools:` followed by one `- {name}: {snippet}` line per active tool
   that supplied a `promptSnippet`. Tools without a snippet are omitted from this
   section. Falls back to `(none)`.
3. The line `In addition to the tools above, you may have access to other custom
   tools depending on the project.`
4. `Guidelines:` followed by `- {bullet}` lines. Bullets are collected per active
   tool (each tool's `promptGuidelines`), de-duplicated preserving first-seen
   order, then two always-on bullets appended: `Be concise in your responses` and
   `Show file paths clearly when working with files`. A bash-without-grep/find/ls
   case adds `Use bash for file operations like ls, rg, find`.
5. `appendSystemPrompt` text if any.
6. `<project_context>` block when context files exist: a header line, then one
   `<project_instructions path="{path}">\n{content}\n</project_instructions>`
   per file, wrapped in `<project_context>` … `</project_context>`. Default
   context files are AGENTS.md and CLAUDE.md loaded from the workspace.
7. `<available_skills>` block (Phase 11) when skills exist and read is active,
   including pi's verbatim usage instructions (how and when to load a skill, and
   the relative-path resolution rule against the skill directory).
8. Two trailing lines: `Current date: {YYYY-MM-DD}` and
   `Current working directory: {cwd}` (forward-slash normalized).

## Data structures

```go
PromptOptions{ Cwd string; Tools []ToolInfo; ContextFiles []ContextFile; Skills []Skill; Append string }
ToolInfo{ Name, Snippet string; Guidelines []string }
ContextFile{ Path, Content string }
```

## Dependencies

Phases 5 to 9 (tool snippets and guidelines). Phase 11 feeds skills.

## Verification

Static. Build, vet, test -race, generate-check, lint.

Runtime. Harness builds a prompt for a fixture workspace containing an AGENTS.md
and asserts: the tools list lines, de-duplicated guidelines including the two
always-on bullets, the `<project_context>` block with the file, the skills block
with usage instructions, the trailing date and cwd lines, and that no pi-repo
persona or pi-docs text appears.
