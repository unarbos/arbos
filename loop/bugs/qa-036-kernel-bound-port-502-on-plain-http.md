# qa-036: the kernel's bound port answers a plain HTTP GET by closing the connection, so every tunnel/probe reports 502

- Feature: `arbos-kernel serve --bind` (WebSocket attach off loopback), phone kernel on the voice pod (`0.2.0` @ `6b62a0be3730`)
- Severity: low for users (the WebSocket path works), high for operations: the iOS loop, cloudflared and any uptime check see `502 Bad Gateway` and conclude the kernel is down while it is fine.
- Seen: 2026-09-15 ~14:00 UTC, `https://live-got-person-permits.trycloudflare.com` → 502; `kquick.log`: "Unable to reach the origin service … EOF" once a minute; `kernel.log`: `attach_refused … websocket handshake: No "Connection: upgrade" header` once a minute (the probes).

## Repro

`curl -i http://127.0.0.1:7788/` on the pod (or the tunnel URL): connection closed without a response → `000` locally, `502` through cloudflared. `wss://…/` handshake: the kernel answers at once (`auth required …`).

## Expected

A plain GET on the bound port gets a small HTTP reply — `200 {"kernel":"0.2.0","attach":"websocket","auth":"token"}` for `/` and `/healthz`, `426 Upgrade Required` for anything else — so tunnels, load balancers and the iOS loop's probe see a healthy origin, and a browser hitting the URL sees what it is.

## Actual

The handshake failure is logged and the socket is closed; cloudflared maps that to 502.

## Suspected location

`crates/arbos-kernel/src/serve.rs` `attach_open_bind` / the WebSocket accept path: on `No "Connection: upgrade"`, write a minimal HTTP/1.1 response before closing instead of dropping the socket.

## Fix

PR #234: a GET without `Upgrade: websocket` is `Conn::Http`; `/` and `/healthz` answer 200 `{"kernel","protocol","attach":"websocket","auth"}`, other paths 426 with an `Upgrade` header; no `attach_refused` for probes. E2e `port_health_e2e`.
