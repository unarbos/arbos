---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# K-16 Attach socket: configurable bind, token auth, WebSocket

Branch `cursor/attach-bind-token-b027` (based on `cursor/release-integration-52cd`). Feature agent note for QA before the PR.

## What it does

- `arbos-kernel serve <place> --bind 0.0.0.0:7001` (or `ARBOS_ATTACH_BIND`) opens the attach socket to the network. Default stays `127.0.0.1:0`.
- `<place>/.arbos/access.toml` names who may come in from off the machine:
  `[[client]] name = "phone", token_env = "ARBOS_PHONE_TOKEN" (or token = "…"), role = "writer"` (owner | writer | reader). `[[person]] email/role` rows parse but cannot log in yet (that is the hub's job later).
- Fail closed: a non-loopback bind with no `[[client]]` rows refuses to start.
- Who is trusted by address: **plain TCP from loopback only** (desktop, CLI). Everyone else — TCP from the network, and every WebSocket, including one from loopback (that is cloudflared) — must present a token: TCP as a first frame `{"type":"auth","token":"…"}` within 10 s; WebSocket as `Authorization: Bearer …` or `?token=…` on the upgrade, or the same first frame.
- The same port speaks WebSocket: an HTTP `GET` in the first bytes upgrades; each text message is one frame (a message with several lines is several frames). A plain client that stays silent for 250 ms is taken as TCP and greeted.
- `reader` may send only `history`; anything else gets an `error` frame and is dropped. `kernel.json` gains `bind`, `auth` (`loopback`|`token`), `ws`, `clients`; `url` stays a loopback address.

## Attack surface

- A silent TCP peer from the network: must be dropped after 10 s with `auth required`, and must not block other peers from being accepted (admission runs per connection).
- Wrong token, then right token on a new connection; token with trailing newline/space (trimmed on both sides); a 15-character token in the file (rejected at load with a message).
- `token_env` unset → the kernel refuses to start with the variable named.
- Several frames in one WebSocket message; a binary WebSocket message carrying JSON; a WebSocket ping (ignored); a client that sends `auth` twice (second one is a no-op).
- cloudflared on the kernel host pointing at the port with **no** `access.toml`: every tunnel visitor must get `auth failed: this kernel has no [[client]] tokens`; nothing else.
- `--bind 127.0.0.1:7001` (explicit loopback): no tokens needed, same as default; `--bind :7001` and `--bind 7001` mean `0.0.0.0`.
- The desktop and `arbos-kernel run/attach/answer` against a `--bind 0.0.0.0` kernel from the same machine: unchanged (they dial `url`, plain TCP, loopback).
- Reader role and the `history` frame from a reader while a turn streams: it should get `replayed` lines and the live frames (broadcast is not role-filtered — everything attached sees the transcript; only writes are gated). Say if that is wrong for the product.
- IPv6 bind (`[::]:7001`): `url` should say `[::1]:7001`.
