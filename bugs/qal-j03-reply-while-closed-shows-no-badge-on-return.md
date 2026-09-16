# qal-j03: a reply that lands while the app is closed shows no badge when the user comes back

- Feature: kernel notifications (#293) as the desktop draws them after a relaunch (#297 tab dot, #329 driver surface); observed on #329's branch (`4bee2319`, desktop and kernel built from it), the away-tab case on the same build works
- Severity: medium — this is the "leave and come back" moment. The reply is in the transcript, the kernel recorded the notification, but the tab shows no dot and the project's unseen count is 0, so the user has no cue that anything happened while they were away. The same reply arriving while the app is open on another tab does badge, notify the OS, and clear on click.
- Journey step: **J6 (leave and come back — notifications)**. Scenario `journey-linux`; failed 2/2 on this build: rollouts `internal/qa/rollouts/20260916T162657Z-journey-linux/` and `20260916T163147Z-journey-linux/`.

## Repro

Desktop app on a project, kernel serving. Send `Run \`sleep 15\` with bash, then reply with the single word LATER-<tag>.`; as soon as the chat is busy, quit the app (the kernel keeps serving). Wait until root's transcript has the `LATER-<tag>` reply. Relaunch the app with its own `state.toml` (no reseed). Before clicking any tab, read the driver state for the project.

Seen (both runs): `{"tab_dot": false, "unseen": 0}`; the transcript has the reply; `.arbos/notifications.jsonl` has the entry (`{"id":9,…,"kind":"reply","body":"LATER-J17934"}`); `runtime/notifications-seen` reads 12 by the end of the run.

Control on the same build, same run: the app open, the home tab in front, a reply with a nonce lands in the project → `unseen: 1`, `tab_dot: true`, one OS notification posted carrying the nonce, **confirmed in `dunstctl history`**, cleared on clicking the tab. So the badge path works; what is missing is the replay-on-attach path.

## Expected

`hooks.rs::notify` says: *"recorded in `.arbos/notifications.jsonl` and sent as a `notify` frame to every client now; a client that attaches later gets the unseen ones replayed."* On relaunch the project's chat is not the one being looked at (the home tab is in front), so the replayed notification must land in `unseen` and the tab must show its dot until the user opens that chat.

## Actual

No badge, unseen 0. Two candidates, the record cannot separate them:

1. **Kernel:** the unseen notifications are not replayed to the attaching client (or are replayed with ids ≤ the kernel's `notifications-seen`, which something advanced).
2. **Desktop:** during restore there is a moment when the restored chat counts as "being looked at" (`reap_notifications`: `window_active && active_ix == ix && active == chat.id`) and `mark_seen()` sends `seen(through)` for everything before the tabs settle on home — which would also advance the kernel's seen marker for every other client.

Tracing gap that made this a guess: `kernel.log` records `attach_open`/`attach_close` but neither a `notify_replayed` nor a `seen` event. One line each would tell which of the two it is.

## Suspected location

- `crates/arbos-kernel/src/serve.rs` — the attach path: where `arbos_core::notify::unseen(&place)` should be sent as `Frame::Notify { replayed: true }` after the snapshot.
- `desktop/src/view/root.rs::reap_notifications` and `desktop/src/model/session.rs::mark_seen` — whether a restore-time frame marks the chat seen before the user has looked.
- `crates/arbos-core/src/notify.rs` — the seen marker file.

## Fix

**Kernel half checked and clear (features agent, 2026-09-16 16:50 UTC, [#334](https://github.com/unarbos/arbos/pull/334)).** `notify_away_e2e` runs the exact case — a turn ends with nobody attached, the reply is recorded — and the next client gets `notify {replayed: true, id: 1}` after `hello`, both on the same kernel and on a new kernel after a restart; the seen mark does not move until a `seen` frame arrives. So `serve.rs` sends the replay. The fault is on the desktop side: either `notify {replayed: true}` is not counted toward the badge on restore, or a restore-time `seen` clears it before the user looks.

Two log lines now tell which, permanently: `notify_replayed who=… count=N ids=a..b seen_through=S` on every attach, and `seen_marked through=T newest=N was=S now=S' unseen_left=U` on every `seen`. In the failing rollout's `kernel.log`: `notify_replayed count=1` followed by `seen_marked … unseen_left=0` with no click means the desktop cleared it on restore; `count=0 seen_through=<newest>` means it had been cleared before this attach. Regression check: `journey-linux` J6 (`notify_before_click` must show `tab_dot: true, unseen ≥ 1` after a relaunch with a reply landed while closed), plus a kernel e2e: record a notification with no client attached, attach, expect a `notify` frame with `replayed: true` and the id, and no `seen` advance until a `seen` frame is sent.
