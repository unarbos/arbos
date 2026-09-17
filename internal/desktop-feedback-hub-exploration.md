---
cursor:
  subagentId: "bc-1dee55eb-63d8-559c-85a6-2c378a051af6"
---

# Desktop feedback delivery — hub, mesh store, queues, HTTP (exploration)

Read-only audit of `/workspace` (unarbos/arbos) on 2026-09-16. No secrets printed.

---

## 1. Hub server (`crates/arbos-hub/`)

### HTTP routes (all handlers in `crates/arbos-hub/src/main.rs`)

| Method | Path | Handler | Lines |
|--------|------|---------|-------|
| GET | `/` | plain 200 `"ok\n"` | 104–105 |
| GET | `/healthz` | plain 200 `"ok\n"` | 104–105 |
| GET | `/list` | JSON roster (`hub.roster_for`) | 132–136 |
| WS | `/register` | `hub::register` | 138–141 |
| WS | `/attach/<machine>` | `hub::attach(..., project: None)` | 143–146 |
| WS | `/attach/<machine>/<project>` | `hub::attach(..., Some(project))` | 148–151 |
| WS | `/claim/<machine>` | `hub::claim` | 153–156 |

- Non-GET methods: **405** at lines 101–102 (`"GET only\n"`).
- Unknown paths: **404** at lines 158–161.
- Usage string listing routes: line 16.

WebSocket implementations: `register` at `hub.rs:389`, `attach` at `hub.rs:675`, `claim` at `hub.rs:824`, `proxy` at `hub.rs:731`.

### Auth model

**Implemented today: bearer tokens only** (not `access.toml`, not Cloudflare Access JWT yet).

- Config: `hub-server.toml` (`--config` / `ARBOS_HUB_CONFIG`; default `~/.config/arbos/hub-server.toml`) — documented in `auth.rs:3–16`, loaded at `main.rs:39–40`.
- Rows: `[[machine]]` (register + attach as owner) and `[[client]]` (attach only, `role = owner|writer|reader`) — `auth.rs:28–52`, parsed `auth.rs:138–179`.
- Token resolution: inline `token = "…"` or `token_env = "VAR"` — `auth.rs:231–249`; minimum 16 chars.
- Presentation: `Authorization: Bearer …` or `?token=…` — `auth.rs:260–274`, checked at `main.rs:107–108`.
- Constant-time compare: `auth.rs:193–203`, `auth.rs:252–257`.
- Machine token attaches as `machine:<name>` with owner role — `auth.rs:94–99`.
- **Not implemented in code:** `trust = "cloudflare-access"` (comment only at `auth.rs:20–21`; design in `docs/design/filesystem-state-design.md:796`).

Client-side hub config (separate file): `~/.config/arbos/hub.toml` — `crates/arbos-core/src/hub.rs:306–327`.

### State storage

**In memory for roster/registry; one JSON file for push registry.**

- Hub registry: `Hub` struct, `HashMap` of machines/kernels rebuilt from live WebSocket registrants — `hub.rs:11–13`, `hub.rs:91–117`, `hub.rs:240–244`. Disconnect removes machine from roster immediately.
- Push device registry: `hub-push.json` beside hub config dir — `push.rs:11–12`, `push.rs:96–107`, `main.rs:57–58` (`config_dir` passed to `Push::new`).
- **No SQLite.** **No general upload store.**

### Data directory

- Config dir: parent of `hub-server.toml`, else `~/.config/arbos/` — `main.rs:41–44`.
- Push file: `<config_dir>/hub-push.json` — `push.rs:107`.
- Default bind: `127.0.0.1:7010` — `main.rs:48`, `hub-server.example.toml:4`.

### Blob upload / POST

**No HTTP POST or multipart upload route.** Hub accepts **GET only** (`main.rs:101–102`). Binary delivery is **WebSocket frame relay** only:

- Client sends `Frame::Put { path, data: <base64>, ... }` through an attach socket; hub proxies JSON to kernel (`hub.rs:761–765` comment; kernel handles in `files.rs:29–37`, `put_bytes` at `files.rs:136–207`).
- Hub's own outbound POST: APNs push only — `push.rs:13–14`, `reqwest` client `push.rs:117–119`.
- `push` registration frame from attach client (not HTTP) — `hub.rs:770–779`.

---

## 2. Hub deployment and configuration

### In repo (`deploy/hub/`)

| File | Purpose | Key lines |
|------|---------|-----------|
| `hub-server.example.toml` | Server tokens, bind | 4, 13–34 |
| `hub.example.toml` | Client `url`, `machine`, token | 5–7 |
| `hub-run.sh` | Restart loop for `arbos-hub` | 4–12 |
| `worker.sh` | `arbos-kernel worker` loop | 10–16 |
| `hub-quick-tunnel.sh` | Ephemeral `*.trycloudflare.com` → loopback | 1–6, 12–17 |

**Not in repo:** systemd units, Caddy configs, named Cloudflare tunnel config for hub, `deploy/cloudflare/` (mentioned in design docs only).

### Hostnames

| Hostname | Where referenced | Repo vs DNS |
|----------|------------------|-------------|
| `wss://hub-api.arbos.life` | `deploy/hub/hub.example.toml:5`, `crates/arbos-core/src/hub.rs:309`, `www/install/index.html:100` | **Example/stable name in repo**; Project notes say CNAME still pending on Jacob's side |
| `wss://kernel-api.arbos.life` | `ios/scripts/gen-secrets.sh:26` | Same — stable name documented, DNS cutover external |
| `*.trycloudflare.com` | `hub-quick-tunnel.sh:14–16`, iOS defaults in `AppSettings.swift:16–17` | Interim until named tunnel + DNS |

### Tunnel pattern (repo)

- Hub listens loopback; `cloudflared` terminates TLS — `http.rs:2–3`, `main.rs:96–100` (`cf-connecting-ip`).
- Quick tunnel script writes `public-url.txt` with rotating URL — `hub-quick-tunnel.sh:3,16`.
- Named tunnel ingress described in comments: `hub-api.example.com -> http://127.0.0.1:7010` — `hub-quick-tunnel.sh:4–5`.
- Voice stack has fuller tunnel tooling (`voice-server/deploy/cloudflare-tunnel.sh`) — **not wired for hub in repo**.

### ArbosLife layout (from scripts, not enforced in repo)

- `hub-run.sh`: `$HOME/arbos-hub` with `bin/arbos-hub`, `hub-server.toml`, `logs/` — lines 4–6.
- `worker.sh`: same base, `config/arbos/hub.toml`, `projects/` — lines 5–6, 10–11.

---

## 3. Federated store addressing (`arbos://`)

### Merge status (main @ 2cf7d5b9)

All three PRs **merged into main**:

- **#251** — `0d99bde7` — store addresses, roster discovery, `[share] mode`, `hello.store`
- **#254** — `9a39b06c` — read/write by address, hub enforces access
- **#257** — `907c6a54` — remote child briefs name parent store by address

### Address format

- Scheme: `arbos://<machine>/<project>/<path>` — `crates/arbos-core/src/hub.rs:432–435`, parser `466+`.
- Store root: `StoreAddress::root(name, p)` — used in hub roster `hub.rs:159`.

### API / route / frame

1. **Discovery:** `GET /list` returns per-project `store`, `access`, `share` — hub builds at `hub.rs:152–200`, `access_of` at `265–277`.
2. **Transport:** WebSocket `wss://<hub>/attach/<machine>/<project>` with bearer token — `hub.rs:675–725`, URL builder `arbos-core/hub.rs:382–396`.
3. **Frames (client → kernel via hub proxy):**
   - Read: `Frame::Read { path }` — `wire.rs` (read section ~line 100+)
   - List: `Frame::List { path }`
   - Write: `Frame::Put { path, text, data, base_hash }` — `wire.rs:144–158`
   - Reply: `Frame::Written { path, size, hash, error }` — `wire.rs:162–169`
4. **CLI:** `arbos-kernel store read|ls|put arbos://…` — `store_cmd.rs:9`, `store_write` via `hub_link.rs:544–562`.
5. **Engine tools:** `write`/`read`/`ls` on `arbos://` — `arbos-engine/src/tools/fs.rs`, hooks `sched.rs:177–198`.

### Auth / rights model

- Hub token roles: `owner | writer | reader` — `auth.rs:158–164`.
- Effective store access: `store_access(share, viewer_user, viewer_role, owner_user)` — `hub.rs:658–687`; caps client role with project `[share] mode` (`private` | `mesh` | `open`) — `hub.rs:696–704`.
- Attach refused when `access == "none"` — `hub.rs:704–723`.
- Default share: `mesh` if one hub user, `private` if multiple — `auth.rs:221–227`.

### Receiving kernel rules (root pages refused)

At `crates/arbos-kernel/src/files.rs`:

- Root-owned pages refused to peers — `files.rs:46–49`, check `files.rs:80–84`, `store::REFUSAL` at `store.rs:332`.
- Protected files never written — `files.rs:74–78`, `NEVER` includes `access.toml` — `files.rs:22`.
- Peers write only `docs/`, `internal/`, `media/` (and attachments path for binary) — `files.rs:43–44`, `86–88`.
- Compare-and-swap on `base_hash` — `files.rs:50–52`, `93–107`.
- Binary via `data` base64, max `PUT_MAX_BYTES` (20 MiB) — `wire.rs:5`, `files.rs:131–188`.

**Verdict for feedback JSON + PNG:** Federated `put` to e.g. `arbos://arboslife/<project>/internal/feedback/<id>.json` plus sibling PNG is **already supported** (text + `data` field), subject to share mode and token role — **not** via a dedicated hub upload endpoint.

---

## 4. Desktop queue / offline persistence

### Two layers

**A. Kernel-held follow-up queue (durable — PR #129)**

- User `Frame::User` while agent busy → inbox file under `<place>/.arbos/agents/<id>/inbox/` — `serve.rs:817–819`, `855–867`, `884–885`.
- `plan::scan` only claims `wake=true` inbox when agent **not** running — `plan.rs:42–47`.
- Inbox files on disk survive kernel restart — `inbox.rs:1–20`, `158–186`.
- Desktop `Session::prompt` while streaming delegates to kernel — `acp.rs:394–396`; `queue_next` holds via kernel when socket live — `session.rs:1591–1618`.
- Plan UI shows queued inbox as `PlanNode` with `inbox: true` — `hooks.rs:587–614`.

**B. Desktop in-memory / transcript queue (partially durable)**

| Mechanism | Location | Persists? |
|-----------|----------|-----------|
| `ChatSession.queue: VecDeque<Prompt>` | `session.rs:517` | **No** — not in `Record` (`record.rs:19–51`) |
| `pending_wire` + `land_turn` | `session.rs:1514–1517`, `520` | **Partially** — user bubble in `items` flushed to `.arbos/desktop/sessions/*.json` via `flush`/`to_record` — `session.rs:908–944`, `record.rs:1–8` |
| Reconnect backoff | `workspace.rs:1695–1724` | 2→4→8→16→32→60s cap, max 30 tries |
| Reconnect notice / pump | `session.rs:3639–3659`, `3818–3826` | — |

**Retry/backoff (desktop attach):** `workspace.rs:1697–1723` — `delay = 2^min(attempt,6)` seconds capped at 60s; `RECONNECT_TRIES = 30` at line 1695.

**iOS analogue (PR #268/#269 context):** `ChatStore.pendingSends` — `ios/Arbos/Chat/ChatStore.swift:86`, shown as pending cards; `flushPending` on reconnect — `375–389`; backoff `scheduleReconnect` — `238–255` (2,4,8,15s cap). **In-memory only** — not in UserDefaults; survives offline **while app alive** via UI cards + reconnect flush.

### Reuse for feedback delivery?

- **Kernel inbox path:** good for text+json metadata tied to a running chat; awkward for large PNG + structured report not tied to an agent turn.
- **Federated `put`:** better fit for blob + JSON into a known store path.
- **Desktop `VecDeque`:** not general-purpose outbox — no disk queue, no retry to arbitrary HTTP endpoint.
- **`record.rs` async writer:** pattern for durable file writes (`record.rs:118–230`) but session-scoped, not outbound delivery.

---

## 5. Desktop outbound HTTP

| Crate | Use | Location |
|-------|-----|----------|
| **ureq** | Primary desktop HTTP | `desktop/Cargo.toml:84` |
| Shared probe client | 2s global timeout | `kernel.rs:188–199` |
| Update checker / download | `ureq::Agent`, 60s timeout | `update.rs:478–482`, feed fetch elsewhere in file |
| Remote kernel / gateway | Per-call agents, 20s | `kernel.rs:457–460`, `607`, `726` |
| Model catalog | `kernel.rs:461` GET `/api/models` | |
| Voice | WebSocket (`voice_ws.rs`), not REST upload | |
| Agent module | `ureq::get` for URLs | `agent/mod.rs:87` |
| History | `history.rs:17` | |

**reqwest** appears in desktop lockfile via `bezel-zed-reqwest-client` / transitive deps — **not** the app's direct HTTP layer for hub/feedback.

**Existing upload pattern (attachments):** WebSocket `Frame::Put` with base64 `data`, not HTTP multipart — `acp.rs:414–472`, `put_attachment` at `450–472`.

**arbos-update:** `reqwest` blocking — `crates/arbos-update/Cargo.toml:30`, `updatectl.rs:368–393` GET feed/payloads only.

---

## 6. Polling the hub (agent / client side)

**Nothing in repo polls the hub on a ~15-minute schedule for feedback.**

| Component | What it polls | Interval |
|-----------|---------------|----------|
| iOS `ProjectStore.refresh` | `GET /list` | On demand (pull-to-refresh / view appear), not timer — `ProjectStore.swift:35–85`, `HubClient.swift:67–86` |
| iOS `gen-secrets.sh` | Probes `/list` for hub health | Ad hoc script — `gen-secrets.sh:28–33` |
| Kernel `hub_link` | Outbound WS `/register` | Reconnect backoff forever 2→60s — `hub_link.rs:181–190`, `207–231` |
| Kernel subscriptions | GitHub, timers, doors | Various — `subs.rs`, `chatdoor.rs:13` (doors min 3s) |
| Desktop | **No** periodic `/list` or hub poll found | — |

The **15-minute poll** in Project notes refers to **App Store Connect beta feedback** (`internal/mobile-feedback-log.md:10`) — external to the arbos repo, not hub API.

---

## Implications for desktop in-app feedback

1. **Hub is not a blob inbox** — use mesh store `put` (WebSocket through `/attach`) or add a new route (out of scope for this audit).
2. **Delivery target on ArbosLife:** write to a known path under a hub-visible project's store (e.g. `internal/feedback/…`) with JSON + PNG via `Put` frames; agent polls that directory via `read`/`list` on a subscription or timer — **agent-side**, not hub-side.
3. **Offline:** desktop should persist an outbound outbox locally (new code); existing queues are chat-scoped, not general delivery.
4. **DNS:** ship against `hub-api.arbos.life` in config examples; runtime may still use quick-tunnel URLs until CNAMEs land.
