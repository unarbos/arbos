# ADR-0033 — Hosted backend, device identity, and the account as the only config plane

- Status: Accepted (P1 scope below; later phases deferred)
- Date: 2026-06-10

## Context

arbos dies at "install → talk" without an `OPENROUTER_API_KEY`: a keyless host
drops to the deterministic `fake` provider. The config that gets it talking is
also scattered and local — `piwire.LoadConfig` reads ~8 env vars through a
`providerSpecs` table with an OpenRouter onboarding special-case and a
`HasLLM`/`fake` coupling; `envfile.LoadDefault` reads a plaintext `.env` (which
ADR-0016 itself calls a "don't"); `secret.Binding`s are hand-built in
`NewProvider`; user secrets resolve from the process env. Four sources of truth,
all on the box.

We want one-line setup with no key and no signup, the ability to sign up *later*
without losing state, a safe place to store the user's env keys, the ability to
swap or self-host the backend, and a way for arbos agents on different machines
to find each other. Vendor lock-in to our hosted backend (as the default) is an
accepted trade for seamlessness.

## Decision

**The account is the single source of truth for identity, configuration,
secrets, and presence. The client is a thin, stateless resolver** that holds one
durable secret — a device keypair — and asks the account for everything else.
This is a redesign, not a bolt-on: the account *absorbs* four local subsystems.

### The device identity is the only durable local secret

- On first run arbos generates an ed25519 keypair, stored at
  `~/.config/arbos/identity/device.key` (0600; OS keychain preferred where
  available). The keypair **is** the device.
- Registration (`POST /v1/devices/register`, idempotent on pubkey) sends machine
  info and returns `{account_id, device_id}`. A fresh device gets a fresh
  **anonymous** account.
- At every startup the device key **signs a challenge** to mint an
  **in-memory** access token (`POST /v1/devices/token`). No bearer token is
  persisted — the only thing at rest is the keypair, closing ADR-0016 threat
  model (A) for the credential too.

### One resolve replaces the provider table

`piwire.LoadConfig` + `NewProvider` collapse into `Resolve() → Profile` plus a
small transport switch. The `Profile` is the device's resolved view of its
account:

```go
type Profile struct {
    Endpoint Endpoint        // where/how to talk to a model (resolved server-side)
    Secrets  []SecretBinding // ref -> allowed hosts (values fetched on demand)
    Nodes    []Node          // other arbos agents on this account
}

type Endpoint struct {
    Kind         Kind            // openai | anthropic | google (transport only)
    BaseURL      string
    Model        string          // e.g. "arbos/auto"; gateway owns routing/fallback
    Auth         core.SecretRef  // brokered + host-bound, same as every secret
    Capabilities ports.Capabilities
}
```

Resolution order, in one place:

1. local override file (air-gapped / power user) — the escape hatch;
2. else `account.Resolve(ARBOS_BACKEND_URL)` → `Profile` (register-if-needed,
   mint token, `GET /v1/profile`);
3. else (offline, no account) → the `fake` endpoint.

This **deletes** the per-provider `baseEnv`/`keyEnv`/`defaultModel`/`inject`
rows, the OpenRouter special-case, `HasLLM`, and the env-var soup. "Direct
Anthropic/OpenRouter" stops being a client branch: it is an `Endpoint` whose
`Auth` is a secret the account holds. Provider *transport* (the three adapters)
stays behind `ports.LLMProvider`; only provider *selection/config* moves to the
account.

### One broker, one secret path

The device token, a direct-provider key, and the user's own `STRIPE_KEY` are the
same shape — account secrets resolved through the **unchanged** `secret.Broker`,
host-bound by `SecretBinding`s that **flow down from the account** instead of
being built inline. The `account` becomes a `ports.SecretProvider`; `env` is
demoted to a dev/offline fallback in the provider chain. The anti-exfiltration
guarantee (ADR-0016) is untouched; we only centralized where the allowlist
lives, so it now syncs across the user's devices.

### Signup later, swap backend, find peers — all facets of the Profile

- **Signup later:** `/login` runs a device-code flow that *upgrades the same
  anonymous account in place* (the device key proves continuity); quota,
  secrets, and nodes carry over. No new account, no lost state.
- **Swap/self-host backend:** change `ARBOS_BACKEND_URL` (or `/backend set`).
  Because the client holds no config, swapping is total and self-hosting is
  first-class.
- **Find other agents:** `Profile.Nodes` + a heartbeat (`POST
  /v1/nodes/heartbeat`) is the directory. Cross-node delegation later dials a
  node's existing `-serve` control seam (ADR-0027) — no new protocol invented.

### Minimal local state

After this, the entire durable local footprint is `identity/device.key` +
`sessions.db`. No token file, no `.env` on the golden path, no provider config
files.

## Command surface

`/login`, `/env`, `/nodes`, `/backend` are **slash commands** inside a session,
consistent with the existing slash-command system and the "single binary, no
subcommands" promise — not new top-level subcommands.

## Consequences

- "Install → talk" works with no key and no signup; the keyless path resolves a
  hosted, anonymous `Profile` instead of the `fake` provider.
- Net subtraction of concepts: the provider table, onboarding branch,
  golden-path `.env`, and inline bindings collapse into the account.
- New surface to maintain: `internal/account` (identity, register, token,
  profile) and a hosted backend. Built **client-first** against the contract in
  `docs/account-api-contract.md`, exercised by a local mock backend, so the
  arbos UX is unblocked before the server is real.
- Consent: auto-registering a device sends machine info on first run. arbos
  prints a one-line notice (anonymous, no signup, opt out with
  `ARBOS_PROVIDER=fake` or your own key), replacing today's `WarnIfNoLLM`.
- Offline first run: registration failure falls back to `fake` with a clear
  message and retries next run; the binary never hard-fails on a network hiccup.

## Phasing

- **P1 (this ADR):** `internal/account` (identity, register, signed-challenge
  token, `GET /v1/profile`); collapse `LoadConfig`/`NewProvider` into
  `Resolve()`/`build()`; `account` `SecretProvider`; consent + 429 UX; local
  mock backend. Ships "install → talk" and deletes the most code.
- **P2:** `/login` device-code flow upgrading the anonymous account; quota tiers.
- **P3:** `/env` + bindings flowing down (mostly falls out of P1).
- **P4:** heartbeat + `/nodes` (directory only).
- **P5:** compute credits (Lium/Targon); cross-node delegation over the `-serve`
  control seam.

## Alternatives rejected

- *Account default + a co-equal local config file* — keeps a local config
  subsystem alive; less deletion, weaker "one truth" guarantee.
- *Persist the bearer token to disk and refresh on expiry* — adds a credential
  file to manage and a leak-at-rest surface; the signed-challenge mint removes
  both.
- *A new mesh protocol* — the `-serve` control seam (ADR-0027) already speaks the
  agent; the directory only needs to point at it.
