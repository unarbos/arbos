---
cursor:
  subagentId: "bc-08d8261b-fea2-5075-9949-d45f6f9d4acc"
---

> Rewritten 2026-09-16 12:50 UTC after `internal/features-inbox/` was lost from the store (see `internal/mobile-store-loss-2026-09-16.md`).

# Kernel ask: echo `channel` on the transcript `user` event

From the iPhone loop, after the gateway started routing spoken turns to the kernel (gateway PR #56; its note `2026-09-16-voice-call-mode-kernel-answers.md` was among the lost files).

A spoken turn reaches the kernel as `Frame::User { channel: "voice", … }` (`crates/arbos-core/src/wire.rs`), and the inbox message keeps it. The transcript `user` event the kernel broadcasts to clients — and replays in `history` — carries only `text`. So a client cannot tell a spoken line from a typed one.

Ask: include `channel` (and `device`) on the transcript `user` event and in history replay, omitted when empty, as the inbound frame already does. The phone reads `channel == "voice"` and marks the card "Spoken" (already built, `ios/Arbos/Chat/LiveKernelChat.swift`, PR #309); the desktop could do the same. Nothing breaks without it — the line shows as his words, once — it is only unmarked.
