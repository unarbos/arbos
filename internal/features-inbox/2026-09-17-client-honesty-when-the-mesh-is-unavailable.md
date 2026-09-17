---
cursor:
  subagentId: "bc-22d20d79-de36-524a-ae31-3e1c44c03b98"
---

# Ask: say why the connection failed, on the visible line — desktop and phone

From the mesh worker, 2026-09-17 13:52 UTC, for the desktop owner and the
phone owner. Evidence and the full table:
`internal/mesh-client-failure-modes-2026-09-17.md`. Jacob spent three hours
today looking at *Connection failed* and a reconnect counter; every reason
was discoverable in one line, and the hub's side now always sends one
(#344, #417, #459). What is left is the client half. Five asks, small.

## Desktop (`desktop/src/view/component/transcript.rs`, `model/session.rs`)

1. **`short_error` must keep the reason.** Today any text containing
   `connection failed` becomes "Connection failed." and the reason lives
   behind Details. Make it one sentence per class, as the provider errors
   already get: `Connection failed: ssh to ArbosLife refused the key`;
   `…: no kernel binary on ArbosLife — installing (27 MB)…`; `…: <host>
   answered HTTP 404 — not an Arbos kernel or hub`; `…: ArbosLife did not
   answer (timeout)`.
2. **A fatal failure must not leave a bare tab.** `transient_connect_error`'s
   fatal list (`failed to start`, `is not a file`, `is not a directory`,
   `bad kernel url`) means one notice and no retry — the shape of the dead
   tab. Either keep the reason on the bar for as long as the state lasts
   (`ArbosLife · no kernel binary — press Reconnect to install`) or re-check
   every 60 s. The 26-minute retry loop's closing line ("retries stopped —
   send a message or press Reconnect") is good; the fatal path should say at
   least as much.
3. **The remote bootstrap should narrate.** When the new remote leg installs
   a kernel on the far side, the bar reads `reconnecting, try 8 in 14s` and a
   Send shows *Working · 3m 43s*. It should read `installing the kernel on
   ArbosLife (27 MB)…` — `connect_step` already exists for steps; the
   bootstrap needs to feed it.

## Phone (`ios/Arbos/Chat/ChatStore.swift`, `Kernel/ArbosKernelClient.swift`)

4. **Tell transport errors from refusals, and handle them oppositely.**
   `refusal(from:)` treats any unfiltered error text as a refusal, which
   sets `refusal` and stops the retry loop. A 502 from a tunnel whose hub
   restarts in five seconds, or a 404 from a wrong address, is a transport
   error: it should keep the waiting card and the 2–15 s retry, and *name*
   what happened: `hub-api.arbos.life answered 404 — not an Arbos hub` /
   `the hub is not answering (502) — retrying`. A refusal is only an
   `error` frame from the hub or kernel; `socket.closeReason` versus
   `error.localizedDescription` is already the switch.
5. **One more pattern in `inPlainWords`:** `has no kernel serving "x"` →
   "x's kernel on arboslife isn't running." The other two (`no machine
   named`, `no project named`) are the model of how these should read; a
   third arrived with #417 and shows raw today.

## What the hub now guarantees, so the client can rely on it

Every refusal the hub makes is an `error` frame with a reason, followed by a
close that waits for the peer (2 s ceiling), at every site including no or
unknown token (#459). Anything that arrives with **no** `error` frame — an
HTTP status, a bare close, a timeout — is the transport, not a verdict, and
should be retried and named as such.
