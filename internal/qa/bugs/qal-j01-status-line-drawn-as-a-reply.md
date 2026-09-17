# qal-j01: the model writes `status "Looking around the new place"` as a reply line, and the chat draws it as a reply

- Feature: the `status` live line (kernel `StatusTool`, prompt line 22 of `crates/arbos-engine/src/prompt.rs`) as the desktop draws it; `main` @ `c964294c`, model `google/gemini-2.5-flash`
- Severity: medium, but it is the first thing a new user sees. The kickoff turn on a fresh project shows three bubbles — `status "Looking around the new place"`, `status "Writing project context"`, `status "Setting plan"` — above the greeting, as if the agent were talking in code. The same happens on every later turn (`status "Spawning worker to fix test and add changelog"`).
- **Closed 2026-09-16 15:20 UTC** by #320 (`main` @ `4b387fce`): a status written as text is the live line, not a reply, in every form. `status_drawn_as_reply` empty in 2/2 desktop journeys on that build (it had been 11× on `c964294c`). J3 keeps the check.
- Journey step: **J3 (watch it work honestly — the status reads honestly)**; also visible in J1. Scenario `journey-linux`; rollouts `internal/qa/rollouts/20260916T132732Z-journey-linux/` and the run after it. Failed in two consecutive runs → named bug per `docs/acceptance-journeys.md`.

## Repro

Fresh place, desktop app on it, model `google/gemini-2.5-flash`. Let the kickoff turn run. Root's transcript:

```
1 assistant  status "Looking around the new place"
2 tool       bash
3 assistant  status "Writing project context"
4 tool       write
5 assistant  status "Setting plan"
6 tool       plan
7 assistant  Hey — the place is ready, …
```

The driver's item list for the chat has four `agent` items: three `status "…"` lines and the greeting.

## Expected

The prompt says: *Say what you are doing: `status "<-ing verb> <the thing you are on>"` … at each major step*, and the tool schema adds *"A tool call, not a line of text in your reply."* When a model writes that exact form as text anyway, the user should see what the prompt promises — the live line beside the agent's name — and never a reply bubble that reads `status "Setting plan"`.

## Actual

The kernel records the line as ordinary assistant text; the desktop's `status_line()` (`desktop/src/model/session.rs:3817`) only recognises the `status: …` / `Status: …` colon form that the kernel writes for a real `status` call, so the quoted form the prompt itself teaches is drawn as prose. #278 strips tool markup written as prose (`<invoke>`, `<tool_call>` …) but this form is not markup, so it passes through.

## Suspected location

- `crates/arbos-engine/src/turn.rs` where assistant text is settled before it reaches the transcript (the #278 seam, `markup::strip_tool_markup`): a whole-line `status "…"` (or `status: …`) reply is the status tool called by hand — set the status, drop the line, nudge once as for a JSON call in text.
- Or, narrower, `desktop/src/model/session.rs::status_line`: accept `status "…"` too. That hides it on the desktop only; the phone and the web client would still show it, so the kernel is the right place.

## Fix

#320 (see the closing line above). Regression check: `journey-linux` J3 (`status_drawn_as_reply` in the evidence) and a kernel e2e with a replay reply of `status "Reading the file"\n` followed by prose: the transcript's assistant line has no `status "` prefix and the agent's status reads "Reading the file".
