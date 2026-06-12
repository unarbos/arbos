# The forest — running heads and joining nodes

> Design: ADR-0034. Wire contract: `docs/account-api-contract.md`. This doc
> is operational: how to run a head, join a node, and where the production
> forest head lives.

A **forest** is a network of arbos nodes around a **head**: a self-hostable
registry + relay (`cmd/forest`). A node that joins gets an **ephemeral public
URL** through an outbound tunnel — no inbound ports, no DNS setup, works from
behind NAT. The node's own auth gate (cookie + one-time token, ADR-0034 P1)
guards the URL; the relay is transport, never a trust boundary.

## Join a forest (node side)

```sh
arbos web
```

`arbos web` binds loopback and joins the default head (`https://arbos.life`)
unless `--local` or `--forest <url>` is set. Startup prints the lease and a
ready-to-click login URL:

```
arbos forest: joined as dusty-marten-cc6 — https://dusty-marten-cc6.arbos.life
arbos forest login: https://dusty-marten-cc6.arbos.life/login?token=...
```

What happens underneath: the node generates/loads its device key
(`~/.config/arbos/identity/device.key`), registers (anonymous account,
idempotent on the key), mints a bearer token via signed challenge, heartbeats
(the heartbeat *is* the lease — assigned name, head-controlled TTL), and
serves its gateway handler back through a yamux-over-WebSocket tunnel.
Binding the web listener to loopback is fine — and recommended — when the
forest URL is the door: local access stays frictionless, remote access only
exists through the gated tunnel.

The name is ephemeral by design: stop the node (or stop heartbeating) and the
lease expires; the next launch gets a different name. Tokens are single-use —
each login rotates a fresh one into the node's log.

## Run your own head

```sh
forest --addr :8080 --domain my-forest.example:8080 --scheme http
```

- `--domain` is the public apex; leases are `<name>.<domain>`. Any wildcard
  DNS works; for a quick box with a public IP, sslip.io gives you
  `*.<ip-with-dashes>.sslip.io` for free.
- Devices persist (`--state`), so anonymous accounts survive head restarts.
  Leases don't — they re-establish on the next heartbeat.
- TLS: terminate in front (reverse proxy with a wildcard cert) and pass
  `--scheme https` so minted URLs match. The production path puts user nodes
  on a separate PSL-registered domain (ADR-0034).
- Lease pressure is tunable live via `--lease-ttl` and `--heartbeat`; nodes
  obey the response, not baked-in constants.

## Production forest head (`arbos.life`)

The default head runs on the Nebius Chakana box:

- Head: `https://arbos.life` (apex serves the install landing; `curl
  https://arbos.life` pipes the install script from GitHub main)
- Leases: `https://<name>.arbos.life`, wildcard TLS (Let's Encrypt via
  Cloudflare DNS-01; acme.sh renews and restarts the unit)
- Box: `const@204.12.163.231`, systemd unit `forest-head.service` (:443,
  `CAP_NET_BIND_SERVICE`); state + certs in `~/.config/arbos-forest/`; DNS
  in Cloudflare zone `arbos.life`
- Still anonymous tier only — no stable name claims yet. Don't put anything
  load-bearing behind it.

Install from anywhere:

```sh
curl -fsSL https://arbos.life | bash -s -- web
```

Point a test node at another head:

```sh
arbos web --forest https://forest.constantinople.cloud
```
