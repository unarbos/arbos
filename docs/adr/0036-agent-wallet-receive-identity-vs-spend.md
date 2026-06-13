# ADR-0036 — Agent wallet: separate receive/identity from spend, scope the signer

- Status: Proposed
- Date: 2026-06-13

## Context

ADR-0033 and ADR-0035 give each agent keys for *identity* (the device key, the
dir key). Looking forward, agents will hold value: pay for their own
compute/LLM (x402-style per-call payments), carry credits, receive funds,
anchor an on-chain identity/attestation, and transact agent-to-agent across the
forest. We want every arbos to "have a wallet" by default and to be
future-proof about it.

The hard part is the one the design must answer head-on: a **spending** key has
to be unlocked to sign, and the agent runs as the same user with full file and
shell access (ADR-0035's security section). A plaintext spend key on disk is a
drainable bank account handed to a process that can be prompt-injected. So
"default wallet" cannot mean "default hot spend key."

## Decision

**A wallet is two capabilities with opposite risk. Default the safe one to
everyone; isolate and scope the dangerous one. Identity ≠ value, and the agent
never holds a raw spend key.**

### 1. Receive + identity is free and safe — default it

A public key can **receive** funds and sign **attestations** ("this agent said
X") with zero spend risk. Every account/agent gets an address by default. The
existing ed25519 device key is already a valid Solana/Sui-class address and an
attestation signer — no new secret, no unlock problem. An EVM/secp256k1 address
is derived from a dedicated wallet seed only when a use-case needs it.

"arbos defaults to having a wallet" = defaults to an **address + identity**,
which costs nothing and risks nothing. Funds can flow in and the agent is
addressable before any spend machinery exists.

### 2. Spend is the only dangerous capability — never give the agent a raw key

Spend authority is tiered by amount:

- **Micro (auto).** A hot **session key** with a hard cap, an expiry, and a
  destination allowlist. On disk (like the device key) is acceptable *because
  the blast radius is bounded* — leaking it loses the cap, not the treasury.
  This is the only thing the agent may use unattended.
- **Macro (gated).** The treasury never signs from a hot key. It sits behind
  one of: (a) a **hardware-backed signer** — Secure Enclave / passkey (P-256) —
  gated by an OS/biometric prompt per signature; (b) **backend custody** under
  policy (the account is already the source of truth, ADR-0033); or (c) a
  **smart account** (ERC-4337) whose owner is (a)/MPC and whose session keys are
  the micro tier. Custodial vs self-custodial is a **pluggable backend**, not
  baked in.

The resolution to "unlock-to-sign is insecure": don't unlock a powerful key —
unlock a capped one, and require a human/hardware gate for anything above the
cap. Frictionless for micro-spend, deliberate for real money.

### 3. Signing is mediated — reuse the broker

The agent never sees a wallet seed. It calls a **spend broker** that signs
specific, policy-checked intents — the same shape as the host-bound secret
broker (ADR-0016) and the gateway token. A spend intent is bounded by the
session key's policy; anything beyond it escalates to the gated tier instead of
silently signing.

### 4. One structure: account treasury, per-agent session keys

This mirrors ADR-0035 exactly. **Value is account-level** — one treasury,
funded once, owned by the device-account. Each **agent (dir, machine) gets a
scoped, capped, expiring, revocable session key / sub-budget** — the same "one
account, per-agent scoped credential" shape already chosen for LLM tokens.
Per-agent attribution and blast-radius control fall out for free; revoking a
compromised agent's key never touches the treasury.

### 5. Curves and chains — keep the door open

The identity key is ed25519. A spend wallet's curve follows its target chain
(secp256k1 for EVM/BTC, ed25519 for Solana). Derive chain keys from a single HD
seed (SLIP-0010 / BIP-32) so one protected root yields per-chain addresses; the
root gets macro-tier protection and never reaches the agent. Do not commit to a
chain now — receive/identity works today on ed25519; spend backends are added
when a payment use-case lands.

## Command surface

`/wallet` (show address, balance, fund link) is a slash command, consistent with
ADR-0033/0035's no-subcommands promise. Wallet and session-key state is
**account-level** (backend or `~/.config/arbos`), never per-dir — so a committed
`.arbos/` dir never carries spend authority.

## Consequences

- Every agent is addressable and can attest by default, at zero new risk.
- The agent can transact unattended only up to a capped session budget; the
  treasury is never exposed to a prompt-injected agent.
- New surface: a spend broker, session-key issuance + policy, a pluggable
  custody backend; optional smart-account contracts later.
- Extends ADR-0035's security section: the device key may double as an
  identity/receive address, but spend authority is a separate, scoped,
  higher-tier key.
- Explicit non-goals for v1: holding large balances in a hot key; self-custody
  of the treasury without hardware or MPC.

## Phasing

- **P1:** every account/agent gets an ed25519 address (receive + attestation);
  `/wallet` shows it. No spend. Zero risk — this alone ships the "default
  wallet."
- **P2:** capped session key + spend broker for micro-spend (the agent pays for
  its own LLM/compute, x402-style); account-level treasury via backend custody.
- **P3:** hardware-backed / passkey signer and/or smart-account + session keys
  for macro-spend and self-custody.
- **P4:** agent-to-agent payments across the forest.

## Alternatives rejected

- *Device key = the wallet* — merges identity and value into one plaintext,
  agent-readable key with irreversible loss on leak. The device key may receive
  and attest; it must not be a spend key for any non-trivial balance.
- *One hot wallet for everything* — drainable by anything running as the user,
  including a prompt-injected agent; exactly the threat ADR-0035 names.
- *Hardware key as the raw chain wallet* — Secure Enclave and passkeys are P-256
  only; ideal for authorization and as a smart-account owner, but not a native
  secp256k1/ed25519 chain key. Use it to *authorize*, not to *be* the key.
- *Commit to a custody model now* — premature and door-closing; a pluggable
  backend keeps custodial, MPC, and smart-account paths open until the payment
  use-case picks one.
