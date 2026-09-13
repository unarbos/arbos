---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# For QA: "Working N" / "PRs N" pills above the composer (U-02)

From the features agent. Branch `cursor/composer-pills-b027` → `rust`. Parity report row 4.

## What I am building (desktop)

Two pill chips directly above the composer, in the slot where the plan strip sits (left of it):

- **Working N** — sub-agent glyph + count of this chat's direct children with a turn running. Click → opens the first working child's chat. Hidden when zero.
- **PRs N** — git-branch glyph + count of distinct GitHub pull-request URLs found in this chat's and its children's tool outputs (`gh pr create` prints one; `gh pr view --json url` too). Click → opens the newest one in the browser; hover tooltip lists them. Hidden when zero.

No kernel change: both counts are derived on the desktop from what is already in the session (children's `busy()`, tool `output` text matching `https://github.com/<owner>/<repo>/pull/<n>`).

## How to exercise it

Parity p3 prompt (three sub-agents): "Working 3" then "Working 2" … then the pill disappears. For PRs: a chat that runs `gh pr create` (needs `gh auth`) or any bash whose output contains a PR URL — `echo https://github.com/unarbos/arbos/pull/21` counts.

## What could break — attack here

1. False PR counts: a PR URL quoted in a *read* of a markdown file, or in the model's prose (prose is not scanned; tool output is). `git log` output listing PR merge commits with URLs. Decide whether that is wanted.
2. Duplicates: the same URL printed by `gh pr create` and then `gh pr view` → one.
3. Very long outputs (100 KB bash) — the scan is per draw; check frame time with `Cmd+,` › Performance meter while a big output sits in the chat.
4. "Working N" while the parent itself is streaming: the pill counts children only; the parent's own spinner is elsewhere.
5. Pill + plan strip + queued-message bar all present at once: layout above the composer must not overflow; narrow window (900 px).
6. Click "Working" when the working child finished between draw and click: nothing should happen (no panic).
