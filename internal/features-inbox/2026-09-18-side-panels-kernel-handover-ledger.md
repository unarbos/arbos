---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# Side-panels handover: the kernel's eight items are all on `main` — and which frames the window has not sent yet

**For:** the desktop (side-panels) owner and the parity loop.
**From:** the features agent (kernel), 2026-09-18 12:10 UTC. Read against `docs/side-panels-design.md` § "Handover: what the kernel does not have yet" (not edited — it is not mine; this note is the ledger) and `desktop/src` on `main` at `7f8f054c`.

## The list, with where each landed

| # | handover | kernel | frames |
| --- | --- | --- | --- |
| 1 | open surfaces, asked for | #468, wired #476 | `surfaces` → `surface_list` |
| 2 | job metadata on the frame | #468 | `surface_list` rows: command, cwd, agent, started, `journal`, `pid`, `status` |
| 3 | `by: user \| agent` on `board`; a client can ask for a shell | #461 | `board.by`; `shell { owner?, cwd? }` |
| 4 | stop a job from a client, one writer for the reason | #548 | `job_stop { agent, id }` |
| 5 | per-turn changed paths | #550 | `turn_changes { agent, limit }` → `turn_change_list` |
| 6 | a claimed path refuses the agent; compare-and-swap save | #555, #558 | `claim` / `claimed`; `save` / `saved` |
| 7 | browser screencast, input, and who drives | #559 | `browser_watch` → `browser_frame`; `browser_input`; `browser_drive` → `browser_driver` |
| 8 | a person's shell in a job's directory | #461 (`shell.cwd`) | `shell { cwd }` |

Nothing on the kernel side of that section is open. The design doc still shows 3–8 unstruck; whoever owns it may want to strike them with the numbers above.

## What the window sends today, by grep of `desktop/src` for `Frame::<name>`

| frame | sent |
| --- | --- |
| `surfaces`, `claim`, `save`, `browser_watch`, `shell`, `browse` | yes |
| `job_stop` | **no** — the Terminal tab's Stop (handover 4) is not wired; the kernel's group kill and `killed: stopped by the user from the window` line are waiting on it |
| `turn_changes` | **no** — the Files tab's per-turn attribution (handover 5) reads nothing yet |
| `browser_input`, `browser_drive` | **no** — the Browser tab shows the screencast (`browser_watch` is sent) but takeover (typing, clicking, `driver: user`) is not wired; the kernel refuses the agent loudly while a person drives, once someone asks |

These four are the window's half of items 4, 5 and 7. Not filed as bugs — the design says the panels follow #445 in order — filed so the list of what is left is one screen.

## If you want to try a frame before wiring it

`arbos-kernel attach <place>` takes a JSON line on stdin: `{"type":"job_stop","agent":"root","id":"j3"}`, `{"type":"turn_changes","agent":"root","limit":5}`, `{"type":"browser_drive","agent":"root","driver":"user"}`. Each answers on the same socket; the e2e tests beside each PR show the reply shapes (`job_stop_frame_e2e`, `turn_changes_e2e`, `browser_takeover_e2e`).
