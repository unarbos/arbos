---
cursor:
  subagentId: "bc-7c66cfa8-381e-5700-9d78-3129f338a4fa"
---

# The text does not grow with the system setting

Nobody had looked at this. Cycle 191 did, and the answer is that the phone
app's text is a fixed size whatever iOS is set to.

## Measured

The simulator's content size was set to `large` (the default) and then to
`accessibility-extra-extra-extra-large`, with the app relaunched each time.
The simulator confirms it took the setting both times.

| | list rows visible | labels ending in an ellipsis | controls in the chat |
|---|---|---|---|
| `large` | 11 | 1 | 5 |
| `accessibility-extra-extra-extra-large` | 11 | 1 | 5 |

The screenshots are the same picture: of roughly 81,000 sampled pixels, **13
differ** — the clock, and a project's age ticking over.

## Why

`Theme.swift` builds every font from `Font.system(size:)`:

    static let title  = Font.system(size: titleSize, weight: .semibold)
    static let body   = Font.system(size: bodySize)
    static let callout = Font.system(size: calloutSize)

`Font.system(size:)` is a fixed size. SwiftUI's text *styles* — `.body`,
`.callout`, `.title` — are the ones that scale, and the app uses none of
them.

## Why this is a decision and not a bug to fix

It may well be deliberate. This app's geometry is tight and deliberately so:
the list's row pitch sits within half a point of Cursor's as a share of
screen height, measured repeatedly since cycle 83, and the chat's left margin
likewise. Dynamic Type would move both, and several of the loop's own checks
are built on those numbers holding.

There is also only one user, on one phone, and if his text size is the
default then nothing about this is costing anything today.

Against that: it is the standard iOS accessibility setting, a person's eyes
change, and the desktop app's behaviour here is not known — so "consistent
with the Mac desktop" cannot be claimed either way yet.

**The question: should the phone's text follow the system size?**

If yes, it is a change to `Theme.swift` — text styles with `.dynamicTypeSize`
bounds, most likely, so a very large setting does not tear the layout apart —
and several harness checks would need their fixed expectations revisited.

If no, it is worth writing down as a decision rather than leaving it as an
accident of how the theme was first written, because the next person to read
`Font.system(size:)` will not know which it was.

## Where it is measured

`deploy/mobile/scenarios/text-size.sh` reports the app at two sizes every
time it runs. It does not judge, for the same reason the two-faces check does
not: the answer is a product decision, and a check that fails until someone
makes it is a check people stop reading.

Filed by the mobile loop at cycle 191. No app change made.
Related: `2026-09-19-one-project-two-faces-which-one-wins.md`.
