---
cursor:
  subagentId: "bc-2f70c55c-890b-5415-9e85-ea2e828f5078"
---

# Affine king on Jacob's Mac

2026-09-22. No tokens or secrets in this file. Did not resume paused loops. Did not wake leftover iOS `bc-08d8261b`. Did not re-offer `hub.toml`.

## 1. H100 answers

Endpoint: `http://107.170.16.157:20000/v1`

`GET /v1/models` → 200. One model:

- id: `affine-king`
- owned_by: `sglang`
- `max_model_len`: 262144

`POST /v1/chat/completions` with `{"model":"affine-king","messages":[{"role":"user","content":"Say only the word pong."}],"max_tokens":256}` → 200. Assistant content: `pong`. (A 32-token cap spends the whole budget on `reasoning_content` and returns empty `content`. Give it room.)

No API key required. A dummy Bearer also works.

Cursor self-hosted Mac worker: not used. Hub path worked.

## 2. Reached the Mac through the Arbos hub

`hub.toml` was already on the Mac (`machine = "mac"`). Used the existing hub URL from the vault item's `arboslife-hub-url` field (the `hub-api.arbos.life` CNAME is still the other FastAPI app; `/list` there is 404).

`GET /list` roster:

- `mac` **online**, worker=false, macos aarch64
- kernels: `.arbos` live, `Clawdbot` live
- build: `0.2.0` `aed86a0b9d97` (Mac **2408**, 2026-09-20T11:39Z)
- user: `const`

Attached as owner to `mac/.arbos` and `mac/Clawdbot`. Arbos desktop was already running: PID 44314, `/Applications/Arbos.app`. Did not open a second copy.

## 3. Settings changed on the Mac

File: `~/.config/arbos/config.toml` (the same file Settings › Model writes).

Changed only these three keys. Everything else, including `api_key`, left as it was.

| key | before | after (now) |
| --- | --- | --- |
| `provider` | `openrouter` | `custom` |
| `api_base` | (OpenRouter default / empty) | `http://127.0.0.1:18765/v1` |
| `model` | `anthropic/claude-opus-5` (host default) | `affine-king` |

`api_base` first pointed at the H100 directly (`http://107.170.16.157:20000/v1`). That is what Jacob's screenshot shows failing. It now points at a loopback coalesce proxy on the Mac (see §5). The H100 is still the real model.

Kernel `provider` frame after the write: `provider=custom`, `model=affine-king`, `key=true`, `source=config`.

Root on `.arbos` still has its own chat model pin (`anthropic/claude-fable-5.1`) until he picks `affine-king` on that chat. The host default for new chats and `inherit` is now `affine-king`.

HTTP, not HTTPS. The existing saved key is sent as Bearer. The H100 does not need it. To avoid sending that key over cleartext, he can put any dummy 16+ character value in Settings › Model › API key, or we can add a “key optional for custom” path later.

## 4. How he picks the model

In the running Arbos desktop:

1. Gear → **Model** (or Settings › Model).
2. Provider: **Custom endpoint**.
3. Base URL field: `http://127.0.0.1:18765/v1` (already saved). That is the Mac loopback proxy, which forwards to the H100.
4. Default model: `affine-king` (already saved). That is what a chat uses until the composer chip picks another.
5. In a chat, the composer model chip on the right of the input. Search `affine-king` if the chip still shows an old OpenRouter id. The chip label may read **Affinee King** (the desktop's display name for `affine-king`). Custom has no suggested-model chips until he types; the live catalog is `affine-king` only.
6. On the failed **hello0** / hello chat: press **Retry**. The kernel now goes through the proxy. A new chat with Affine King selected also works.

To go back to OpenRouter: Provider → **OpenRouter**. The key is still in `config.toml`. Then pick an OpenRouter id on the chip.

## 5. Real Arbos turn

Jacob's screenshot (chat **hello0**, chip **Affinee King**) is the same 400: `affine-king rejected the request` / `window_tokens = 0…`. That was the kernel talking to the H100 **directly**. SGLang log: `System message must be at the beginning.` Arbos sends several leading system messages; that host allows one, at index 0.

Workaround on the Mac (no new app build):

- Script: `~/.config/arbos/affine-king-coalesce.py` (also stored at `.arbos/internal/affine-king-coalesce.py`)
- Listens on `127.0.0.1:18765`, joins leading system messages, forwards to `http://107.170.16.157:20000`
- LaunchAgent `com.arbos.affine-king-coalesce` (loaded, PPID 1) so it survives logout/reboot
- `healthz` → `ok`; `GET /v1/models` → `affine-king`

Proved through the hub on `mac/.arbos` / `root` with `model=affine-king`. Assistant line:

```
affine-king-ok
```

No tools. That is a real Arbos turn on the Mac using affine-king.

Earlier direct-H100 attempts (root, Hyperliquid chat, Clawdbot root, empty Clawdbot chat) all 400'd with the same notice. H100 from this VM: three system messages → 400; one joined system → `pong`.

## 6. Code change

PR: https://github.com/unarbos/arbos/pull/813  
Branch: `cursor/affine-king-system-coalesce-5078`  
Repo: `unarbos/arbos`

Non-OpenRouter bases now join the leading system messages into one. OpenRouter is left split (cache markers). Three unit tests. Live H100 check as above.

The Mac is still on **2408** / `aed86a0b`. The proxy is the live workaround until a build with this commit is on that machine. Did not replace the Mac binary. Did not start the update-bar loop. Once that build is in, `api_base` can go back to `http://107.170.16.157:20000/v1` and the LaunchAgent can be unloaded.

## Not done

- HTTPS in front of the H100.
- Dropping the loopback proxy (waits on #813 in a Mac build).
- Switching older pinned chats (`claude-fable-5.1`, `mercury-2.5`) over to `affine-king` — he does that on the composer chip.
