# Account backend — API contract

> Status: P1 surface. The client (`internal/account`) codes against this; a
> local mock backend implements it for development. See ADR-0033.

The backend is the account's home: identity/auth, an OpenAI-compatible LLM
gateway, a secret store, and a node directory. The client is a thin resolver —
it holds only a device keypair and resolves a `Profile` from here.

- Base URL: `ARBOS_BACKEND_URL` (default: the hosted backend; overridable for
  self-host/staging). Everything is HTTPS/JSON unless noted.
- Auth: most calls carry `Authorization: Bearer <access-token>`, an **ephemeral
  token minted per process start** (see `/v1/devices/token`). Registration and
  token minting authenticate with the **device key** (signatures), not a bearer.
- All signatures are ed25519 over the exact bytes named below, base64
  (std, no padding stripped) in the stated field.

## Identity & auth

### `POST /v1/devices/register` (idempotent on pubkey)

First run. A new pubkey gets a fresh **anonymous** account.

Request:

```json
{
  "public_key": "<base64 ed25519 pubkey>",
  "machine": { "hostname": "...", "os": "linux", "arch": "amd64", "arbos_version": "..." }
}
```

Response `200`:

```json
{ "account_id": "acct_...", "device_id": "dev_..." }
```

Re-registering a known pubkey returns its existing `{account_id, device_id}` (so
first-run is safe to retry, and a relinked device keeps its account).

### `POST /v1/devices/token`

Mint an ephemeral access token. No bearer; proves possession of the device key.

1. `GET /v1/devices/challenge?device_id=dev_...` → `{ "nonce": "<base64>", "expires_at": "<rfc3339>" }`
2. Client signs `nonce` bytes with the device key.

Request:

```json
{ "device_id": "dev_...", "nonce": "<base64>", "signature": "<base64 sig over nonce>" }
```

Response `200`:

```json
{ "access_token": "<opaque>", "token_type": "Bearer", "expires_in": 3600 }
```

The token lives in client memory only; it is never written to disk.

## Profile (the one resolve)

### `GET /v1/profile` — Bearer

The device's entire resolved config. The client builds its provider and broker
from this and holds nothing else.

Response `200`:

```json
{
  "endpoint": {
    "kind": "openai",
    "base_url": "https://gateway.arbos.example/v1",
    "model": "arbos/auto",
    "auth_ref": "arbos.device-token",
    "capabilities": { "vision": true, "reasoning": true, "tools": true }
  },
  "secrets": [
    { "ref": "arbos.device-token", "hosts": ["gateway.arbos.example"] }
  ],
  "nodes": [],
  "tier": "anonymous"
}
```

- `endpoint.kind` ∈ `openai | anthropic | google` — selects the transport
  adapter only.
- `endpoint.auth_ref` names a secret in `secrets[]`; the client resolves it via
  `GET /v1/secrets/{ref}` and the broker injects it, **host-bound** to that
  secret's `hosts`. For the hosted default this ref is the device token itself,
  bound to the gateway host (so a prompt-injected agent cannot send it
  elsewhere — ADR-0016).
- "Direct Anthropic/OpenRouter" is just a different `endpoint` whose `auth_ref`
  is a user-provided key the account stores.

## Secret store — Bearer

The user's env keys and the brokered credentials. Values are returned only over
TLS to an authenticated device and are never persisted client-side.

| Method | Path | Purpose |
|---|---|---|
| `GET` | `/v1/secrets` | list refs + bindings (no values): `[{ "ref": "...", "hosts": ["..."] }]` |
| `GET` | `/v1/secrets/{ref}` | resolve one value: `{ "value": "..." }` (broker-only consumer) |
| `PUT` | `/v1/secrets/{ref}` | set: `{ "value": "...", "hosts": ["..."] }` |
| `DELETE` | `/v1/secrets/{ref}` | remove |

The `account` `ports.SecretProvider` calls `GET /v1/secrets/{ref}` at the broker
boundary and `Destroy()`s the value after injection.

## LLM gateway — Bearer

### `POST /v1/chat/completions`

OpenAI-compatible Chat Completions (streaming SSE), so the existing
`internal/provider/openai` adapter targets it unchanged. The gateway owns
routing/fallback across free tiers (OpenRouter free, Chutes) behind the
`arbos/auto` model alias, meters usage, and on exhaustion returns:

```
HTTP 429
{ "error": { "type": "quota_exhausted", "message": "free tier used up — link your account for more: /login" } }
```

The client surfaces `message` as a notice (not a stack trace).

## Node directory — Bearer

| Method | Path | Body / result |
|---|---|---|
| `POST` | `/v1/nodes/heartbeat` | `{ "host","cwd","caps":["..."] }` → lease (below) |
| `GET` | `/v1/nodes` | `[Node...]` (also embedded in `GET /v1/profile`) |

For the anonymous tier the heartbeat **is** the lease (ADR-0034): the first
beat assigns a random name, every beat extends it, and silence expires it.
The response carries the head-controlled knobs so abuse pressure is tunable
server-side:

```json
{
  "name": "fuzzy-otter-3f2",
  "url": "https://fuzzy-otter-3f2.onarbos.net",
  "ttl_seconds": 3600,
  "heartbeat_seconds": 30
}
```

Cross-node delegation later dials a node's existing `-serve` control seam
(ADR-0027) directly. No new protocol.

## Forest: gateway login broker, subdomain claim, relay (ADR-0034)

### `GET /auth?device=dev_...&redirect=<url>` — browser flow

The forest head brokers human login for a node's gateway. Runs the identity
provider (GitHub et al.), checks the authenticated user is linked to the
account owning `device`, then redirects to `redirect` with a short-lived
**assertion**: a signed statement `{device_id, account_id, sub, exp}` over the
backend's signing key. The gateway verifies it against the **pinned backend
public key** (offline-capable) and sets its own session cookie. The relay
never participates in the auth decision.

### `GET /v1/keys` — no auth

Backend signing pubkey(s) for assertion verification, fetched once and pinned
client-side: `{ "keys": [{ "kid": "...", "public_key": "<base64 ed25519>" }] }`.

### `POST /v1/nodes/claim` — Bearer, linked accounts only (P4)

Claim a stable, *chosen* subdomain — the linked-account tier's upgrade over
the heartbeat's assigned ephemeral name. Anonymous accounts get `403
identity_required`.

```json
{ "name": "kitchen" }
```

Response `200`: `{ "host": "kitchen.onarbos.net" }`. `409` if taken by
another account; idempotent for the owner.

### Relay handshake — `wss://relay.<forest>/v1/tunnel`

The node dials outbound and authenticates with its device key (same
challenge/signature shape as `/v1/devices/token`). After the handshake the
connection is a multiplexed stream (yamux) carrying HTTP + WebSocket traffic
for the node's claimed host, reverse-proxied to the local gateway listener.
TLS terminates at the relay (`*.arbos.life` wildcard); auth terminates at the
node.

## Account linking (P2)

`/login` runs a device-code flow that **upgrades the current anonymous account
in place**:

1. `POST /v1/auth/link/start` (Bearer) → `{ "user_code": "ABCD-1234", "verification_url": "https://...", "device_code": "...", "interval": 5 }`
2. User opens the URL, authenticates with Google/GitHub/wallet.
3. Client polls `POST /v1/auth/link/poll` `{ "device_code": "..." }` → `200` once linked.

Because the device key already owns the anonymous account, linking attaches an
identity provider to it; usage, secrets, and nodes are preserved.

## Error envelope

Non-2xx responses use `{ "error": { "type": "...", "message": "..." } }`.
Recognized `type`s the client special-cases: `quota_exhausted` (429, notice),
`unauthorized` (401, re-mint token once then fail), `not_found` (404).
