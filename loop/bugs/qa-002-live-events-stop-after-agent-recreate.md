# qa-002: after an agent folder is deleted and recreated, its live event frames stop reaching clients

status: pr-open — https://github.com/unarbos/arbos/pull/13 (branch `cursor/fix-qa-002-tail-identity-de28`, stacked on #10, retargets to `rust` when #10 merges)
severity: medium (desktop shows a frozen chat after "delete root" until the transcript outgrows its old length)
scenario: rapid-create-delete
rollout: /cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/qa/rollouts/20260912T223531Z-rapid-create-delete
fingerprints: 91a0391c25

## Repro

1. Start the kernel, attach, send a prompt to `root`, wait for `turn idle` (transcript now has N lines; N=2 in the rollout).
2. Do what the desktop does on delete: `rm -rf .arbos/agents/root` (`desktop/src/kernel.rs:1189-1205`), then recreate the folder (`bootstrap`/`create_chat` shape).
3. Send another prompt to `root` and wait for `turn idle`.

## Expected

The attach stream carries `event` frames for the new `wake` and `user` lines, like it did for the first prompt.

## Actual

Zero `event` frames for `root` after the recreate (`frames.jsonl`; `result.json` notes: `live_wake_or_user_frames_after_recreate: 0`, new transcript has 2 lines). The turn runs (`turn running/idle` frames arrive), but the chat content never does. Frames resume only once the new transcript has more lines than the old one had.

## Suspected location

`crates/arbos-kernel/src/serve.rs:148,276-287`: `tails: HashMap<String, usize>` keeps "lines already broadcast" per agent id and only emits when `events.len() > *seen`. Nothing resets the cursor when the folder disappears or shrinks. Same shape for `announced` (line 151).

## Fix idea

Reset the cursor when `events.len() < *seen` (file shrank or was replaced), or key the cursor on the transcript file's inode/creation time, or drop entries for agents no longer listed.
