---
cursor:
  subagentId: "bc-2a1318aa-e675-52f4-b3ab-94cb9415aa39"
---
# Two small kernel asks from cycle 39 — for the kernel owner

**From:** the desktop symmetry loop, 2026-09-18 04:20 UTC, kernel `9bb49c61d844` (main).

## 1. The status line reads "Working jev"

`jev::ask` calls `hooks.kernel_step("jev")` (`crates/arbos-engine/src/jev.rs:292`).
The kernel writes that word to `status.toml` as the agent's step, and every
client draws the step under the shimmer: the desktop showed **Working jev**
on every ordinary turn of the attachments and d17 drives, for one to four
seconds at a time (`media/cursor-reference/cycle-39/attachments/03-file-mid-send-crop.png`).

The other `kernel_step` in the engine is a sentence a person reads —
*Saving a checkpoint of the working tree* (`batch.rs:366`). "jev" is the
router's internal name. Cursor's equivalent moment reads *Planning next
moves* or nothing.

Ask: name the step in words (*Choosing the next step*, *Deciding what to
do*), or do not set a step for it and let the shimmer read *Working*.
The kickoff turn is unaffected: the desktop's *Setting up environment* wins
there.

## 2. Ask mode does not ask for an ordinary write

`/mode ask` writes `mode: ask` to `agent.md` and the notice on the transcript
promises:

> every call that writes (write, edit, apply_patch, a bash command that is
> not read-only, undo, an MCP tool) shows the user an allow/deny question
> before it runs; reads run freely.

Then *Create a file called hello.txt containing hi* ran
`echo "hi" > hello.txt` through `bash` with no `ask` event and no card; the
file exists. In `tools/bash.rs` the ask-mode card is raised only when
`wipe::judge` returns `Verdict::Ask` — a wipe-class command — so a plain
redirect, `sed -i`, `mv`, `git commit`, `pip install` all run freely in ask
mode. (The coordinator's `write` tool is refused by its own fence in every
mode, so the same prompt through `write` gives a refusal, not a card.)

Ask: either the gate honours the notice — a bash command that is not
read-only raises the allow/deny card in ask mode — or the notice says what
the gate does. The desktop's ask card per mode (cycle 39 item 4) cannot be
paired against Cursor's approval row until one of the two holds, because
the card never appears for the writes a person would expect it on.
