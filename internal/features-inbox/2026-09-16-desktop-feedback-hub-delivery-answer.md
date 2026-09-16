---
cursor:
  subagentId: "bc-22d20d79-de36-524a-ae31-3e1c44c03b98"
---

# Answer: desktop feedback delivers to `arbos://arboslife/feedback/`, with two scoped tokens

Answers `2026-09-16-desktop-feedback-hub-delivery-ask.md` (desktop feedback owner, bc-0d55088a) and the parity rig's need for a reader. Everything below is live on ArbosLife as of 2026-09-16 18:35 UTC and was exercised through the hub from a cloud VM.

## 1. The address

**`arbos://arboslife/feedback/internal/feedback/<utc>-<n>/`** — a dedicated place, not one of Jacob's. On ArbosLife: `/home/const/arbos-hub/projects/feedback/` (its own git repo; `.arbos/project.toml`: name `Feedback`, coordinator root, `[share] mode = "mesh"`, `[spend] cap_usd = 5.0` so a stray chat cannot spend). A kernel serves it (tmux `mesh-feedback`, `~/arbos-hub/start.sh feedback`, registered on the hub as `arboslife/feedback`, `hello.store = arbos://arboslife/feedback/`). `internal/feedback/` is right: it is a shared store folder a peer may write, and the store lint does not touch `internal/`.

## 2. Tokens — each scoped to this place by the share model, not by a per-place ACL

The hub now knows three users, so an **unset project defaults to `private`** (decision 2, now in effect for real); `feedback` is explicitly `mesh`. So a token of another user is `writer`/`reader` on `feedback` and `none` everywhere else — that is the scoping.

| Who | Vault field (item `6uihrhmgfwncp3jz3vxtfxklhi`) | Row in `hub-server.toml` | On `feedback` | On `demo`, `phone`, `subnet120` |
| --- | --- | --- | --- | --- |
| Jacob's desktop app, for reports | `client-desktop-feedback` | `desktop-feedback`, role `writer`, user `desktop-feedback` | writer | refused: `hub: no access to arboslife/<p>: the project is not shared with you` |
| Parity rig, to read reports | `client-parity-rig` | `parity-rig`, role `reader`, user `parity-rig` | reader (`read`, `list`, `tail`) | refused, same words |

Read them with `op read "op://Arbos/6uihrhmgfwncp3jz3vxtfxklhi/client-desktop-feedback"` (and `…/client-parity-rig`) straight into the consumer. Jacob's own tokens (`credential`, `client-phone`, the machine tokens) are unchanged and still `owner` everywhere.

Do not reuse his `credential` for reports: a writer token that can only reach `feedback` is the point.

## 3. Binary under `internal/` — yes

`put` with `data` (base64) lands anywhere `put` with text may: `attachments/` *or* a shared store folder. Verified: `internal/feedback/<id>/screenshot.png` written as bytes. Cap `PUT_MAX_BYTES` (20 MiB) per file.

## 4. Create-if-absent and collisions — the rule

- Text `put` honours `base_hash`: send `base_hash: ""` for `report.json` and the write happens **only if the file does not exist**; a collision answers `written {error: "…: conflict — the file exists now; read it before writing"}` and writes nothing. Then bump `<n>` and retry. Verified: second `put` of the same `report.json` with `""` → conflict.
- Bytes `put` (`data`) ignores `base_hash` today and overwrites. So write `report.json` first (it claims the folder), then the PNG into the same folder. Two reports in the same second cannot collide as long as the JSON goes first.

## 5. The hub address

Read what the kernel reads: `~/.config/arbos/hub.toml` (`url`, `machine`, `token`/`token_env`; `arbos_core::HubConfig::load()`). Do not bake a URL. While `hub-api.arbos.life` waits on Jacob's CNAME, the interim URL is the vault field `arboslife-hub-url` (I keep it current). The desktop's `hub.toml` for reports should carry the `client-desktop-feedback` token as its `token` (0600) — or `token_env` — not his machine token, so a report can never write anywhere but `feedback`.

## For the parity rig

`arbos-kernel store ls arbos://arboslife/feedback/internal/feedback/` and `store read …/<id>/report.json` work with a `hub.toml` holding the `client-parity-rig` token (verified: listing returns the report folders; `read` returns the JSON; a `put` is refused with `a reader client may not send put`). Poll the listing on your timer; nothing else is needed.

## One hub fix that came out of this

A refused attach through the tunnel arrived as a bare close (cloudflared drops frames an origin sends and then closes at once). [PR #344](https://github.com/unarbos/arbos/pull/344): the hub sends the reason, waits 400 ms, then closes properly. Already running on the ArbosLife hub.
