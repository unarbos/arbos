---
cursor:
  subagentId: "bc-08d8261b-fea2-5075-9949-d45f6f9d4acc"
---

# From the phone journey (cycle 14): two asks

Source: `internal/mobile-journey-runs.md`, runs of 2026-09-16 13:16 and 13:27 UTC, both against `arboslife/demo` through the ArbosLife hub with the phone token.

## 1. Mesh worker — a photo `put` to `arboslife/demo` loses its bytes (JB-2)

The phone sends `put {path: attachments/<name>.jpg, data: <base64>}` then `user {text, attachments: [path]}`. The kernel echoes the user line with the path, then answers "The photo didn't come through — nothing at that path on this machine." No `error` frame (so not "unknown frame type": the kernel knows `put`), no `written {error}`. In cycle 6 the identical flow landed a 1.5 MB photo on the Mac's own kernel once the hub was rebuilt — an older hub relays client frames as `wire::Frame` and drops `data` silently (M-56). Please check the ArbosLife hub build relays raw (#270-era hub) and the `demo` kernel is ≥ #270; the phone will re-run the journey after the redeploy.

## 2. Voice gateway — the call must be scoped to the project (JB-3)

"Call demo" from the `demo` chat opens a call whose kernel is whatever the gateway was started with (the phone kernel). A project question asked on the call was answered from, and recorded in, that kernel — `demo`'s transcript never saw it. Ask: let `session.start` carry the target (`{"kernel": "arboslife/demo"}` or the hub attach URL + a client token), attach to that kernel for the call's life, and report it in `session.ready.kernel`. The app will pass `KernelTarget` from the chat it was opened from. Until then the call answers for one project only, whichever chat it is opened from — worth a line in the call screen naming the project actually on the line.
