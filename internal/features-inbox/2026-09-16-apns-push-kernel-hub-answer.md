---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# APNs push: what the phone and Jacob need from the kernel/hub half

Re-placed after the store's second loss (2026-09-16), from the record on
[#301](https://github.com/unarbos/arbos/pull/301), whose body is the full
account. Only the parts another party still has to act on are here.

## iPhone worker

Register the device with the hub through the attach socket, before or after
`user` frames, **on every attach** (tokens rotate):

```json
{"type":"push","platform":"apns","token":"<hex>","project":"arboslife/phone","sandbox":false}
```

- `project` defaults to the address the socket attached to; `*` means every
  project of the token's user. The same token registering again replaces its
  row.
- The hub answers `{"type":"pushed","project":…,"enabled":…}`. `enabled:
  false` means the hub has no APNs key yet — keep registering; nothing is
  sent until Jacob configures it.
- A push carries `target` (`hub:<project>`), `id` (the notify id), `kind`,
  `agent`, and `aps.badge` = the unseen count. Open `userInfo.target` on tap.
  The replayed `notify`s on attach fill in whatever a push did not carry.
- A `seen` from any client becomes a background push (`content-available:
  1`) with the new badge, so the phone's badge follows the desktop.

## Checking it works (added 16:45 UTC, [#333](https://github.com/unarbos/arbos/pull/333))

- The hub stays up on a bad key; it logs `push disabled — <reason>` at
  start and serves.
- `GET /push` with any token: `enabled`, `reason`, your devices as token
  tails, and the last 50 attempts with Apple's status and detail.
- `GET /push/test` (or `/push/test/<token-tail>`): a test alert to your
  phone — 200 when it took it, 503 when push is off, 502 when Apple
  refused, with why in the body.
- The `pushed` reply to the app's `push` frame carries `reason` when
  push is off, for a plain line in the UI.

## Jacob

Into the hub host's `hub-server.toml`:

```toml
[push]
apns_key = "/etc/arbos/AuthKey_ABC123DEFG.p8"   # or apns_key_env = "APNS_KEY"
key_id = "ABC123DEFG"
team_id = "25SCF3Q2AK"
topic = "com.unarbos.arbos.ios"                  # default
sandbox = false                                  # default for tokens that do not say
```

The APNs key comes from Apple Developer → Keys (APNs enabled, download the
`.p8` once). The App ID needs the Push Notifications capability. The key
never leaves the hub host; kernels and phones do not see it.
