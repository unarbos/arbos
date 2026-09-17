---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# The model key belongs to the user: `provider` / `configure` frames, env and `op://` keys

Branch `cursor/remote-provider-key-b027` (on integration `a8cabfc`). Jacob hit "No API key for OpenRouter" on an ssh place.

1. **Kernel says what it has.** On every attach (after `hello`) and after any change, the kernel sends `provider {provider, model, key: bool, source}` — `source` is `config`, `env:VAR`, `secrets:NAME`, `memory`, or `none`; never the value. A turn refused for a missing key also broadcasts it.
2. **Desktop offers the user's key.** When the chat's kernel reports `key: false` and this desktop has a key of its own, a strip above the composer says "This machine has no OpenRouter key" with **Use my OpenRouter key on this machine** (and a "just for this session" variant). The click sends `configure {provider, api_base, model, api_key, remember}` over the already-authenticated attach connection. Owner role only (`Role::allows`): a `writer` token gets an `error` frame.
3. **Kernel takes it.** Builds the config from the frame (same code `spawn host=` uses to write a remote's config: `HostConfig::with_key`), writes `config.toml` 0600 via `Host::save` when `remember`, otherwise keeps it in memory (`arbos_core::host::set_override`) so it dies with the kernel; turns read `Host::load()` fresh, which honours the override; the key is registered with the secrets door (`MODEL_API_KEY`) so tool output redacts it. The frame is never logged: `frame_rejected` no longer prints the head of a line that carries `api_key`.
4. **Env and `op://`.** `OPENROUTER_API_KEY` (or the provider's `api_key_env`) in the kernel's environment already counted; now also `[secrets] OPENROUTER_API_KEY = "op://Arbos/<item>/credential"` (or `env:`/`file:`) in `<place>/.arbos/secrets.toml` is resolved when the config has no key — for servers with 1Password and `OP_SERVICE_ACCOUNT_TOKEN`.

## Attack surface

- `configure` from a `writer`/`reader` token → refused with an error frame; from loopback TCP (owner) → accepted.
- `remember = false` then a kernel restart → the key is gone and `provider {key: false}` returns; the desktop offers again.
- `configure` with an empty or 8-character key → refused ("that is not a key").
- The desktop's own key comes from `api_key_env` — the offer must resolve it before sending, never send the variable name.
- A remote kernel behind the tunnel with `access.toml` tokens: the phone (writer) cannot configure; Jacob's desktop (owner over the tunnel token) can.
- `secrets.toml` naming `op://` but no `op` binary / no `OP_SERVICE_ACCOUNT_TOKEN` on the server → the refusal notice says why; the desktop offer still appears.
- The key never appears in `kernel.log`, the transcript, tool output (`[REDACTED:MODEL_API_KEY]`), or the parity JSON dumps — grep them after a configure.
- `spawn host=` on a machine that already has a config with a key: unchanged (writes nothing).
