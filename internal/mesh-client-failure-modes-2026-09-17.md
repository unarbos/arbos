---
cursor:
  subagentId: "bc-22d20d79-de36-524a-ae31-3e1c44c03b98"
---

# What a client sees when the mesh is unavailable — and what it should

Mesh worker, 2026-09-17 13:50 UTC. Server side by doing (every hub case was
driven through the live tunnel today); client side by reading the shipped
code (`desktop/src/model/{session,workspace}.rs`, `desktop/src/view/detail.rs`,
`desktop/src/view/component/transcript.rs`; `ios/Arbos/Kernel/*.swift`,
`ios/Arbos/Chat/ChatStore.swift`), because I cannot drive Jacob's devices.
Where a line says "by code", the owner should confirm it on a screen.

## First, a correction that reshapes the question

**Jacob's desktop chat does not go through the hub.** Its remote path is
**ssh**: `desktop/src/kernel.rs` reaches a machine from `machines.toml`, puts
a kernel at `$HOME/.arbos-remote/bin/arbos-kernel` on the far side, runs
`arbos-kernel serve <place>` there and attaches over an ssh tunnel. The hub is
used by the **phone** (and by the desktop only for feedback reports). So at
13:20 I diagnosed his *Connection failed* by elimination on the hub side and
named `hub-api.arbos.life`; that was wrong, and it reached him. What the box
shows instead: `~/.arbos-remote/bin/` was created at **13:11**, the kernel
binary landed at **13:31** (build `cbbe9922`, 26.9 MB — a release cut, so the
desktop's new remote bootstrap), a kernel started serving **`/home/const`** at
13:32:01, and his desktop has probed it every 30 s since (44 attaches by
13:42, no turn yet). Nothing of his reached ArbosLife between 10:12 and 13:11
by any path; what failed in those three hours was the ssh remote start on his
Mac, whose reason is in the desktop's own log there, not on the server. The
most likely shape: the desktop build he ran at 10:12 wanted a kernel at the
new dotted path, found none, and treated "failed to start" as fatal
(`transient_connect_error`: no retry) — a dead tab with one notice — until an
updated app with the bootstrap ran at 13:11 and finished at 13:31. That is a
guess to be confirmed from his log, stated as one. (Confirmed from his
machine at 13:30, before this report: `ArbosLife:~` as an ssh remote, no
`hub.toml`, no `machines.toml`; the kernel placement failed because the app's
only route was the unpublished `v0.2.0` release, which 404s. The address story
was corrected to him then. The kernel serving `/home/const`, registered as
project `const`, is a side effect of that path and is left in place until he
says whether to narrow it.)

The hub cases below therefore describe **the phone** first-hand and the
desktop's *feedback* link; the desktop's *chat* has its own, ssh-shaped set,
listed after.

## The hub cases, as the wire shows them (observed today through the tunnel)

| Case | What the service sends | When | Phone (`ChatStore`, by code) | Desktop chat |
|---|---|---|---|---|
| Hub process down, tunnel up | cloudflared answers **HTTP 502** for the upgrade (origin refused) — *not exercised today; cloudflared's documented behaviour; I did not stop the hub a third time for a test* | at once | URLSession error text ("There was a bad response from the server.") → `dropped(said)` → shown as if a refusal, **and retries stop** (`refusal != nil` blocks `resumeIfNeeded`) — a transient failure treated as a verdict | n/a (ssh) |
| Tunnel down, hub healthy | named tunnel: Cloudflare edge **530 / error 1033**; a dead *quick* tunnel: the hostname stops answering (404/530 or DNS) — *not exercised* | at once | as above: the transport's sentence shown as a refusal, retries stop | n/a |
| Wrong or rotated token | **before #459: a bare close, no reason** (observed 13:20 through both tunnels); **after #459:** `{"type":"error","detail":"auth failed: unknown token"}` then a clean close | 0.1 s | bare close → `socket.closeReason` nil → "Socket is not connected" is *filtered out* → **waiting card + retry every 2–15 s for ever, never saying why**; with #459 the reason arrives and `inPlainWords` shows it verbatim ("Auth failed: unknown token") and retries stop — acceptable once the wording says what to do | n/a |
| Address is not the hub (his live case for `hub-api.arbos.life`) | **HTTP 404** `{"detail":"Not Found"}` from another service | 0.1 s | URLSession's "bad response" text shown as a refusal; retries stop; the 404 and the hostname never reach the screen | n/a |
| Machine registered, that kernel gone | `hub: arboslife has no kernel serving "x" (live: const, demo, feedback, …)` | 0.41 s | `inPlainWords` has words for `no machine named` and `no project named` but **not for `has no kernel serving`** → the raw sentence is shown; it is a good sentence, so acceptable — add the third pattern | n/a |
| Machine not registered at all | `hub: no machine named "x" is registered (known: …)` | 0.4 s | "x is not connected — its kernel isn't running, or the machine is off." — **the model of how this should read** | n/a |
| Project not shared | `no access to arboslife/subnet120: the project is not shared with you` | 0.16 s | shown verbatim; acceptable | n/a |

Two hub-side conclusions. First, every refusal the hub *makes* now arrives
with its reason (#344, #417, #459); the remaining silences are the
**transport's**: a 404 or 502 has no error frame, and the client sees only
URLSession's or tungstenite's sentence. The hub cannot fix those; the client
must name the HTTP status and the hostname it dialled — "hub-api.arbos.life
answered 404: that address is not an Arbos hub" is one line and would have
ended thirty hours of confusion in one glance. Second, the phone's
`refusal(from:)` treats any non-filtered error text as a refusal that stops
retrying; a 502 from a tunnel whose hub restarts in five seconds therefore
parks the phone until the user comes back to the app. Transport errors should
retry; hub refusals should stop. The distinction is "did an `error` frame
arrive" — the phone already has that (`closeReason` vs `localizedDescription`)
and only needs to use it as the switch.

## The desktop chat's own cases (ssh path, by code)

| Case | What the desktop shows | Retry |
|---|---|---|
| ssh unreachable / key refused / host down | notice `connection failed: <e>` **collapsed to "Connection failed."** by `short_error`; the reason is behind the Details disclosure | transient → 30 tries |
| No kernel binary on the far side (old path) | `failed to start` is in the fatal list → one notice, **no retry, tab dead** until Send or Reconnect | none |
| Remote bootstrap in progress (new build) | bar: `ArbosLife · reconnecting, try N in Ns`; a Send shows *Working* with the clock running; no line says "installing the kernel on ArbosLife (27 MB)" | as transient |
| Spawn lost the place lock (`place already served`) | retried silently; the winner is attached | transient |
| ssh tunnel drops mid-session | `Connection::Lost`, bar counts down, notice only on the 5th, 10th… attempt | transient |

**How long the desktop retries and whether it says why it stopped.** 30
tries; delays 2, 4, 8, 16, 32 s then 60 s each: **about 26 minutes**, then one
chat notice — *"connection lost; retries stopped — send a message or press
Reconnect to try again"* — and the bar reads `· connection lost`. So it does
say *that* it stopped and what to do, but not *why the connection fails*:
the reason is in the first `connection failed: …` notice, shown once, cut to
"Connection failed." on the visible line. His three hours were one 26-minute
loop that ended at about 10:38 with that notice, a dead tab until 13:11, then
a Send that started a fresh loop (the screenshot's *try 8*) while the
bootstrap ran.

## What is acceptable and what is not

Acceptable today: the hub's own refusals on the phone (`no machine named`,
`no access`, and with a third pattern `has no kernel serving`); the desktop's
"retries stopped — press Reconnect" line.

Not acceptable, and whose fix it is:

1. **Client, desktop:** `short_error` turning every `connection failed: <reason>`
   into "Connection failed." The reason must be the visible line, shortened,
   not hidden: "Connection failed: ssh to ArbosLife refused the key",
   "…: no kernel at ~/.arbos-remote on ArbosLife, installing (27 MB)…",
   "…: hub-api.arbos.life answered 404". One sentence per class, like
   `short_error` already does for provider errors.
2. **Client, desktop:** a *fatal* classification with no retry and one notice
   is how a tab goes dead for three hours. Fatal should still re-check at a
   slow cadence (every 60 s) *or* keep the reason on the bar permanently
   (`ArbosLife · no kernel binary — Reconnect to install`), never a bare
   `connection lost`.
3. **Client, phone:** transport errors (404, 502, DNS) are shown as
   refusals and stop the retry loop; refusals and transport errors need
   opposite handling and the phone already has the bit that tells them apart.
4. **Client, phone:** name the status and the host for HTTP failures; add
   `has no kernel serving` to `inPlainWords`.
5. **Hub:** nothing left; #459 was the last bare close. Wording is fine;
   `known:`/`live:` lists are what a person needs.

Filed for the desktop and phone owners:
`internal/features-inbox/2026-09-17-client-honesty-when-the-mesh-is-unavailable.md`.
