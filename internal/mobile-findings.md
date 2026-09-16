---
cursor:
  subagentId: "bc-08d8261b-fea2-5075-9949-d45f6f9d4acc"
---

> **Partial rebuild, 2026-09-16 12:50 UTC.** The store lost this ledger (with `features-inbox/` and every other `mobile-*.md`) between 12:12 and 12:39 UTC; see `internal/mobile-store-loss-2026-09-16.md`. Rows M-01…M-79 existed in full and are gone; what follows for them is what the loop's context still held (the rows that were open at the loss, verbatim where the text survived, and one line per fixed row from the PR record). M-80 onward is complete.

# iPhone loop — findings ledger

Columns: id · date · source · area · what · state.

## Rows M-01…M-79 (reconstructed summary)

Fixed, by PR — the detail lives in the PR bodies on `unarbos/arbos`:

- **#231 (cycle 1)** M-01…M-10: projects list with face/colour/name from the desktop tab identity; project chat as a navigation destination; worker lines; `Theme.swift` from the desktop palette.
- **#242 (cycle 2)** type scale and spacing retuned to the Cursor reference (17 pt body, 22 pt title); worker pill shows only touched workers.
- **#246 (cycle 3)** M-33…M-40: call screen redesigned — voice first, pulled down; `VoiceOrb`, level meter from an output tap.
- **#260 (cycle 4)** M-41 reply-once (`step` pairing, #247); real project faces from the roster (#233); TestFlight pipeline, app icon, `ExportOptions.plist`, CI workflow.
- **#268 (cycle 5)** M-47 composer as a bottom inset; M-48…M-52 history paging, soft reconnect, resume on foreground, item cap 2000; "Link lost — reconnecting in Ns".
- **#269/#274 (cycle 6)** M-53…M-59: attachments via `put`, dictation into the composer, backward history via `before`, one calm line on a lost link; M-60 baked phone token + `check-secrets.sh` guard; M-61/M-62 tool-markup stripping in the stream.
- **#290 (cycle 7)** M-63 hub URL only moves when the host is truly gone; M-64 build number in Settings; M-65 refusal by name ends the retry; M-66 pill/composer background; M-67/M-68 replayed spawns rebuild finished workers; M-69 harness px→pt.
- **#291** ArbosLife cutover baked into the phone build.
- **#296 (cycle 8)** M-70…M-73: `notify`/`seen`, the away card, local notifications, launch-arg hub URL not clobbered.
- **#302 (cycle 9)** M-74…M-76: APNs registration through the attach socket, `enabled: false` handled honestly, push opens the right project, old-hub `push` error swallowed.
- **#304 (cycle 10)** M-77…M-79: the call's `+` takes a photo or a file.

Still open at the loss (text as it survived):

| id | date | area | what | state |
| --- | --- | --- | --- | --- |
| M-09 | 09-15 | projects list | the hub roster carries no face for a project the phone has never attached to → default folder glyph and a hashed colour | largely closed by #233 + #260; hashed fallback stays for unknown projects |
| M-19 | 09-15 | Agents pill | counted every child ever in the tree, not this session's | addressed cycle 2 (`touched` set); keep an eye |
| M-27 | 09-15 | worker chat | a remote worker's chat shows only local placeholder lines ("Nothing on record yet") | **open — kernel**: worker history over the hub (re-seen cycle 13, `media/mobile/cycle-13/06-`) |
| M-30 | 09-15 | call | first reply 0.67 s, but the model answered the project question generically | closed by gateway PR #56 (kernel answers) — see M-85 |
| M-31 | 09-15 | call screen | Jacob's reference set has no call screen | closed by cycle 3's own design |
| M-36 | 09-15 | barge-in | 2.4 s through the gateway's echo gate | closed — M-85 |
| M-52 | 09-16 | history paging | `history` paged forward only | closed — kernel #272 |
| M-54 | 09-16 | kernel wire | a photo could not reach the agent (no frame carried bytes) | closed — kernel #270 |
| M-56 | 09-16 | arbos-hub | an older hub dropped `data` and `before` silently | closed — hub redeployed |
| M-58 | 09-16 | pod hub token | 401 to the baked token | closed — M-60, `client-phone` token |
| M-63 | 09-16 | connect | failed attach rewrote `settings.hubURL` silently | closed — #290 |
| M-71 | 09-16 | push | a suspended phone cannot be reached by the app itself | **open — needs Jacob's APNs key + capability**; app half done (#302) |
| M-78 | 09-16 | call / voice server | barge-in on a ~3 s reply not achieved; the reply ignored the project | closed — gateway PR #56, M-85 |

## Rows M-80 onward (complete)

| id | date | source | area | what | state |
| --- | --- | --- | --- | --- | --- |
| M-80 | 09-16 | loop (cycle 10) | harness | the iOS notification permission alert stopped taking `idb` taps in this simulator session (it worked twice in cycle 9); every scripted pass after it is blocked until the alert is answered | open — reboot the simulator before the run, or pre-answer via the Simulator app's GUI |
| M-81 | 09-16 | Jacob (F5) + coordinator | chat | a line that stays "Sending…" while the project's link is down is silent for as long as the link is down — 98 s on the desktop this morning, minutes on the phone; silence reads as broken | fixed #306 (build 994) — after ten seconds without the kernel's echo the card says "demo is not answering — waiting" in muted text (not red); keyed on the missing echo, not on the socket's opinion, since the socket does not notice a cut for ~20 s; verified with a `pfctl` cut: line at +13 s, reply lands and the line clears when the cut lifts (`media/mobile/cycle-11/07-`, `08-`) |
| M-82 | 09-16 | steward via coordinator | chat | the pending-line isolation fix (#306) was reasoned from Jacob's screenshots, not shown live | **proven** cycle 12, two ways: link cut (`pfctl`, port 443) and kernel killed (local hub, two kernels); no marker reached the other project in either — screen, transcript file and kernel log all clean (`media/mobile/cycle-12/01-`–`05-`). With the socket only blocked, TCP delivered the line to its own project when the block lifted; with the kernel dead and a switch, the line is dropped — decision for Jacob whether to hold it instead |
| M-83 | 09-16 | loop (cycle 12) | chat | opening a project while the link is down showed a blank page — no history, no "Reconnecting…" — for as long as the link was down | fixed #309: while connecting the page says "Opening pod…", after ten seconds "pod is not answering — waiting"; **verified** cycle 13 with the cut placed before a cold start (`media/mobile/cycle-13/08-`–`10-`) |
| M-84 | 09-16 | loop (cycle 12) | list | a project whose kernel dies vanishes from the list (hub roster drops it); Jacob's `demo` stayed listed during the hub redeploy because the hub itself was gone | note — a project that was here a minute ago should stay listed, dimmed "not answering", so its chat (and its pending lines) can still be opened; hub or app-side memory, to decide |
| M-85 | 09-16 | gateway + loop (cycle 12) | call | barge-in re-measured on gateway PR #56: 401/404 ms (not achieved in cycle 10, 2.4 s in cycle 3); kernel-answered first audio 2.93 s; small talk 0.70 s; app confirmed not to forward `transcript.final` in duplex; `client.speaking` now carries `route` | done — AirPods leg needs Jacob's phone (`internal/voice/mobile-call-measurements.md`) |
| M-86 | 09-16 | loop (cycle 13) | harness | "the transcript would not scroll back down" was the harness: `idb ui swipe` starting at y ≥ 750 pt begins on the pill/composer inset, not the scroll view; swipes that start inside the transcript (y 150–650) scroll both ways | fixed in `mac-cycle13.sh`; the `scrollPosition` change made on that false reading was reverted before it shipped |
| M-87 | 09-16 | loop (cycle 13) | chat | F12 verified live and recorded: a 20-line reply streamed for 14 s while the view was scrolled up two screens; the view did not move (`media/mobile/cycle-13/02-`, `03-`, `recording-…mp4`) | done — #309 |
| M-88 | 09-16 | loop (cycle 13) | list | after a cold start with the hub unreachable, the list fell from four cached rows to a single `pod · Idle` row and stayed there | **found and fixed** (cycle 15, #319): when the hub fetch failed, `list` held only the pod row, was non-empty, replaced `entries` and was cached — so the shrunken list survived relaunches. Now a hub that does not answer keeps last time's rows marked Off and does not overwrite the cache; verified offline cold start keeps four rows (`media/mobile/cycle-15/01-`), live again on relaunch (`02-`) |
| M-89 | 09-16 | store | ledger | `internal/` lost `features-inbox/` and every `mobile-*.md` (this ledger, coverage, feedback log, TestFlight, Mac host, catalogue) between 12:12 and 12:39 UTC | rebuilt what the context held; `internal/mobile-store-loss-2026-09-16.md`; the loop now keeps a copy of its `internal/mobile-*` and `features-inbox/*mobile*` files on the Mac at `~/mobile-docs/` after every write |
| M-90 | 09-16 | Jacob (F18) + loop (cycle 14) | voice gateway | dictation finals are routed to the gateway's own kernel as a turn (PR #56's answerer routing runs in dictation mode); a note dictated in any project lands unsent in `pod` and gets answered; Jacob saw the loop's own test phrase in his `pod` chat | app stopgap in #319: dictation starts with `answerer: "model"`, `pod` stays quiet; gateway ask filed (`features-inbox/2026-09-16-mobile-dictation-routed-to-kernel-and-tool-output.md`) |
| M-91 | 09-16 | journey JB-3 | call | "Call demo" talks to the gateway's fixed kernel, not `demo`; 0/3 runs landed the spoken turn in the project | open — gateway: `session.start` carries the target; app passes `KernelTarget` (ask filed `…-mobile-journey-photo-and-call-asks.md`) |
| M-92 | 09-16 | loop (cycle 15) | list | after the link returns, rows stay "Off" until the app is relaunched or pulled; the roster does not re-fetch by itself | open — retry the roster with backoff while hub rows are Off |
