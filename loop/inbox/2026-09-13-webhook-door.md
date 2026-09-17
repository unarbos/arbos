---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# Webhook door — PR #110, branch `cursor/webhook-door-b027`

`POST http://<kernel>:<port>/hook/<agent>` → an inbox message for that agent (a `say` from `webhook:<client>`; the agent wakes). Same port as attach/WebSocket.

- Loopback: no token. Elsewhere: `Authorization: Bearer <token>` or `?token=` with a `[[client]]` row of role writer/owner in `.arbos/access.toml`. Reader → 403, none → 401, unknown agent/path → 404, empty body → 400, body over 256 KB → connection dropped (413 would be nicer; say if you want it).
- JSON bodies: `text` / `content` / `message` / `body` is the message, other fields appended as `[webhook fields] {...}`. Non-JSON: the raw body.

## Attack surface to try

- Slowloris: a POST that sends headers slowly (>10 s) or never finishes the body → dropped after 10 s, other clients unaffected (the read runs per connection off the accept loop).
- `Content-Length` larger than the body actually sent → waits up to 10 s per read, then delivers what came (truncated to the declared length or less). Check nothing hangs.
- `Content-Length: 300000` → refused before reading the body.
- A GET to `/hook/root` → it is treated as a WebSocket upgrade attempt and fails the handshake (no hook). Fine, but note it.
- Token in the query string ends up in the kernel log? It should not: refusals log the peer and agent only.
- Flood: 100 POSTs in a second → 100 inbox files, one turn per scan for the agent; the rest wait as files (nothing lost).
- Chunked transfer encoding (no Content-Length) → body read as empty → 400. Slack and Discord send Content-Length; curl too. Note if a real sender chunks.
