# ADR-0035 — The agent is (dir, machine): two-key identity, derived URLs, and the router/relay split

- Status: Proposed
- Date: 2026-06-13

## Context

ADR-0033 made one global device key *the* device and *the* account. ADR-0034
P3 gave that device a forest URL, but on the anonymous tier the name is
**random** (`mintName` → `dusty-marten-cc6`), re-minted every launch and
forgotten on silence; a stable, chosen name is reserved for the *linked* tier
(`/v1/nodes/claim`).

The unit people actually run is `arbos` in a **project directory**. Today
identity is per-machine and the URL is random and disposable, so "come back to
this project tomorrow, get the same agent at the same URL" does not exist, and
"no signup" also means "no durable handle." We want the opposite: `arbos .` in
a directory → instantly a working URL, the *same* one every time you return,
with no login — and a clean path to fund and scale it later without a second
identity system.

## Decision

**An agent is a (directory, machine) pair. Its identity is a composite of two
keys; its public URL is derived from that identity; the LLM router and the
public relay are independent choices.**

### 1. Two keys — only one of them is a secret

- **Device key (machine)** — unchanged: `~/.config/arbos/identity/device.key`.
  The sole machine secret and the sole *signer*.
- **Dir key (directory)** — new: a keypair under `.arbos/`, committed with the
  repo. It is a **public binding input, not a secret**.
- `AgentID = H(devicePub ‖ dirPub)`; the public URL is derived from `AgentID`.

Claiming and serving an `AgentID`'s URL requires a **device-key signature**
(the existing challenge flow, ADR-0033). The dir key's only job is to make the
URL deterministic and per-project *without putting a secret in the repo*. The
honest property: the thing you commit (dir key) is safe to leak; the thing
that must stay on the box (device key) never enters the repo. This is not a
second signing factor — see the security section for what device-key theft
still costs.

### 2. A derived URL is stable-while-running — and free

For this path the derived name replaces random `mintName()`. The URL string is
**permanent** for a given (dir, machine): same dir → same URL, every run, no
login. "Ephemeral" stops meaning "you lose the name" (ADR-0034) and starts
meaning **the URL only resolves while you heartbeat** — stop, and it 404s until
you return. No one else can ever take it (claiming needs your device key).

The routing subdomain is a **truncated hash** of `AgentID` (unguessable,
collision-resistant). A speakable `adjective-animal` label is derived
deterministically from the same hash for *display only*, never as the routing
key — so the ~9M speakable pool can't be squatted on the shared domain.

This deliberately **gives the anonymous tier the stable URL** that ADR-0034
reserved for linked accounts. That is the point; it reshapes the paid tier (see
below).

### 3. Router ⟂ relay

Two orthogonal axes, never coupled again:

- **Router** (LLM egress): OpenRouter (BYO key) *or* the arbos.life gateway
  (`arbos/auto`). This is the `Endpoint` (ADR-0033).
- **Relay** (public ingress URL): the lease at a forest head.

Choosing OpenRouter for LLM does **not** mean "no arbos.life" — you still
register the device and hold a lease for the URL. The account is the relay
identity; the router is a per-account `Endpoint`. Every cell of the 2×2
(OpenRouter | gateway) × (anonymous URL | funded URL) is a valid product.

### 4. One account per device; agents are sub-identities; one login per machine

ADR-0033 stands: the device key owns one account. An agent (a dir under that
device) is a **presence/lease under the account, not its own account**. Linking
(ADR-0034 P2 `/login`) attaches a human identity to the account once per
machine, and every agent under it becomes linked-tier. Per-agent billing
attribution, when wanted, is a **scoped, host-bound, short-lived** token naming
the agent — not a pasteable long-lived key, because the agent can read its own
environment (ADR-0016).

### 5. Concurrency: one live agent per (dir, machine)

Two `arbos` processes in the same dir resolve to the same `AgentID` and would
fight over one lease (last-heartbeat-wins, `head.go`). A lockfile in `.arbos/`
makes the second invocation refuse ("already running — open `<url>`") or attach
as a viewer. "More than one agent on a machine" means more than one *directory*.

### What linking sells now

Not a stable URL — derivation gives that away for free. Linking sells:
funded/metered LLM, a higher cap on concurrent live agents, a **chosen vanity
name**, an offline parking page instead of a 404, and multi-device account
sync. The identity gate sits on bandwidth, funded compute, and vanity names —
all expensive to mass-produce — not on the bare URL.

### 6. Sybil defense: meter scarcity, not the keypair

A fresh device key is `rand.Read` of 32 bytes — free. `handleRegister` mints a
fresh anonymous account per new pubkey, so the abuse loop is: mint key →
register → drain the free `arbos/auto` gateway → discard → repeat. The
per-device live-lease cap (§2) bounds *URLs* per key, not *keys*, so the
resource to defend is the **gateway's free quota**, and it must be metered by
something scarcer than a keypair.

We do **not** defend by fingerprinting the box. Every client-reported hardware
signal (`machine-id`, MAC, CPU, disk serial) is spoofable for free by an
attacker who controls the box, collides for honest users (VMs, containers,
cloned cloud images, CI), and widens the consent surface (ADR-0033) — the worst
cost/benefit of any layer. Instead, in increasing cost to the attacker:

1. **Server-observed source signal (primary).** The head already sees the
   connecting IP. Meter free quota per **IP and per /24 / ASN**, not per key,
   and give **datacenter/cloud ASNs** a tighter cap than residential — Sybil
   farms live on cheap VPS IPs, and IPs cost real money at scale while keypairs
   do not. Record a registration context `{pubkey, first-seen IP, ASN,
   created-at}` and key the free meter on the scarcest available bucket, with
   the device key as the finest grain.
2. **Thin, graduated free tier.** A brand-new anonymous key gets a *small*
   allowance — enough for the magic moment (§2), not enough to farm — and trust
   (more quota, more concurrent agents) accrues with account age and good
   behavior. A freshly spun key is nearly worthless until it ages, so spinning
   thousands buys little.
3. **Proof-of-work on register/token mint (optional, server-tunable).** Make
   minting cost CPU: negligible for one honest device, expensive at farm scale.
   Like lease TTL, the difficulty rides server-side so it can be turned up under
   pressure without shipping node binaries.
4. **The linked tier is the real wall.** Per ADR-0034: GitHub/Google/payment
   accounts are expensive to mass-produce and bannable; keypairs are neither.
   Sybil resistance and free-tier generosity are the same dial — keep the
   anonymous tier deliberately thin and route real volume through linking.

A coarse box signal may feed layer 1 as a **soft clustering hint only** — e.g.
a hashed `machine-id` to collapse many keys from one install into a single
rate-limit bucket — never as a hard identity or gate, and only if it stays in
the first-run consent notice. Abuse response is a **shadow throttle** (degrade
quietly) rather than a hard block where possible: a hard `429` tells the
attacker exactly which signal tripped and what to rotate.

## The device key is the only secret, and it is extractable

Stated plainly so the posture is a choice, not an accident. The seed is
plaintext at 0600; `0600` stops *other* Unix users, nothing running **as you** —
backups and sync carry it off, and the arbos agent itself has shell and file
tools as the same user. Mitigations, in priority order:

1. **Hide it from the agent's own tools** — denylist `~/.config/arbos/` and
   `.arbos/` in the file/shell tools. Near-zero cost; closes the most likely
   exfiltration route (a prompt-injected agent reading its own key).
2. **Hardware-back where available** — keychain/Secure Enclave (macOS),
   TPM/keyring (Linux) — so the seed is non-extractable and a leak can't be
   *reused* after access ends. ADR-0033 already says "OS keychain preferred";
   this is where it earns its keep.
3. **Keep the gateway one-time-token + cookie gate (ADR-0034 P1) as the real
   authority for the URL** — non-extractability defeats exfil-and-reuse, not a
   live local attacker; the gate bounds remote access regardless of key custody.

Not file encryption whose unlock material lives on the same disk — that is
theater against a same-user attacker.

## Command surface

- `arbos .` (or any existing path, or an `http(s)://` URL) → web for that
  directory.
- `arbos` (bare) or `arbos <prompt>` → TUI. Disambiguation: resolves to an
  existing path → web; parses as a URL → web/connect; else → TUI prompt.
- `.arbos/` is the per-directory home: the dir key, lockfile, caches, and the
  agent's **brain** (`sessions.db` — sessions, mind/atoms, plans, callbacks).
  Two sibling directories are two separate minds; merging brains across
  directories is a future capability, not a default.

Consistent with ADR-0033/0034's no-subcommands promise: still flags plus a
positional, no `arbos join` / `arbos serve`.

## Consequences

- The headline UX exists: `arbos .` → working URL, the same one every return,
  no signup.
- Net subtraction: the random-rename machinery and the anonymous-vs-linked
  *name* split (ADR-0034) collapse — names are derived; the tier line moves to
  funding, scale, and vanity.
- **Amends ADR-0033**: the durable local footprint gains a per-dir key, but it
  is **non-secret** — "the only durable secret" now reads "the only durable
  *secret* is the device key; the dir key is public." The account stays
  per-device. ADR-0033's `sessions.db` also moves *into* `.arbos/`: the brain is
  per-directory, while the device key (machine identity) and the managed-secret
  vault (account credentials) stay machine-level under `~/.config/arbos/`.
- **Amends ADR-0034**: the anonymous tier gets stable derived URLs;
  "ephemeral" is redefined as resolves-while-live; the per-device cap is on
  concurrent *live* leases, not distinct names. The head change is a count in
  `handleHeartbeat` plus derived-name routing.
- New surface: per-dir key load (`LoadOrCreateDirKey`), `AgentID` derivation,
  the lockfile, head support for derived names + live-lease caps, and the
  agent-tool denylist.
- Security posture is explicit: device-key theft is full compromise of that
  machine's agents (an attacker can claim any `AgentID` for a public dir key).
  We mitigate *extraction*, we do not assume a fully trusted device.
- Abuse posture is explicit: the free gateway quota is metered by server-
  observed scarcity (IP/ASN) and a thin graduated tier, **not** by box
  fingerprinting; the linked tier is the real Sybil wall. New head surface: a
  registration context record + per-bucket free-quota meter.

## Phasing

- **P1:** per-dir key + `AgentID` + derived (hash) URL on the existing forest
  head; `arbos .` web mode; the lockfile. No login, no caps — the magic moment.
- **P2:** speakable derived label; concurrent-live-lease cap per device, with
  the existing `429 quota_exhausted` upsell extended to the cap. Sybil layer 1:
  per-IP/ASN free-quota meter + a thin graduated allowance; datacenter ASNs
  capped tighter.
- **P3:** linked-tier perks (vanity name, higher cap, offline parking) on the
  ADR-0034 P2 link flow; per-agent scoped tokens for billing attribution.
- **P4:** device-key hardening — agent-tool denylist first, then keychain/TPM
  backing.

## Alternatives rejected

- *Portable identity (device key travels with the repo)* — gives "same dir, any
  machine, same URL," but reintroduces a single carry-off secret in the repo
  and concurrent-claim flapping. A per-machine device key keeps the secret on
  one box.
- *Per-dir account* — granular billing, but N logins per machine and it breaks
  ADR-0033's one-device-one-account. A scoped token under one account gives the
  attribution without the extra accounts.
- *Encrypt the device key at rest* — theater when the unlock material is on the
  same disk and the agent runs as the same user; hiding it from tools or
  hardware-backing it is the real control.
- *Keep random ephemeral names for the anonymous tier* — protects the shared
  domain, but defeats the entire "same dir, same URL" hook. Derived names are
  unguessable and unsquattable by construction.
- *Couple router and URL (the original "route everything through arbos.life")* —
  one coupled choice instead of two independent ones; loses the ability to
  BYO-LLM while keeping an arbos.life URL.
- *Hardware fingerprinting as a hard Sybil gate* — spoofable for free by an
  attacker who controls the box, collides for honest users (VMs, containers,
  CI), and widens the consent surface. Server-observed scarcity and the linked
  tier carry the defense; a hashed box signal is at most a soft clustering hint.
