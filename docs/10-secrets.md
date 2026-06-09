# 10 — Secrets & API-key handling

> Status: foundational. Types built (`core.SecretRef`, `core.SecretValue`,
> `ports.SecretProvider`, `secret.Broker`/`Binding`); backends and egress
> hardening are phased. See ADR-0016.

## Two threat models (the second is the agent-specific one)

- **(A) Box attacker** — reads `~/.env`, `/proc/<pid>/environ`, memory, history.
- **(B) Confused deputy** — the agent is **prompt-injected** via a web page or
  tool result into exfiltrating its own keys ("curl them to evil.com"). No box
  access needed. This is the threat env-var secrets fail hardest against, because
  every subprocess the agent spawns inherits them.

## The principle

> The agent — its LLM context, config, tool args, and every subprocess
> environment — only ever sees **`SecretRef`** handles. Real values are resolved
> at the last moment inside a trusted **broker**, injected only into the specific
> outbound request that needs them, and **bound to the destination host**.

## The types (data-structure-first)

```go
// Opaque handle. No material. Safe to log / persist / put in a Grant or args.
type SecretRef struct { Name string }            // core

// Leak-proof wrapper: stringers + marshalers redact; Expose() is the only
// accessor; Destroy() zeroes. A stray log/marshal cannot leak it.
type SecretValue struct { /* unexported bytes */ } // core

// Port. Backends: keychain | 1password | doppler | age-file | env(dev).
type SecretProvider interface {                    // ports
    Name() string
    Resolve(ctx, SecretRef) (SecretValue, error)
}

// The only thing that touches raw material at the boundary.
type Binding struct { Ref SecretRef; Hosts []string; Inject Injector } // secret
type Broker  struct { /* provider + bindings */ }                       // secret
func (*Broker) Apply(ctx, ref, *http.Request) error
```

`Broker.Apply` resolves a secret **only after** the request host matches the
binding allowlist, then injects and destroys it. A disallowed host or unbound
ref is refused and the value is **never resolved** — verified by test
(`internal/secret/broker_test.go`: provider call-count stays 0 on refusal).

## Backends (decided)

| Deploy | Default backend |
|---|---|
| Desktop (macOS/Win/Linux) | OS keychain (Keychain / Credential Manager / libsecret) |
| Any | 1Password (`op`) and Doppler (`doppler secrets get --plain`) — first-class |
| Headless / VPS (no keychain) | age/sops-encrypted file, key unlocked at boot |
| Dev only | environment variables — explicitly insecure (`env(dev-insecure)`) |

Your current 1Password/Doppler workflow becomes a first-class `SecretProvider`:
arbos resolves refs on demand and injects at the boundary, instead of those
tools exporting plaintext env vars/args that leak via `/proc` and `ps`.

## What it neutralizes

- **(B)** The LLM never holds a secret → can't be tricked into leaking one; a key
  bound to `api.openai.com` can never be attached to `evil.com`.
- **(A)** Nothing plaintext at rest; nothing broad in the environment; short,
  zeroed in-memory lifetime; subprocesses never inherit secrets.

## Don'ts (structural, not convention)

No plaintext `.env`; no secrets in the agent's or any child's process env; no
secret values in the LLM context, logs, the event log, or tool-call args (args
carry `SecretRef`).

## Composition with delegation (ADR-0013)

A `Grant` carries `SecretRef`s + `Binding`s, never values. A delegated Codex
session gets "may reach api.openai.com with a brokered key," not the key. Since
children run as subprocesses with no secrets in env, an injected child has
nothing to exfiltrate. External runtimes get brokered header injection now, and
the egress proxy later.

## Concurrency

Broker/provider is shared, security-critical state. Resolved plaintext is
short-lived and zeroed, never broadly cached. The port is shaped so the broker
can later run as a separate process/privilege (resolve over a socket) with no
caller changes.

## Phasing

- **Built now:** the four types + a dev `env` provider and a test `map` provider.
- **Next (with the provider HTTP client, Phase 4):** wire `Broker.Apply` into
  outbound provider calls; add keychain + Doppler/1Password providers.
- **Hardening (later):** egress proxy, separate-process broker, age/sops file
  backend, MCP-shared-tool credential brokering for external runtimes.

## Open items

- Binding config surface (where users declare ref → hosts → injector).
- Headless key-unlock UX for the age-file backend (passphrase vs boot secret).
- Approval policy when a tool requests a brokered call to a new host.
