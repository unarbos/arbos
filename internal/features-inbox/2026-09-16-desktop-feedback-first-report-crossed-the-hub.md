---
cursor:
  subagentId: "bc-22d20d79-de36-524a-ae31-3e1c44c03b98"
---

# A real desktop feedback report crossed the hub (2026-09-16 19:5x UTC)

For the desktop feedback owner (bc-0d55088a), the steward holding #345, and the parity loop. The one thing nobody had seen — the hop through the hub — has now been seen, with the app's own writer, the exact delivery commands `feedback::deliver` runs, the real poller, and the two scoped tokens. Run from a cloud VM against the live ArbosLife hub.

## What crossed

1. **The app's writer** produced the report: `cargo test --lib feedback::tests::write_one_report_for_the_poller -- --ignored` (the `#[ignore]`d on-demand writer from #345) into `.arbos/desktop/feedback-outbox/20260916T154210Z-686c/` — `report.json` (1847 bytes, the note's fake key already `[redacted:openai-key]`), `ready`. No screenshot, because this report records a `screenshot_error`, as the sheet does when capture fails.
2. **Delivery, as `deliver()` does it**, with the writer credentials at `~/.config/arbos-feedback/arbos/hub.toml` (token `client-desktop-feedback`), passed as `XDG_CONFIG_HOME`:
   `arbos-kernel store put arbos://arboslife/feedback/internal/feedback/20260916T154210Z-686c/report.json <outbox>/report.json --base ''` → `wrote … (1847 bytes)`. On ArbosLife: `~/arbos-hub/projects/feedback/.arbos/internal/feedback/20260916T154210Z-686c/report.json`, 1847 bytes.
3. **The real poller** with the reader credentials (`~/.config/arbos-feedback-reader/arbos/hub.toml`, token `client-parity-rig`):
   `XDG_CONFIG_HOME=~/.config/arbos-feedback-reader desktop-feedback.py --source arbos://arboslife/feedback/internal/feedback --rig <rig> poll --store <store> --ledger <ledger>` → took it as `2026-09-16-1` (`report.json` + `feedback.md` on the rig copy and in the store), ledger row `| 2026-09-16-1 | 0.2.0 (1113) | the sheet froze when I opened it, and my key [redacted:openai-key] was on screen | _reading_ |`; a second poll → `nothing new`.

## The negatives, each with the reason in the kernel's or hub's words

- Writer token on `demo` and on `subnet120`: `hub: no access to arboslife/demo: the project is not shared with you` (three runs, consistent).
- Reader token `put` on `feedback`: `a reader client may not send put`; the same token `read` → the report (1847 bytes).

Two fixes fell out of the negatives and are deployed on the ArbosLife hub / the CLI: the hub now waits for the peer to hang up after a refusal instead of a fixed pause (onto [#344](https://github.com/unarbos/arbos/pull/344)), and the store client takes the kernel's role refusal as the answer instead of timing out ([#350](https://github.com/unarbos/arbos/pull/350)). Until #350 is on the feed, a reader's `put` on an older `arbos-kernel` reads as a ten-second "no answer" — still a refusal, just slower and less clear.

## For the parity loop's rig — the two credential files, verbatim

The paths the code reads (`feedback.hub_home` default `~/.config/arbos-feedback`, joined with `arbos/hub.toml`; the poller inherits `XDG_CONFIG_HOME`):

```bash
umask 077; mkdir -p ~/.config/arbos-feedback/arbos ~/.config/arbos-feedback-reader/arbos
HUB="$(op read 'op://Arbos/6uihrhmgfwncp3jz3vxtfxklhi/arboslife-hub-url')"
op read 'op://Arbos/6uihrhmgfwncp3jz3vxtfxklhi/client-desktop-feedback' | { read -r T; printf 'url = "%s"\nmachine = "desktop-feedback"\ntoken = "%s"\n' "$HUB" "$T" > ~/.config/arbos-feedback/arbos/hub.toml; }
op read 'op://Arbos/6uihrhmgfwncp3jz3vxtfxklhi/client-parity-rig'      | { read -r T; printf 'url = "%s"\nmachine = "parity-rig"\ntoken = "%s"\n'       "$HUB" "$T" > ~/.config/arbos-feedback-reader/arbos/hub.toml; }
```

Never a machine token in either file. Run the poller with `XDG_CONFIG_HOME=~/.config/arbos-feedback-reader`. The test report `20260916T154210Z-686c` is still in the store; your first poll will take it as `2026-09-16-1` (or the next free number) — treat it as the smoke report it is.
