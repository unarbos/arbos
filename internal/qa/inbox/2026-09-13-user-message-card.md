---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# For QA: right-aligned user message card (U-04)

From the features agent. Branch `cursor/user-message-card-b027` → `rust`. Parity report row 7.

## What I am building (desktop)

The user's prompt in the transcript was a full-width light-grey bar with a pencil at the right. Cursor draws a right-aligned white card with a soft shadow, ~10 px corners, capped width, dark text. Now: the bubble is right-aligned, capped at about 80 % of the reading column, white (raised surface) with a thin border and `shadow_sm`, 10 px radius; the pencil (edit and resubmit) shows on hover only, at the card's bottom-right. Slash-command prefix and attachments render inside the card as before. Selection, copy, and double-click-to-select-all are unchanged.

## How to exercise it

Any prompt; then a long one (200 words) to see wrapping at the cap; one with an attachment; one with `/model`; a multi-line one. Light and dark appearance (Settings › Appearance).

## What could break — attack here

1. Dark mode: the "white" card must be the raised surface tone, not pure white; check contrast of the text and the border.
2. A very short prompt ("hi"): the card should shrink to its text, right-aligned, not fill 80 %.
3. Hover pencil: reachable with the mouse on a card whose bottom-right is under the composer's shadow; keyboard users have no path (same as before).
4. Selection drag across a card and the answer under it (the card is narrower now; the drag must still select both).
5. Attachments with wide images inside the capped card.
6. Window narrower than the reading column: the card cap is relative, check it does not overflow the column.
