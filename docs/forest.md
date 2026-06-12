# The forest — running heads and joining nodes

> Design: ADR-0034. Wire contract: `docs/account-api-contract.md`. This doc
> is operational: how to run a head, join a node, and where the throwaway
> test forest lives.

A **forest** is a network of arbos nodes around a **head**: a self-hostable
registry + relay (`cmd/forest`). A node that joins gets an **ephemeral public
URL** through an outbound tunnel — no inbound ports, no DNS setup, works from
behind NAT. The node's own auth gate (cookie + one-time token, ADR-0034 P1)
guards the URL; the relay is transport, never a trust boundary.

## Join a forest (node side)

```sh
arbos --web 127.0.0.1:8420 --forest https://forest.constantinople.cloud
```

Startup prints the lease and a ready-to-click login URL:

```
arbos forest: joined as dusty-marten-cc6 — https://dusty-marten-cc6.forest.constantinople.cloud
arbos forest login: https://dusty-marten-cc6.forest.constantinople.cloud/login?token=...
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

## The first forest (default for remote testing)

The first forest head runs on the Nebius box and is the **default target for
testing arbos remotely** — prefer joining it over opening ports or
ssh-tunneling ad hoc:

- Head: `https://forest.constantinople.cloud` (apex shows active lease count)
- Leases: `https://<name>.forest.constantinople.cloud`, real TLS (Let's
  Encrypt wildcard via Cloudflare DNS-01; acme.sh renews and restarts the
  unit)
- Box: `const@204.12.163.231`, systemd units `forest-head.service` (:443,
  `CAP_NET_BIND_SERVICE`) and `arbos-node.service` (a resident test node);
  state + certs in `~/.config/arbos-forest/`; DNS in the Cloudflare zone
  `constantinople.cloud`
- Still a test forest: anonymous tier only, no PSL registration. Don't put
  anything load-bearing behind it.

To point a test at it from any machine:

```sh
arbos --web 127.0.0.1:8420 --forest https://forest.constantinople.cloud
```
