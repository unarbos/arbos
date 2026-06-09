# Phase 11 — Skills loader (project + user)

Back to [overview](./overview.md). Decision D6 (project and user scope).

## Goal

Port pi's skills mechanism so the agent discovers SKILL.md files and loads them
on demand, across both project and user scopes.

## Changes

- Discover SKILL.md under a project skills directory (a `.arbos/skills`-style
  path under cwd) and a user skills directory, gitignore-aware, with pi's
  discovery rule (a directory with SKILL.md is a skill root; otherwise recurse).
- Parse frontmatter (`name`, `description`, `disable-model-invocation`), validate
  per agentskills.io, handle name collisions across scopes.
- Produce the `<available_skills>` XML index for the prompt (Phase 10), excluding
  `disable-model-invocation` skills from the model-visible index.

## Data structures

- `Skill{ Name, Description, FilePath, BaseDir string; DisableModelInvocation bool }`.

## Dependencies

Phase 5 (`read` loads bodies). Feeds Phase 10.

## Verification

Static. Build, vet, test -race, generate-check, lint.

Runtime. Harness seeds a project skill and a user skill, asserts both appear in
the index, a `disable-model-invocation` skill is excluded, and a project skill
wins a name collision.
