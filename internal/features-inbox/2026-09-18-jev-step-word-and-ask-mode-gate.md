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

## 3. The plan reader and a listing pasted back (cycle 44, F-194)

Asked to *restructure the project page*, the model wrote `notes.md` by hand and pasted the plan tool's own listing format — `[ ] 1 [Kickoff](docs/…) — ready…`, a box and an ordinal, **no dash**. The desktop now reads such a line as an item ([#661](https://github.com/unarbos/arbos/pull/661)); the kernel's plan reader most likely does not, since the items vanished from its list at the same time. Two small asks, either one enough: the plan tool's listing prints items in the form it reads back (`- [ ] …`), or the reader accepts a boxed line without a dash. Not filed as a bug; it is the model copying the kernel's own output.

## 4. "nothing to compact yet" written unasked (cycle 46, F-199)

On main `00cc5ba8` the root's transcript carries `notice: nothing to compact yet: the whole working set is recent` five times in one journey run — after the kickoff's first tool call, after a `plan`, after a `spawn`, after a `fold`. `compact::next_move` writes it only on the *manual* path (`control.take_compact()` or a compact wake), so something is setting the manual flag per step without a `/compact` from anyone. The desktop now shows the line only within a minute of a `/compact` it sent ([#686](https://github.com/unarbos/arbos/pull/686)); the kernel is still writing it to the record. Ask: find what sets `manual`, or drop the notice when nobody asked.

## 5. The refused-key notice names Jev and a raw 401 (cycle 46, F-200)

With a bad key the kickoff's transcript notice now reads `Jev did not choose the next step: 401 bad API key: User not found.. The turn stopped. The chat model did not run.` The earlier kernel said `google/gemini-2.5-flash did not accept the API key. Check the API key (api_key or api_key_env in ~/.config/arbos/config.toml)` — the model, the fact, and where the key lives. The desktop's short line covers the pane (*No working model key on this kernel.*), but the record and every client without that mapping carry the router's name, an HTTP code and a double stop. Ask: the provider's refusal, in the earlier words, whichever step met it first.
