---
cursor:
  subagentId: "bc-2a1318aa-e675-52f4-b3ab-94cb9415aa39"
---

# #372's stranger-kernel control, driven on the rig — one bug, the rest holds

For the author of [#372](https://github.com/unarbos/arbos/pull/372), from the layout loop. Script: `/tmp/c23/stranger_drive.py` on the rig (three scenarios); stills in `media/cursor-reference/cycle-25/stranger-kernel/`.

Bundle: the app built from `6b186378` with `ARBOS_KERNEL_BIN` = a `main` kernel (`d73a25aea876`). Strangers: a kernel from the #366 branch (`9132d4a5e4c5`, answers `/healthz`) and the cycle-20 kernel (`794d98a64eec`, no gate).

| scenario | expected | seen |
| --- | --- | --- |
| A. stranger `9132d4` busy (`sleep 150` attached, turn running) | control, tooltip names the work, click ends it and respawns from the bundle | **control shown**; log: *"…is 9132d4a5e4c5 and this app ships d73a25aea876 — turns running: root"*. Tooltip right. **Click did nothing**: `kernel.json` still `9132d4`, control still there, no restart line in the log. |
| B. stranger `9132d4` idle | swapped silently, nothing shown | **as expected**: `kernel.json` → `d73a25aea876`, no control, chat attached live. |
| C. stranger `794d98` idle, too old for the gate | `unknown` counts as busy: control | **control shown**; tooltip reads *"…it is too old to say whether it is busy."* Click did nothing, as in A. |

## The bug

`status_bar.rs`, `plate()`: the `on_click` closure ignores the plate's `action` and always runs the update path —

```rust
.when(clickable, |el| {
    el.on_click(cx.listener(|this, _, _, cx| {
        // … collects places …
        this.updater.update(cx, |updater, cx| updater.install(places, cx));
    }))
})
```

so `Action::RestartKernel(place)` is never dispatched and `kernel::restart_kernel` (kernel.rs:349) has no caller. The tooltip promises *"Click to stop and restart it on this build"*; the click runs `install`, which with no update pending does nothing visible. Fix: match on `action` in the closure — `Action::RestartKernel(place)` → `kernel::restart_kernel(&place)` off the UI thread, then `forget_strangers` and re-attach the place's chats; everything else → the existing install path.

## Reads well

- Tooltip text (`old-02-tooltip-crop.png`): both builds named, *"work can finish and never be reported"*, what the click ends. Two small things: Jacob reads build labels (`0.2.0 (1185)`), not shas — the app's label is in the bar already; and the tooltip covers the control itself while hovered, which is fine.
- Bar at 1440×900 and 1100×700 (`busy-03-barcrop.png`): gear · amber **Kernel from another build** · `0.2.0 (1196)`; on the one surface the amber plate is the loudest thing in the window, as intended. Ordering kept: the stranger plate sits before the update control (no update was pending in these runs, so only the version showed to its right).
- While attached to the busy stranger, the chat showed the running prompt with nothing under it and an idle composer — the silent failure the PR describes, now with the bar shouting beside it. Good.
