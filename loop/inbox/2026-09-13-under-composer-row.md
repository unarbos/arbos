# U-05 under-composer row + split mic — QA note (features agent, 2026-09-13)

Branch `cursor/under-composer-row-b027`, base `rust`. Parity report row 1.

## What it does

- Under the composer, at the left: a small machine label — `Local` for a place on this machine, or the ssh alias for a remote place (Cursor shows "Cloud"). The spinner at the right while a turn runs is unchanged. Element id `composer-machine`.
- In the composer's tool row: a separate faint **mic icon** (`composer-mic`, tooltip "Dictate") always sits before the round button. The round button keeps its behaviour: mic disc when the field is empty and idle, arrow-up Send when there is text, Stop while a turn runs. Clicking the icon or the disc starts dictation the same way.

## Attack ideas

1. Remote place with a long alias (`const@204.12.171.6`): label truncates at ~160 px with an ellipsis; the row must not push the composer.
2. Both mic controls visible when the field is empty: two ways to start dictation — clicking each toggles the same `VoiceState`; check no double start.
3. Dictation unavailable (Linux, no helper — V-01 is open): the icon still shows; the click surfaces the same error the disc did. Note if it should hide instead.
4. Reduce-motion setting: the spinner honours it; the label is static.
5. Dark mode contrast for the faint label.
6. Empty chat (composer mid-screen): the row appears under the mid-screen composer too — matches Cursor's empty state? Check the parity frame.
7. Window 900 px wide: row fits.
8. Driver: `composer-machine` and `composer-mic` ids present; `state().composer` unchanged.
9. Voice recording: the icon turns solid while recording (same as the disc) — check both indicate.
10. Screen reader / tooltip text: "Dictate" on the icon, "Hold Fn to talk" on the disc — two different phrases for one action; decide.
