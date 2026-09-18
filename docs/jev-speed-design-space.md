# Jev is the controller

Jacob said unify the whole system. This page is that system.

Jev is not a packer, not an index, and not a model picker. **Jev is the controller.** One JSON object on the kernel turn decides:

- which obvious tool to run, or whether the language model must write
- which chat model the language model step invokes
- which existing glances stay in the standing brief, and which become pointers

Architectures 1 (pack), 2 (overflow pointers), and 3 (standing brief) are layers of this one loop. They are not products.

The mouth stays the mouth. Jev does not speak. Live speaks. The gateway does not call Jev. `session.start` does not call Jev. The brief is a **read**. Model choice is Jev naming which LLM to invoke — not a second UI, not a second service, not a Live decision.

This is not more mechanical tool picks as a separate program. That job stays [the harness](jev-harness.md) ([#546](https://github.com/unarbos/arbos/pull/546)). Do not start slices A–G as a second program. Do not publish `v0.2.0`. The Mac update channel carries the merge.

---

## 1. Words used here

- **Jev**: TypeSafe’s System One model. It answers typed questions. It does not write prose. It does not speak. OpenRouter slug `typesafe/jev-latest`.
- **Controller**: the one Jev call on a kernel turn. Same door as #546. Same key.
- **Live**: the voice model on the call (GPT Live today). It hears Jacob. It speaks. It may **delegate** a question to the kernel.
- **Gateway**: `voice-server/`. Phone and desktop connect to it. It connects to Live and to one kernel.
- **Kernel**: `arbos-kernel serve <folder>`. The agent loop lives here. Jev already runs here.
- **Standing brief**: a small file the kernel keeps at `.arbos/voice-brief.md`. Existing lines only. What a mouth is allowed to know.
- **Slice**: one candidate line for the brief. It already exists. It has an id, a clipped body, and sometimes an address.
- **Pack**: fill the brief budget with the highest-ranked slices. Packing is selection. It is not writing.
- **Pointer**: an address for a true slice that did not fit (`notes.md#tldr`, `agents/run-tests`). Overflow. Not a second page.
- **Fall-through**: Jev errors, times out, or returns junk → today’s one-model loop and today’s inject. The turn and the call still work.
- **`jev = false`**: the off switch. No brief file. No model field applied. Today’s inject. Today’s one-model loop.

---

## 2. Laws

1. **Jev does not speak.** Live and the narrator stay the mouth. Tool results go back through the language model or through Live. Jev never talks to Jacob.
2. **Jev does not judge small talk.** Live still decides greetings vs a kernel ask.
3. **Jev does not sit in the gateway.** The gateway only **reads** the brief.
4. **Do not block `session.start` on Jev.** The seed is a file read. If the file is missing or unreadable, that is **unknown**, not empty: inject identity only. Do not pretend the project is idle.
5. **No vault values. No file bodies.** Glances and paths only.
6. **Identity is pinned in code.** Folder, machine, `arbos://`, via. Jev cannot drop them. That is the #492 lesson.
7. **Model choice is not a feature.** It is one field on the controller object. No settings page. No extra OpenRouter call. No Live-side pick.

---

## 3. One loop

```
kernel turn (not Live, not the gateway)
        ↓
code gathers slices (existing text, already clipped)
        ↓
one Jev call — the controller:
  act / tool / done
  model: which LLM to invoke on an llm (or need-say) step
  keep / pointers: what the brief holds
        ↓
act=tool  → run that tool; results stay on the turn
act=llm   → invoke the named chat model; it writes or talks
act=done  → end; if nothing was said, the LLM says it
        ↓
code packs the brief (budget ~2,000 tokens)
        ↓
write .arbos/voice-brief.md  (or delete it when jev = false)
        ↓
session.start / a later inject:
  gateway reads the file
  Live gets the brief
  Jev is not on this path
```

When no turn is in flight, the kernel still **gathers and packs in code** (deterministic order: tldr, working, last, ask, branch). It does not call Jev just to refresh. Jev ranks when it is already being asked “what next.”

That is the complexity removed: one JSON object, one key, one file, one off switch.

---

## 4. What we send Jev

The situation card from #546, plus two short lists:

```
slices:
  tldr     [notes]  "- [Voice](…) — live on the pod"
  working  [child]  "run-tests · cargo test"
  last     [say]    "All thirty tests pass."
  ask      [wait]   "Allow cargo publish?"
  branch   [git]    "cursor/jev-brief-picker-1bda"
models: fast=google/gemini-3.8-flash  powerful=<config model>
```

Same redactions as #546. No vault. No diffs. No whole files.

---

## 5. What Jev returns

One JSON object (today’s chat door; Decisions later, not a second program):

```json
{
  "act": "tool" | "llm" | "done",
  "tool": "grep",
  "args": {},
  "why": "short",
  "model": "fast" | "powerful" | "default",
  "keep": ["tldr", "working", "last"],
  "pointers": ["ask"]
}
```

- `act` / `tool` / `done` — the controller’s move. Tools it can parse run now. Prose, plans, and talk go to the language model. Done ends the turn.
- `model` — which **chat** model the next LLM invoke uses. Ignored on a pure tool step. Unknown slug → configured model (fall-through). Not in the menu → configured model. **Unknown `model` does not junk `act`.**
- `keep` — slice ids that stay as text in the brief.
- `pointers` — slice ids that become addresses only.

**Live never sees this object.** Code copies existing lines into the brief. The gateway injects that file as `STANDING BRIEF` next to today’s PROJECT IDENTITY.

How `model` resolves, on the same OpenRouter key and the same `fallback_models` list the turn already has:

| Value | Slug |
| --- | --- |
| `powerful` or `default` or missing | `config.toml` `model` (the primary) |
| `fast` | First menu slug that looks cheap (`flash`, `mini`, `haiku`, `lite`, `nano`, `small`), else the first fallback, else the primary |
| A slug already on the menu | That slug |

The menu is the primary plus `fallback_models` (OpenRouter defaults when the list is empty; `["none"]` forbids extras). Jev cannot invent a model.

`jev = false` never asks, so the field never applies. Replay never asks.

Jev is still not a fallback. A 403 still walks `fallback_models` as today. The field only names the **first** model for this LLM invoke.

---

## 6. The brief file

Path: `<place>/.arbos/voice-brief.md`

Identity is **not** in this file. The gateway already injects place from the roster.

The file holds glances:

```
tldr: - [Voice](docs/…) — live on the pod
working: run-tests · cargo test
last: All thirty tests pass.
branch: cursor/jev-brief-picker-1bda
overflow:
- ask → agents/root/waiting
```

A sibling `.arbos/voice-brief.json` holds the same sections as data so a client can parse without scraping.

Budget: about 2,000 tokens, so one inject always fits (Live append cap is 500 tokens for a *diff*; a start inject may be larger, but the brief stays small on purpose).

`jev = false`: the kernel **deletes** these files so the gateway cannot keep a stale pack. Today’s inject returns.

A read that fails is unknown. The gateway does not treat that as “nothing is going on.”

---

## 7. What Live gets

Unchanged inject kinds. One extra block when the file is present:

- PROJECT IDENTITY (code, pinned)
- ON-SCREEN CHAT (client snapshot)
- WORKERS AND ACTIVITY (as today)
- **STANDING BRIEF** (the file, if the read confirmed it)

`session.start` waits at most 1.5 seconds for chat history **and** this file, in parallel with the OpenAI connect. A miss drops the brief, not the call. **Jev is not in that wait.** Do not add a Jev wait to inject.

Mid-call: inject when the brief **hash** changes, not every 3 seconds. (Hash change can land after this first PR; the file is enough for start.)

“What are you working on” should be answerable from tldr + working without a kernel essay.

---

## 8. The three shapes, as layers

The comparison that got us here. Kept so the layers stay honest.

| Layer | Job in the controller | Not allowed |
| --- | --- | --- |
| **3 Standing brief** | The file. The read at start. The off-path refresh. | Jev at `session.start`. Jev on the gateway. |
| **1 Packing** | How `keep` becomes bytes inside the file. | Packing on the inject path. |
| **2 Index** | `overflow:` lines when a slice is true but over budget. | An index *instead of* the tldr sentence Jacob needs. |
| **Model field** | Which LLM the controller invokes. | A second UI. A second service. A Live-side pick. |

Rejected still: Jev classifies small talk; Jev writes the highlight; Jev in the phone; Jev hears audio; an LLM summary at start; hosting Jev; a standalone model picker.

---

## 9. Desktop and iPhone

No Jev chrome. The brief is not a second brain.

The same file may later feed the call strip and the phone “what is happening” line. This first PR does not change those UIs. The kernel file is the API.

---

## 10. Done when

- A turn with Jev on writes `.arbos/voice-brief.md` from glances. Identity is not in it.
- A slice that will not fit becomes a pointer, not a second page.
- The same Jev JSON may name `model: fast` / `powerful`. The next LLM invoke uses that slug when it is on the menu.
- Junk `model` keeps the configured model. The `act` still parses.
- `jev = false` deletes the brief and never applies a model field. Today’s inject. Today’s one-model loop.
- `session.start` only reads the file. It does not call Jev. A missing file does not block the call.
- No vault values, no file bodies, in the card or the brief.
- Live still speaks. Jev still does not.
- Tests pin: parse of `model` / `keep` / `pointers`; pack + overflow; delete on `jev = false`; unknown read is not empty; unknown `model` falls through.
- No `v0.2.0` publish from this work.

---

## 11. Related

- [Jev in the harness](jev-harness.md) — what #546 shipped
- [Jev, fully in the agent and the app](jev-full-integration.md) — tool router; do not start A–G as a second program
- [What GPT Live can see](gpt-live-context.md)
- [Desktop call mode](desktop-call-mode-design.md)
- [Work sound](call-mode-work-sound.md)
- Check list: [`media/jev-brief-picker/`](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/jev-brief-picker/)
