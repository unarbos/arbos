# J-02 folded-sidebar title lead + J-03 composer/plan-strip alignment — QA note (features agent, 2026-09-13)

Branch `cursor/composer-plan-alignment-b027`, base `rust`. Found by Jacob on his Mac.

## J-02

Folded sidebar: the chat title started at a fixed 92pt while the fold cluster (traffic lights + fold button) ends near 120–124pt on macOS, so the title sat under the cluster. The lead is now derived: `cluster left (TOOLBAR_INSET, or HEADER_INSET in full screen) + cluster width + 14pt gap` → 134pt on macOS, 56pt on Linux, and it follows full screen.

## J-03

The composer card (`composer-card`) had `.w_full()` plus `-8pt` bleed margins; with an explicit width the negative margins shifted the card instead of widening it, so its right edge stopped 16pt short of the plan strip above it. `.w_full()` is gone; the card stretches like the `bleed()` strips. The four strips above the composer (plan, permission, questions, steer) now use `COMPOSER_RADIUS` (14) instead of `surface_radius`, and the plan/permission strips get the same 1px border as the card so the stack reads as one column of equal-width pills.

## Attack ideas

1. macOS folded, normal window: the title's left edge is right of the cluster by ~14pt; unfold: lead back to 14pt.
2. macOS full screen (no traffic lights): cluster at 16pt; title at 16 + 26 + 14 = 56pt.
3. Linux (no traffic lights): folded lead 56pt; check the title does not jump too far right.
4. Composer with a plan strip: both edges flush left and right at 1600 and 900 px widths; with the context rail open (U-01) too.
5. Drag-over highlight and drop still cover the whole card (the width change must not shrink the drop target).
6. The empty-chat mid-screen composer: same width as before (it lives in a different container — check it did not shrink).
7. Dark mode: the strips' border colour matches the card's.
8. A questions strip (ask tool) and a steer strip at once: both 14pt radius.
