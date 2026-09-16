---
cursor:
  subagentId: "bc-08d8261b-fea2-5075-9949-d45f6f9d4acc"
---

> Rebuilt 2026-09-16 12:50 UTC from the loop's context after the store lost this file (see `internal/mobile-store-loss-2026-09-16.md`). The table is as it stood at cycle 10 plus cycles 11–13.

# iPhone loop — coverage record

What the loop has exercised and when, so nothing goes long untested. One row per aspect; the last cycle that checked it, how (simulator / Jacob's phone), and what is still open. Rotation rule: every aspect within four cycles; anything older than that goes first in the next cycle. Standing order (Jacob, 2026-09-16): never idle, style pair + still every cycle, a recording every third, long-running projects, every aspect.

| aspect | last checked | how | state / open |
| --- | --- | --- | --- |
| projects list — faces, rows, sections | 16 (offline start → rows come back by themselves, M-92 fixed) | simulator, ArbosLife hub, phone token | four real projects with their faces; a project moves to "Working" while its kernel runs; a dead kernel's row vanishes (M-84, open); with the hub unreachable at launch the cached rows show "Off" (right) |
| projects list — search, filter, refresh | 25 | simulator, ArbosLife hub | search filters as typed ("sub" → subnet120); filter menu All / Live only; a new project (`Feedback`) from the roster appeared by itself (`media/mobile/cycle-25/01-`). Note: with a filter on, the list composer still names the last project, not the one shown |
| list composer → names its project, rides above the keyboard | 11 | simulator | placeholder "Message demo…" (F1); bottom inset above the keyboard (F2) |
| project chat — send, prompt card, streaming, Worked line | 13 | simulator, `demo` | a 20-line reply streamed 14 s; the view held where the reader scrolled (F12, recorded); "Sending…" → "demo is not answering — waiting" at 10 s (M-81) |
| project chat — worker lines, Working pill, workers sheet | 18 (M-100 fixed: Done means done on old-kernel spawns), journey ×9 | simulator, `demo` | "Agents 3" pill → sheet lists three finished workers |
| worker chat — open from line / sheet, back | 13 | simulator, `demo` | opens; still "Nothing on record yet" for a remote worker (M-27, kernel — still open) |
| project chat — long history, scroll, older lines | 6 | simulator, `longproj` (450 lines), kernel #272 | opens at the tail; "Show N earlier lines" pages 200 back and holds the place (iOS 18+) — **due** |
| project chat — several workers at once, archived children | 7 | simulator, `subnet120` | finished children in the sheet, kept across a reopen (M-68); older-kernel spawn names (M-67) — **due** |
| call — voice first, orb, colours | journey ×3 (connect 0.28–0.45 s; kernel-answered first audio 5.8–6.8 s on the pod today, was 2.9 s) | simulator, injected clips, gateway PR #56 | kernel-answered first audio 2.93 s, small talk 0.70 s |
| call — pulled down: type, mute, close, `+` | 10 | simulator | `+` live (M-79); photo path on the call to re-check once the prompt harness is fixed (M-80) |
| call — barge-in | 12 | simulator | **401/404 ms** (M-85); AirPods leg needs Jacob |
| call — AirPods / speaker route, screen off, CallKit | — | needs the phone | **never checked**; Jacob's phone |
| attachments (`+`), photos, files | 14 (`phone`: photo drawn on the card, model saw it), journey ×3 (`demo`: bytes lost on ArbosLife, JB-2) | simulator, library photo, kernel #270 + rebuilt hub | a 1.5 MB photo landed and reached the model; an older kernel's refusal now reads plainly (F10) — **due** for a live re-check |
| voice notes in the composer | 14 (words wait in the field; gateway routing stopgap M-90) | simulator, injected clip | deltas append, whole sentence kept, second tap sends (F9); real mic on Jacob's phone: his report on 956 was the bug, re-check on the next build |
| settings sheet | 25 | simulator | Voice / Arbos kernel / Mesh hub with tokens "saved", Notifications with the push line, Arbos 0.2.0 (build) footer; editing a token still not exercised |
| notifications (`notify`, push) | 22–24 (away card on `demo` in-run; `GET /push` reads `enabled: false` + reason, `/push/test` 503 until Jacob's key) | simulator, `simctl push` with the hub's payload | banner, badge, tap-to-project, away card, seen both ways all verified; real pushes start when Jacob's APNs key + capability land (no release needed) |
| background 8 s → foreground | 13 | simulator | list intact (the SpringBoard kick in the harness drops the status bar; harness, not app) |
| background minutes/hours → resume; what a returning user sees first | 27 (12 min, `pod`) | simulator | the chat as it was, then a line sent straight after resume answered in 7 s — the resume reconnect works with M-109 in (`media/mobile/cycle-27/01-`); hours-long suspension still needs Jacob's phone |
| network drop → reconnect | 23 (**M-109**: the automatic reconnect never completed — fixed; kernel killed and restarted → reattached first retry, ask card re-offered), J8c ×8 | simulator | one calm line; pending lines never cross projects (M-82, proven both ways); a project opened while down says "Opening…" then "not answering — waiting" (M-83, verified) |
| cold start | 13 | simulator | lands on the list, four rows, 6 s |
| TestFlight build on Jacob's phone | 956 (13 reports), 994 (fixes for F1–F6) | TestFlight | F7/F8 open until he confirms; F9–F13 land with #309 |
| style pair vs Cursor stills | 2 (list, chat), 3 (call vs GPT), 4 (list), 5 (chat, four workers), 6 (composer with a chip), 7 (list search) | 17 (chat), 18 (list), 21 (call vs GPT: `media/mobile/cycle-21/01-`) | pairs in `media/mobile/cycle-N/` | next: composer with a photo chip |
| recording | 1, 3, 6, 10, 13, 16, 28 (the photo landing and the scoped call answering from `demo`, 66 s, `media/mobile/cycle-28/recording-photo-and-scoped-call-66s.mp4`) | mp4 | next due cycle 31 |

**Journey runs** (`internal/mobile-journey-runs.md`): 22 so far — 3 on `demo` (old numbering; JB-1…3 named), 3 on `pod` on QA's ids (run 4 a floundering root, runs 5–6 clean apart from the U steps). Next: `arboslife/demo` on QA's ids once its kernel is updated.
