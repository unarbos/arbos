# ADR-0034 — Gateway auth, tunneling, and the forest

- Status: Accepted
- Date: 2026-06-11

## Context

The gateway (`internal/gateway`) has no auth. Anyone who can reach the bind
address gets chat, file read/write, secrets CRUD, PTY spawn, and job kill. The
only guard is a `Sec-Fetch-Site` header check, and the WebSocket handlers skip
origin verification because there is no cookie to ride. This is fine for
`localhost` and fatal for anything else — and we want `arbos web` on a remote
box, reached from a browser, gated by a real login.

Beyond one box, we want a network: `arbos --forest arbos.life` joins a node to
a directory, gives it a stable URL (`kitchen.arbos.life`), and lets its owner
log in from anywhere — behind NAT, with no port forwarding. Later, arbos.life
is a service that provisions a VPS and a URL for you.

ADR-0033 already gives us the bones: a device ed25519 keypair as node
identity, an account backend, a `/login` link flow for human identity
(GitHub/Google), and a node directory. `Session.Principal` (ADR-0019) is
reserved and unused. What's missing is the door (gateway auth), the wire (a
tunnel), and the name for how they compose (the forest).

## Decision

**The forest is ADR-0033's account + node directory, given a name and a wire.**
The head of the forest is the account backend. A tree is a device. Joining the
forest is device registration + a subdomain claim + a persistent tunnel. There
is no new identity system; one identity plane is used three ways.

### Two identities, bound by the account

1. **Node identity** — the device keypair (ADR-0033). The node proves "I am
   kitchen" to the forest head.
2. **Human identity** — GitHub et al., via the backend's link flow (ADR-0033
   P2). The human proves "I am const" to the forest head.
3. **The account binds them.** Gateway authorization is one question: *does
   the browser's human identity belong to the account that owns this device?*

A node never implements OAuth itself. No GitHub app, client secret, or
callback URL on a random VPS — the forest head is the OAuth broker, and the
node verifies a short-lived assertion signed by the head against a **pinned
backend public key** (so the check works even when the node can't reach the
backend at request time).

### Layer 1 — gateway auth, standalone

An `Authenticator` seam in front of `Server.Handler()`:

- **Cookie session**: HMAC-signed, httpOnly, SameSite=Lax; key generated and
  stored next to `identity/device.key`. WebSockets ride the cookie, which
  lets us turn origin verification back on and retire the `Sec-Fetch-Site`
  hack.
- **Loopback default**: binding a loopback address keeps today's zero-friction
  behavior (auto-principal `local`). Binding anything else turns auth **on,
  not optional** — there is no flag to forget.
- **One-time-token login (the v0 credential and the forever escape hatch)**:
  startup prints `http://host:8420/login?token=<one-time>`; visiting it sets
  the cookie. Single-use, expiring, rate-limited. This alone makes "open port
  on a VPN" safe with zero external dependencies, and it remains the
  air-gapped path when there is no backend.
- The authenticated principal flows into `Session.Principal`.

This layer has no backend dependency and ships standalone value first.

### Layer 2 — GitHub login, brokered by the forest head

When the device is registered (ADR-0033 P1) and the account is linked (P2):

1. Browser hits the gateway → login page → redirect to
   `https://<backend>/auth?device=dev_...&redirect=...`.
2. Backend runs GitHub OAuth (it owns the one OAuth app), checks the GitHub
   user is linked to the account owning that device, and redirects back with
   a short-lived signed assertion.
3. Gateway verifies the assertion against the pinned backend pubkey and sets
   its own session cookie.

**The auth decision is enforced at the node, never at the relay.** The relay
is transport, not a trust boundary.

### Layer 3 — the tunnel (our own relay)

The node dials **outbound** to the relay (`wss://relay.<forest>`),
authenticating with its device key, and keeps one persistent connection
multiplexed (yamux or HTTP/2 streams). The relay terminates TLS with a
wildcard cert (`*.arbos.life`) and proxies HTTP + WebSocket frames for
`kitchen.arbos.life` down the tunnel to the local gateway listener.

- Outbound-only: works behind NAT and firewalls with no port forwarding.
- Direct access (localhost, VPN, open port) keeps working without the relay;
  the tunnel is an additional front door with the same auth at the gateway.
- A compromised relay can deny service, not impersonate: the session cookie
  is set by the node and the login assertion is verified at the node.

We build the relay ourselves rather than embedding Tailscale: we own the URLs,
the SaaS path needs the relay anyway, and the per-node protocol is a few
hundred lines of Go on each side (the frp/chisel/cloudflared shape).

### Layer 4 — joining the forest

`arbos --forest arbos.life --web :8420` composes the layers: register device
(exists) → get a name → start the persistent tunnel + heartbeat → print the
public URL. The forest membership list *is* `Profile.Nodes`; `/nodes`
(ADR-0033 P4) is "show me my forest."

Names come in two tiers, priced by identity:

- **Anonymous device** (a bare keypair): the heartbeat *is* the lease. The
  head **assigns** a random name (`fuzzy-otter-3f2`) — never lets the device
  choose one, which is most of the phishing defense on a shared domain — and
  the lease expires a head-controlled TTL after the last heartbeat. Stop
  beating, lose the name; next launch gets a different one. Concurrent
  tunnel streams are hard-capped. Ephemerality is the abuse control:
  phishers can't build reputation on a URL that rotates, squatters can't
  hold names they aren't using, and cleanup is mostly "wait".
- **Linked account** (GitHub via ADR-0033 P2): may **claim** a stable, chosen
  subdomain (`POST /v1/nodes/claim {"name":"kitchen"}` — a lease tied to the
  account, not property), gets login-from-anywhere and real bandwidth. The
  URL and the bandwidth are where the identity gate sits, because GitHub
  accounts are expensive to mass-produce and bannable; keypairs are neither.

Lease TTL and heartbeat cadence ride in the heartbeat *response*, so abuse
pressure is tunable server-side without shipping node binaries.

**The production user-content domain must not be the product apex.** A
malicious node at `evil.arbos.life` could set `Domain=arbos.life` cookies
against the service itself, and one blocklisted subdomain poisons the shared
reputation. Like github.io and trycloudflare.com: user nodes live on a
sibling domain (e.g. `*.onarbos.net`) registered in the Public Suffix List;
`arbos.life` keeps the service and the forest head API. PSL propagation takes
weeks — file it before the public relay ships. (The throwaway test forest
runs on an sslip.io name and is exempt from all this.)

### Layer 5 — arbos.life the service

Backend/web work only: GitHub login (Layer 2 infra), provisioning = create a
VPS (cloud-init installs arbos and joins with a pre-minted device credential),
dashboard = the node directory rendered as UI. The node software needs nothing
new by this point — that is the test that the earlier layers were cut right.

## Command surface

`--forest <head>` is a **flag**, consistent with the existing flag surface and
ADR-0033's no-subcommands promise, because joining must work headless on a
fresh VPS before any session exists. `/join` is the in-session command. No
`arbos join` subcommand.

## Consequences

- `arbos web` on a non-loopback address is safe by default; localhost stays
  frictionless.
- Remote auth covers **every** route, including both WebSockets — the gateway
  exposes file write, PTY, and secrets CRUD, so anything less is decorative.
- New surface: gateway auth middleware + login routes; `internal/account`
  growth (assertion verify, claim, tunnel client); a relay service at the
  forest head; wildcard DNS + cert operations for the forest domain.
- The backend pubkey must ship pinned (and be rotatable) for offline assertion
  verification.
- Self-hosting the forest head stays first-class (ADR-0033): the relay and
  auth broker are part of the swappable backend, not coupled to arbos.life.

## Phasing

- **P1 (shipped):** gateway auth middleware, cookie sessions, one-time-token
  login, loopback default, WebSocket origin re-enabled. No backend
  dependency.
- **P2:** backend-brokered GitHub login at the gateway (needs ADR-0033 P1+P2).
- **P3 (shipped):** `internal/forest` + `cmd/forest`: self-hostable head
  (register, signed-challenge token, heartbeat-as-lease, relay) and the node
  client; `arbos --forest <url>` joins and serves the gateway through the
  tunnel, anonymous tier with assigned ephemeral names. A throwaway head is
  the default target for remote testing (see docs/forest.md).
- **P4:** stable chosen names for linked accounts (`/v1/nodes/claim`), the
  PSL'd user-content domain, TLS at the relay.
- **P5:** arbos.life provisioning + dashboard.

## Alternatives rejected

- *Embed Tailscale (tsnet)* — free NAT traversal and e2e crypto, but URLs and
  identity live in Tailscale's world; awkward for `*.arbos.life`, and the
  SaaS still needs our relay.
- *SSH reverse tunnel as v0* — cheap start, but a dead-end protocol we'd
  replace immediately; the own-relay client is small enough to build first.
- *Auth at the relay instead of the node* — makes the relay a trust boundary
  and breaks direct (VPN/localhost) access parity; defense in depth requires
  the node to decide.
- *`arbos join` subcommand* — contradicts ADR-0033's command-surface decision;
  a flag serves the headless case equally well.
- *Optional auth flag for remote binds* — a flag someone forgets is an open
  PTY on the internet; non-loopback means auth, period.
