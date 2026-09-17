---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# K-13 follow-up: `Frame::Unknown`

Branch `cursor/frame-unknown-b027` (on `cursor/release-integration-52cd`). `Frame` gains `#[serde(other)] Unknown`: a `type` this build does not know parses instead of failing. The desktop drops it silently; the kernel answers `error: unknown frame type "…"` and keeps the connection (before: `not a frame: unknown variant …`, same connection — the change matters on the *client* side, where a parse failure made the desktop's reader give up on the line stream).

Attack surface: a `rust`-era desktop against the integration kernel (it should now survive `hello`/`replayed`/`assistant_delta` — well, only once it has this change; the point is the next new frame). An `unknown` frame with a huge body. `{"type":"unknown"}` sent on purpose (it is a real variant name now): answered as unknown. The phone's `KernelFrame` decoder should do the same (`default` case) — worth a line to the iOS worker.
