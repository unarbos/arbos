# qa-015: the desktop driver's `screenshot` works only on macOS

status: confirmed (QA works around it by grabbing the Xvfb root with `xwd`)
severity: low (test tooling; no user impact)
scenario: desktop-huge-transcript-scroll, desktop-rapid-session-switch, desktop-kill-kernel-under-ui
rollout: /cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/qa/rollouts/20260913T012820Z-desktop-huge-transcript-scroll
feature: desktop driver (`desktop/src/driver.rs`, `desktop/driver/arbosdriver.py`)
fingerprints: none

## Repro

Launch `arbos-desktop` under Xvfb with `ARBOS_DRIVER_SOCKET` set, connect with `arbosdriver.Arbos`, call `screenshot(path)`.

## Expected

A PNG of the window on any platform the app runs on.

## Actual

`DriverError: screenshot: find the AppKit window`. Everything else in the driver (state, click, type, scroll, hover) works on Linux.

## Suspected location

`desktop/src/driver.rs`, the `screenshot` method: looks up an AppKit `NSWindow`. On Linux, gpui can render the window to an image, or the driver can shell out to `xwd`/`import` on `$DISPLAY`.

## Notes for the desktop team

Under Xvfb with lavapipe (`mesa-vulkan-drivers`) the app launches in ~2 s, opens a 4,000-line transcript in 2.3 s, and takes 60 scroll events with none over 1 s. Six chats created and 35 sidebar switches in 16 s, no refused clicks. A kernel SIGKILLed under the window is respawned on the next prompt (pid 93248 -> 93269) and the app keeps running.
