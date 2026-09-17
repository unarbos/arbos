# qa-006: a prompt to a folder the kernel does not list as an agent is accepted, stored as a node, and never fires; damaged folders are accepted silently

status: pr-open — https://github.com/unarbos/arbos/pull/24 (branch `cursor/fix-qa-005-006-agent-exists-de28` -> `rust`)
severity: medium (a user message can vanish with no feedback)
scenario: malformed-folder
rollout: /cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/qa/rollouts/20260912T223640Z-malformed-folder
fingerprints: 32afa5f6f4 0027be132b 864a41ee6d

## Repro

1. Create `.arbos/agents/garbage/agent.md` with non-UTF-8 bytes. Start the kernel.
2. Attach: the `snapshot.tree` lists `orphan` and `root` but not `garbage` (`result.json` notes `agents_listed`).
3. Send `{"type":"user","agent":"garbage","text":"hello garbage"}`.

## Expected

Either the folder is an agent (listed, prompt runs) or it is not (prompt rejected with an error the client sees). One rule, applied by both paths.

## Actual

- The prompt is accepted: `state-after/agents/garbage/plan.jsonl` holds node #1 `pending`, origin `user`, `when.wake: true`.
- No `turn` frame, no error, no stderr line. The node never fires because `plan::scan` iterates `list_agents`, which skipped the folder.
- Related leniency in the same rollout: `orphan/agent.md` with `parent: ghost-parent` is accepted and listed; corrupt transcript lines (2 of 6) are skipped without any notice; a folder with a transcript but no `agent.md` is ignored.

## Suspected location

- `crates/arbos-kernel/src/hooks.rs:424-426` `inbox`: only `agent_dir(agent).exists()`.
- `crates/arbos-core/src/files.rs:196-213` `list_agents`: requires `Agent::load` to succeed (`agent.rs:179-187` `read_to_string` fails on invalid UTF-8).
- `crates/arbos-kernel/src/plan.rs:115` `scan` walks `list_agents` only.

## Fix idea

One `agent_exists(place, id)` = `Agent::load(...).is_ok()` used by `inbox`, `handle_frame`, and `scan`. On boot, log every folder skipped and why (needs the kernel log from design gap 1).
