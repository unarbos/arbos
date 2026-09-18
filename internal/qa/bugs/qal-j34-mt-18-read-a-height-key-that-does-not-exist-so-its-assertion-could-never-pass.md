# qal-j34 — mt-18 read a height key that does not exist, so its assertion could never pass

- **status**: fixed in the rig (this loop's defect, not a product defect)
- **found**: 2026-09-18 12:06, triaging cycle 8's desktop-step breaks
- **kernel**: `arbos-kernel 0.2.0 d2a807e48423 protocol 1`; app at `2301abd291c0`
- **rule it printed**: `mt-18-page-fills-column`
- **rollouts**: `20260918T114343Z-mt-18-…` (the standing break), `20260918T120818Z-mt-18-…` (pass after the fix)

## What it claimed

`mt-18-project-page-not-in-chat-column` broke in **every cycle from 2026-09-17 18:18 onward** —
five in a row — with:

```
mt-18-page-fills-column: transcript height 0 of window 1000 with 21 open items above the composer
```

Read plainly: with 21 open items in `notes.md`, the project page has pushed the transcript out of
the chat column entirely. A height of **zero** should have been the tell — a rendered column
cannot be zero pixels tall while 21 items sit above it.

## What was actually happening

```python
tr_h = (tr or {}).get("bounds", {}).get("height") or (tr or {}).get("height") or 0
```

An element entry from the driver is

```
{id, path, x, y, w, h, cx, cy, visible, interactive, reachable}
```

— `desktop/src/driver.rs:969`, `describe`. There is no `bounds` and no `height`; the key is **`h`**.
So `tr_h` was `0` on every build and `tr_h >= win_h / 2` was **unconditionally false**. The
assertion could not pass, on any product, ever.

`win_h` was right only by luck: `(state.get("window") or {}).get("height") or 1000` does read a
real key, and the fallback `1000` happens to equal Xvfb's real height here, so a second latent
fault stayed hidden behind a coincidence.

## What the product actually does

With the key corrected, on the same app:

| | |
|---|---|
| `transcript_element` | `view-4294967298.conversation-drop.transcript-2` |
| `transcript_h` | **571.0** |
| `window_h` | 1000 |

The transcript keeps 57% of the column with 21 open items, against the 50% the scenario asks for.
`mt-18` passes, and its other assertion — that the page's `[label](url)` markdown never appears in
the chat items — had been passing all along. **Nothing was wrong.**

## The fix

Read `h`. And separate the two failures that had shared one rule, because they read completely
differently:

- `probe-no-transcript-element` — no element on screen whose path holds `transcript`, so the column
  was never measured and the run says nothing about the page. This rig's fault.
- `mt-18-page-fills-column` — the element was found, measured, and is genuinely short. The
  product's fault.

Synced to `internal/qa/multitasking_scenarios.py` in the same breath as the local edit, per review
rule 11.

## Where this belongs

The second rig fault found in cycle 8's break list within the hour, after `qal-j33`, and the fifth
today. Both of today's pair share a shape worth naming: **a measurement whose failure value is
indistinguishable from its failure to measure.** `qal-j33` typed at a window that was not listening
and reported the words missing; this read a key that does not exist and reported the height zero.
In both, the number the assertion compared was a default, not an observation.

That is review rule 6 asked of a measurement rather than a race: before trusting a red, check that
the quantity it names was actually obtained. A zero, an empty list and a `None` are what a missing
reading looks like, and they are also what the worst product failure looks like — which is exactly
why they must be told apart at the point of reading.
