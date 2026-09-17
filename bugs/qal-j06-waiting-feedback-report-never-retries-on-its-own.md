# qal-j06: a feedback report that waited ("saved, and waiting") goes out only when the user sends another one — nothing retries it on its own

- Feature: desktop feedback delivery (#345, `desktop/src/feedback.rs::deliver_pending`, `desktop/src/view/root.rs::deliver_feedback`); `main` @ `0c82887b`
- Severity: medium. The sheet tells the user *"Saved, and waiting: it could not be sent yet — … It will go by itself when the link is back."* It does not go by itself. `deliver_feedback` has one call site — right after a new report is written — so a report made offline (or before credentials exist) is retried only when the user reports something else. A user who reports one problem offline and never reports again has a report that never leaves the disk, while the app told them it would.
- **Closed 2026-09-16 22:00 UTC** by #356 (`d152c70f`): the outbox drains on launch, on a kernel reconnecting, on a minute timer and on Send — only Send speaks to the user — and it walks every open place (the old path saw only the active one, so a report filed while another place was on screen was invisible even to a later Send). `fb-01` re-run 2/2 with the driver's new surface: after the unsendable Send, `feedback.message` = `{ok: false, "Saved, and waiting: … It will go by itself when the link is back."}`, `outbox.waiting 1`, `screenshot {attached: true, whole_screen: false}`; credentials written while the *other* place was in front and nothing sent → within 100 s `<A>/delivered`, `<A>` in the store, `outbox {waiting 0, sent_this_run 1}`, no second report folder written; the next Send says `{ok: true, "Sent. It reaches an agent within fifteen minutes … Reference …"}`; the poller took both.
- Scenario: `fb-01-feedback-report-written-delivered-picked-up`, check `fb-01-a-never-retried`; rollout `internal/qa/rollouts/20260916T211719Z-fb-01-feedback-report-written-delivered-picked-up/` (`parts.a.delivered_later: false`, `parts.b.delivered_marker: true`).

## Repro

App with `[feedback] address = "arbos://qa-b/beta/docs/feedback"` and `hub_home` pointing at a directory **without** `arbos/hub.toml`. Thumbs-down on an answered turn, type a note, Send.

- `<place>/.arbos/desktop/feedback-outbox/<A>/report.json` is written at once (9,255 B, note present, `screenshot.b64` beside it); within a second `attempts.json` says `{"attempts": 1, "error": "no feedback credentials at …/feedback-home/arbos/hub.toml — a report will not be sent under another token"}`. Correct, and the reason is clear.
- Now write `hub.toml` into `hub_home` (the link is back). Wait 75 s. `<A>/delivered` never appears; the store has no `<A>/`.
- Send a second report B: `<B>/delivered` appears within seconds and `beta/.arbos/docs/feedback/<B>/report.json` is in the store. A is still waiting (its 30 s backoff had not elapsed at the moment B was sent, and nothing looks again afterwards).

## Expected

"It will go by itself when the link is back" — a timer (the backoff table already exists: 30 s, 60 s, 120 s … 3600 s) or at least a retry pass at app start and whenever the window is touched, so a lone offline report reaches the store without the user having to report a second problem.

## Actual

`deliver_pending` runs once per Send. The backoff table decides *whether* a waiting report is due at that moment, but nothing schedules the moments.

## Suspected location

- `desktop/src/view/root.rs::deliver_feedback` — called only from the Send path (line ~2416). Add a periodic call (the smallest due backoff, capped at an hour) and one at start-up when the outbox has anything waiting.

## Fix

#356 (see the closing line above). Regression check: `fb-01` phase A → credentials appear → `<A>/delivered` within 75 s with no second report sent (`fb-01-a-never-retried`).
