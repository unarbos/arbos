# qa-017: a running turn recreates its agent folder after the desktop deleted it

status: pr-open — https://github.com/unarbos/arbos/pull/68 (branch `cursor/fix-qa-017-018-52cd` -> `cursor/release-integration-52cd`)
severity: medium (ghost folders the kernel does not list; deleted chats keep running and writing)
scenario: rapid-create-delete (with a model key)
rollout: /cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/qa/rollouts/20260913T064952Z-rapid-create-delete
fingerprints: a2776c674a cf36b9e7ec

## Repro

Create a chat, prompt it, and `rm -rf` its folder while the turn runs (what `desktop/src/kernel.rs` `delete_agent_dir` does).

## Expected

The turn notices the folder is gone, stops without writing, and the folder stays deleted.

## Actual

`append_events` recreates `.arbos/agents/<id>/transcript.jsonl` for every later step (`create_dir_all` on the parent); the plan and attempts files come back the same way. `list_agents` skips the folder (no agent.md), so the ghost is invisible to the kernel and to the window. #24 closed the frame-driven half of this (qa-005); this is the turn-driven half.

## Suspected location

`crates/arbos-engine/src/turn.rs` (no liveness check between steps), `crates/arbos-core/src/files.rs` `append_events` (`create_dir_all`). Fix idea: before each append in a turn, if `agent.md` is gone, stop the turn without writing; `append_events` for an agent transcript should not create the agent folder.

## Back on `main` (2026-09-14 03:41 UTC)

`rapid-create-delete` on `main` @ `888f512e` finds ten ghost folders per run (`chat-rcd-*/transcript.jsonl` + `trace/` written after the delete). The turn checks the folder only at the top of each step; the model's reply lands seconds after the `rm -rf`. Fix PR [#143](https://github.com/unarbos/arbos/pull/143): `append_events` never recreates the folder, the trace writer skips a gone agent, the turn re-checks after the model step. Test `a_late_model_reply_does_not_recreate_a_deleted_chat`.
