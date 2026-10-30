# Self-hosting Arbos

Arbos has one server-side piece: the **relay** (`arbos-relay`). Everything else — the Mac app, the iPhone app, `arbosctl`, and the agent machines — talks to it. This guide takes you from a checkout to a relay your own devices can use.

Words used here:

- **Relay**: the one internet-facing service. It holds the chat rooms, provider keys, connector tokens and the vault, meters model calls, and wakes machines.
- **Machine** (also "box"): the user's cloud computer, one per user. By default a [Daytona](https://www.daytona.io) sandbox built from the `arbos-agent` image. Each project is a **workspace** on it.
- **Process box**: the same machine, run as a child process of the relay on the relay host. No Daytona, no Docker. Good for a first run.
- **Public URL**: the `https://` address machines and devices dial back to. It must be stable for persistent projects.

## 1. Prerequisites

- Rust stable (`rust-toolchain.toml` pins the channel; `rustup` installs it).
- Node 22.19+ and pnpm 9 for `harness/` (pi needs zstd in `zlib`; 22.14 is too old).
- `pi` 1.2.0 (`npm i -g @earendil-works/pi-coding-agent@1.2.0`) for a process box; `prime-agent` 0.10.0 plus its kernel venv (see `image/Dockerfile`) for Prime subagents. Without Prime, set `ARBOS_DEFAULT_HARNESS=pi`.
- A model key: `OPENROUTER_API_KEY` (or `ANTHROPIC_API_KEY` / `OPENAI_API_KEY`). It lives on the relay host only; a machine never sees it.
- For real machines: `DAYTONA_API_KEY`.
- For the Mac app on Linux: `libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libxcb-xkb-dev libasound2-dev libssl-dev libgl1-mesa-dev libfontconfig-dev` (and `mesa-vulkan-drivers` without a GPU). The iPhone app needs Xcode 16+ and fastlane.

Build everything:

```bash
cargo build --workspace
cd harness && pnpm install && pnpm -r typecheck
```

Prebuilt static `arbos-relay` / `arbos-agentd` / `arbosctl` for Linux are attached to every [release](https://github.com/unarbos/arbos/releases).

## 2. First run: relay + process box on one host

```bash
# 1. relay (prints a one-time pairing code on first start)
export OPENROUTER_API_KEY=…
export ARBOS_AGENTD_BIN=target/debug/arbos-agentd ARBOS_PI_BIN=$(command -v pi) \
       ARBOS_PI_EXT=$PWD/harness/arbos-pi-ext/index.ts
cargo run -p arbos-relay -- --listen 127.0.0.1:7700 --data ./.arbos-local/relay

# 2. pair this machine
cargo run -p arbosctl -- pair <code>

# 3. a project (the first one creates the user's machine; later ones are workspaces on it)
cargo run -p arbosctl -- project create --name demo --local-box
cargo run -p arbosctl -- machine status

# 4. chat
cargo run -p arbosctl -- chat open demo                        # prints a chat id
cargo run -p arbosctl -- chat send demo <chat> "hello" --stream
```

Every tool, the pi main agent and the subagents run for real; only the sandbox boundary and the per-workspace Unix users are missing. Workspaces live under `.arbos-local/relay/boxes/u_default/work/<project>`. `arbosctl` keeps its device token under `~/.config/arbos/` (`ARBOSCTL_CONFIG` overrides); `cargo run -p arbosctl -- --help` lists every command.

## 3. Sign-in modes

`ARBOS_AUTH_MODE` picks how devices sign in:

- `paircode` — the relay prints a one-time code; `arbosctl pair <code>` or the apps' pairing screen exchange it for a device token. Default when no GitHub OAuth app is configured. Simplest for one person.
- `github` — "Sign in with GitHub" (OAuth, PKCE), JWT access tokens with rotating refresh tokens, credits and spend caps. Needs your own OAuth app: callback `https://<your relay>/auth/github/callback`, then `ARBOS_GITHUB_CLIENT_ID`, `ARBOS_GITHUB_CLIENT_SECRET`, `ARBOS_PUBLIC_URL` (the relay's public origin) and optionally `ARBOS_JWT_KEY`. This is what arbos.life runs.

## 4. Relay environment

All optional unless noted.

| Variable | Meaning |
| --- | --- |
| `ARBOS_RELAY_LISTEN` | bind address, default `127.0.0.1:7700`; use `0.0.0.0:7700` on a server |
| `ARBOS_RELAY_PUBLIC_URL` | the URL machines and devices dial back (**required for Daytona machines**), e.g. `https://relay.example.com`; WebSockets must pass through |
| `ARBOS_RELAY_DATA` | SQLite + blobs + local boxes, default `./.arbos-local/relay` |
| `ARBOS_RELAY_ADMIN_TOKEN` | token for the admin endpoints (`/admin/*`, `POST /pair/new`); when unset one is generated into `$ARBOS_RELAY_DATA/admin-token` at mode 0600 and the log names the path, not the token |
| `ARBOS_RELAY_KEY` | the key-encryption key for connector tokens and secrets (each user's secret values are AES-256-GCM under a per-user key wrapped by this one); generated once into the `meta` table when unset — fine for a laptop, a loud boot warning in `github` mode: set it from the environment (for example `op run`) on a real server |
| `OPENROUTER_API_KEY` / `ANTHROPIC_API_KEY` / `OPENAI_API_KEY` | provider keys for the model proxy (never reach a machine) |
| `ARBOS_OPENAI_BASE`, `ARBOS_ANTHROPIC_BASE` | upstreams, default OpenRouter |
| `ARBOS_OPENAI_ROUTE`, `ARBOS_OPENAI_DIRECT_BASE` | where `openai/*` chat models go: `auto` (default — straight to OpenAI at `https://api.openai.com/v1` with `OPENAI_API_KEY` when that key exists, else OpenRouter), `direct`, or `openrouter`; usage is metered the same way (token prices from the relay table when the upstream reports no cost) |
| `ARBOS_DECISION_SCORER` (`typesafe/jev-1.14`), `ARBOS_DECISION_THRESHOLD` (0.6) | agentd defaults (read in the box's environment; Daytona machines use the built-in values) for who replies in group chats: one judge call per human message to a decision model through the relay's `/proxy/decisions` (OpenRouter Decisions API, needs `OPENROUTER_API_KEY`) asking who the message is for — one agent, any one agent, several, all, or nobody — plus one yes/no per agent; an agent replies only when its verdict's probability reaches the threshold. No keyword rule runs before it. A person's explicit @mention (typed `@handle` or a chip) always wakes every agent it names, whatever the judge says or when it fails — the woken agent may still post nothing when the line only names it; on a judge error unmentioned messages stay silent. An agent's own bubble never wakes another agent. Per project: `PATCH /projects/{id} {"models": {"decision_model": {"scorer": …, "threshold": 0.6}}}`. `ARBOS_DECISION_PROVIDER` is accepted for old configs and ignored (there is one judge). The relay serves an identical decision request made within 15 minutes from its cache, so every box in a room gets the same answer and the room pays once |
| `ARBOS_OPENAI_AUDIO_BASE`, `ARBOS_VOICE_MOCK` | speech-to-text / text-to-speech upstream (default `https://api.openai.com/v1`, needs `OPENAI_API_KEY`); `ARBOS_VOICE_MOCK=1` answers without a key |
| `ARBOS_OPENAI_IMAGES_BASE`, `ARBOS_IMAGE_MODEL`, `ARBOS_IMAGE_MOCK` | image generation upstream and model; `ARBOS_IMAGE_MOCK=1` returns a labelled placeholder PNG |
| `DAYTONA_API_KEY`, `DAYTONA_API_URL`, `DAYTONA_ORG_ID` | Daytona access; without a key new projects get process boxes |
| `ARBOS_DAYTONA_SNAPSHOT` | snapshot for new machines (the current registered `arbos-agent-<version>`) |
| `ARBOS_DAYTONA_DOCKERFILE`, `ARBOS_BOOTSTRAP_DIR` | dev-image path (§6) |
| `ARBOS_AUTH_MODE` | `github` or `paircode` (§3) |
| `ARBOS_GITHUB_CLIENT_ID`, `ARBOS_GITHUB_CLIENT_SECRET`, `ARBOS_PUBLIC_URL`, `ARBOS_JWT_KEY` | GitHub OAuth app and the relay's public origin (§3) |
| `ARBOS_AUTH_REDIRECTS` | comma-separated URL prefixes `/auth/github/start?redirect=…` may send the browser to, besides the apps' `arbos://auth/callback`, loopback and the relay's own origin — your site's welcome page, e.g. `https://arbos.life`; anything else answers 400 `bad_redirect` (`relay-server.sh` writes `ARBOS_SITE_URL` by default) |
| `ARBOS_DEFAULT_GRANT_USD` (100), `ARBOS_DEFAULT_DAILY_CAP_USD` (20), `ARBOS_GLOBAL_DAILY_CAP_USD` (500) | inference credits: welcome grant, per-user daily cap, relay-wide daily cap that trips the kill switch (`PUT /admin/killswitch`) |
| `ARBOS_ALWAYS_ON` | machines stay on (default on in `github` mode): no auto-stop, a machine per signed-in user from sign-in; manual Stop and the kill switch are the only things that keep one down |
| `ARBOS_MACHINE_USD_PER_HOUR` (0.332), `ARBOS_MACHINE_STOPPED_USD_PER_DAY` (0.013), `ARBOS_GLOBAL_MACHINE_DAILY_CAP_USD` (400) | machine time is metered into its own ledger bucket, outside the grant and caps; the global machine cap only logs a warning |
| `ARBOS_AGENTD_BIN`, `ARBOS_PI_BIN`, `ARBOS_PI_EXT`, `ARBOS_PRIME_BIN`, `ARBOS_DEFAULT_HARNESS` | process-box binaries and default subagent harness |
| `ARBOS_MEDIA_VENV` | python with the document libraries for process boxes (`scripts/box-media-deps.sh` prints the export) |
| `ARBOS_GOOGLE_CLIENT_ID/SECRET`, `ARBOS_SLACK_CLIENT_ID/SECRET`, `ARBOS_GITHUB_CLIENT_ID/SECRET`, `ARBOS_OAUTH_REDIRECT_URL`, `ARBOS_HARNESS_COMMS_URL`, `ARBOS_CONNECTORS_MOCK` | connector OAuth apps (redirect `{public_url}/connectors/oauth/callback`); `ARBOS_CONNECTORS_MOCK=1` returns canned data |
| `ARBOS_SLACK_SIGNING_SECRET`, `ARBOS_SLACK_BOT_TOKEN`, `ARBOS_TELEGRAM_BOT_TOKEN`, `ARBOS_TELEGRAM_WEBHOOK_SECRET`, `ARBOS_TELEGRAM_POLL`, `ARBOS_WHATSAPP_TOKEN`, `ARBOS_WHATSAPP_PHONE_ID`, `ARBOS_WHATSAPP_VERIFY_TOKEN`, `ARBOS_WHATSAPP_APP_SECRET`, `ARBOS_BEEPER_POLL_S`, `ARBOS_CHANNELS_MOCK` | channel adapters (Slack Events API at `{public_url}/channels/slack/events`, Telegram webhook or long polling, WhatsApp Cloud API webhook, Beeper via Harness Comms); `ARBOS_CHANNELS_MOCK=1` turns every platform into a mock |
| `APNS_KEY_P8`, `APNS_KEY_ID`, `APNS_TEAM_ID`, `APNS_TOPIC` | iPhone notifications and VoIP call pushes over APNs (token auth; the `.p8` as text or a path); without them push is off and the relay says so once |
| `LIVEKIT_URL`, `LIVEKIT_API_KEY`, `LIVEKIT_API_SECRET`, `LIVEKIT_WEBHOOK_SECRET` | voice/video calls on your own LiveKit (§9); without them the call endpoints answer 503 `calls_off` |
| `ARBOS_VAULT_MOCK` | seeds two demo secrets and a local egress echo target |
| `ARBOS_OP_BIN` (`op`), `ARBOS_OP_CACHE_TTL_SECS` (300), `ARBOS_OP_ITEMS_TTL_SECS` (3600), `ARBOS_OP_CONFIG_DIR` (`<data>/op`), `ARBOS_OP_READS_PER_HOUR` (1000), `ARBOS_OP_READS_PER_DAY` (1000), `ARBOS_OP_DISABLED` | connected 1Password vaults (Settings → Secrets → Connect a vault): the relay resolves `op://` references with the pinned `op` CLI (`scripts/deploy/relay-server.sh` installs 2.30.3), keeps resolved values in memory for the cache TTL and the item list for the items TTL, and refuses with 429 at 95 % of the per-hour / per-day read limits of your 1Password plan (Individual and Families: 1,000 reads/h per token and 1,000 requests/day for the account; raise both for Teams/Business). `ARBOS_OP_DISABLED=1` hides the provider |
| `ARBOS_FEATURE_VAULT_CONNECT` (off) | remote flag: `1` shows "Connect a vault…" (1Password) in the Mac and iPhone apps and allows `POST /vaults`; off (the default since 2026-10-21) hides the connect flow everywhere without a client release — the relay answers `providers: []` and `features.vault_connect: false` on `GET /secrets` and `GET /me`, and `POST /vaults` → 403 `feature_off`. Vaults already connected keep working either way |
| `RUST_LOG` | e.g. `info,arbos_relay=debug` |

Example for a public relay on one server, TLS terminated by a reverse proxy (Caddy, nginx) or a Cloudflare tunnel in front of the plain-HTTP relay:

```bash
ARBOS_RELAY_LISTEN=0.0.0.0:7700 ARBOS_RELAY_PUBLIC_URL=https://relay.example.com cargo run --release -p arbos-relay
```

`scripts/relay-start.sh` starts the relay with a cloudflared tunnel beside it: `ARBOS_TUNNEL=named` (a Zero Trust tunnel whose token is in `ARBOS_TUNNEL_TOKEN`; `ARBOS_TUNNEL_HOSTNAME` sets the name), `ARBOS_TUNNEL=quick` (dev only — a random `*.trycloudflare.com` URL that changes on every restart), or `ARBOS_TUNNEL=none` with your own `ARBOS_RELAY_PUBLIC_URL`. `scripts/deploy/relay-server.sh` is the systemd + Caddy install used for relay.arbos.life (`--from-source <ref>` builds the relay on the server).

If the public URL changes, the relay writes the new one into every machine at the next wake (`/work/.arbos/relay-url`) and agentd prefers that file; a machine that still cannot say hello within 60 s is bounced once and then shown as `error · box cannot reach relay`.

## 5. Real machines on Daytona

With `DAYTONA_API_KEY` set, `project create` (without `--local-box`) creates a sandbox from `ARBOS_DAYTONA_SNAPSHOT` at **4 vCPU / 8 GiB / 10 GiB** (the Daytona per-sandbox maximum; about $0.33 per active hour, $0.013 per stopped day). The relay must be reachable from the sandbox (`ARBOS_RELAY_PUBLIC_URL`).

- One machine per user; every project is a workspace `/work/<project>` owned by its own Unix user at mode 700, with its own pi session, subagents, SQLite memory/history, blob dir, browser profile and cgroup slice. One `arbos-agentd` per machine supervises them; up to 4 subagents per workspace and 8 per machine run at once.
- `GET /machine` shows state, spec, spend and workspace states; `POST /machine/start|stop` drives it; `arbosctl machine status|start|stop` from a terminal. Per-user snapshot override: `PUT /admin/users/{id}/box {"snapshot": "…"}` before the machine exists.
- `cargo run -p arbos-relay -- daytona-smoke` runs the lifecycle check (`create → stop → start → archive → start → delete`) against the live API.
- Older per-project sandboxes are copied into the user machine with `arbosctl migrate-to-user-box` (idempotent; old sandboxes are stopped and marked, never deleted).

## 6. The machine image

`.github/workflows/image.yml` builds `image/Dockerfile` on an image tag (`image-v<version>`) or by hand (`workflow_dispatch` with a version), pushes it to GHCR as `ghcr.io/unarbos/arbos/arbos-agent:<version>` (public), smoke-tests it, and registers the Daytona snapshot `arbos-agent-<version>` with `image/register-snapshot.sh` (needs the `DAYTONA_API_KEY` repository secret; otherwise run the script by hand). The image bakes in `arbos-agentd`, so every agentd change needs an image rebuild; machines do not update it at boot.

Without CI, the same build runs on any Docker host and goes to Daytona's transient registry (tags are immutable: bump the version for every rebuild):

```bash
CC_x86_64_unknown_linux_musl=musl-gcc cargo build --release -p arbos-agentd --target x86_64-unknown-linux-musl
mkdir -p dist && cp target/x86_64-unknown-linux-musl/release/arbos-agentd dist/
docker build --platform linux/amd64 -f image/Dockerfile -t arbos-agent:<v> .
# temporary push credentials: GET $DAYTONA_API_URL/docker-registry/registry-push-access?snapshotName=arbos-agent-<v>
docker tag arbos-agent:<v> cr.app.daytona.io/<project>/arbos-agent:<v> && docker push cr.app.daytona.io/<project>/arbos-agent:<v>
image/register-snapshot.sh arbos-agent-<v> cr.app.daytona.io/<project>/arbos-agent:<v> 4 8 10
```

While no production snapshot is registered, point the relay at the dev image instead: `ARBOS_DAYTONA_DOCKERFILE=image/Dockerfile.dev-inline ARBOS_BOOTSTRAP_DIR=./dist ARBOS_DEFAULT_HARNESS=pi` (`dist/` holds the static agentd and `arbos-pi-ext.tar`; the machine fetches both at boot).

## 7. Pointing the apps at your relay

- **Mac**: `defaults write sh.arbos.mac RelayURL https://relay.example` for the bundled app, or `ARBOS_RELAY_URL=https://relay.example cargo run -p arbos-mac` from source (`ARBOS_RELAY_URL=fixture` runs the seeded demo without a relay). In `paircode` mode the sign-in screen falls back to the one-time code from the relay log. `ARBOS_RELAY_TOKEN` overrides the stored credentials.
- **iPhone**: build `apps/ios/Arbos` with Xcode and enter the relay URL on the pairing screen (`SIMCTL_CHILD_ARBOS_RELAY_URL` / `_TOKEN` for simulator runs).
- **Terminal**: `arbosctl --relay https://relay.example pair <code>`.
- **Your own computer**: `arbos-local pair <code> --relay https://relay.example --name "My laptop"` then `arbos-local run` lets an agent run a command or read a file on it, behind approval cards (Settings → This Mac lists paired helpers).

## 8. Updates

Self-built relays update by rebuilding. The Mac app updates itself from the appcast of the channel it was built on (`ARBOS_UPDATE_CHANNEL=stable|edge`, `ARBOS_UPDATE_FEED=<url>` to point at your own feed, `ARBOS_UPDATE_DISABLE=1` to turn it off). `scripts/bundle-mac.sh` builds a universal `dist/Arbos.app`, the DMG and the updater tarball, signing and notarizing when the credentials are present and saying what it skipped otherwise.

## 9. Calls (LiveKit)

Voice and video calls between people run on a [LiveKit](https://github.com/livekit/livekit) server you host next to the relay. The relay decides who may be in a call, mints the join tokens, and keeps the call log; media goes LiveKit ↔ apps and never through the relay.

1. Run `livekit-server` with a key pair and a webhook pointing at the relay (`livekit.yaml`):

```yaml
port: 7880
rtc:
  tcp_port: 7881
  port_range_start: 50000
  port_range_end: 60000
  use_external_ip: true
keys:
  <LIVEKIT_API_KEY>: <LIVEKIT_API_SECRET>
turn:
  enabled: true
  domain: livekit.example.com
  tls_port: 5349
webhook:
  api_key: <LIVEKIT_API_KEY>
  urls:
    - https://relay.example.com/livekit/webhook
```

Put TLS in front of port 7880 as `wss://livekit.example.com` (Caddy: `reverse_proxy localhost:7880`), open UDP 50000–60000 and TCP 7881, and give TURN a certificate (or terminate TURN/TLS on 443 so strict networks work). LiveKit signs each webhook with the API key pair; the relay verifies it with `LIVEKIT_API_SECRET` (set `LIVEKIT_WEBHOOK_SECRET` only when the webhook uses a different key).

2. Give the relay `LIVEKIT_URL=wss://livekit.example.com`, `LIVEKIT_API_KEY`, `LIVEKIT_API_SECRET` and restart it; the log says `LiveKit calls on`. `arbosctl admin calls` shows the configuration, live calls, and the last ended ones.

3. Call tokens are good for an hour, rooms are named `call_<ulid>`, at most 16 people. A call ends when the last person leaves, when LiveKit reports `room_finished`, or when nobody has been in it for a minute (the relay then closes the LiveKit room too). Ringing on iPhones that are closed needs the APNs key above (PushKit VoIP push, same key).

### 9.1 The arbos.life deployment (`scripts/deploy/livekit/`)

Our own LiveKit runs on the relay host in Docker; `scripts/deploy/livekit/setup.sh` (root, idempotent) reproduces it and is the reference for a self-hoster who wants the same shape:

| piece | where | notes |
|---|---|---|
| server | `livekit/livekit-server:v1.13.10`, `docker-compose.yml`, host networking, `restart: unless-stopped`, json logs rotated 5×20 MiB | wrapped by `livekit.service` (systemd, after Docker) |
| config | `/etc/livekit/livekit.yaml` from `livekit.yaml.tmpl`; keys + hostnames in `/etc/livekit/livekit.env` (root only) | `node_ip` is the public IP because the host sits behind 1:1 NAT |
| signaling + API | `wss://livekit.arbos.life` → Caddy → `127.0.0.1:7880` (`Caddyfile.livekit`, imported from `/etc/arbos/sites.d/`) | one hostname serves the WebSocket and the HTTP (`/twirp`) API the relay calls |
| media | UDP 50000–50200 and TCP 7881 straight to the server | sized for ~20 concurrent participants; widen the range for more |
| TURN | embedded: `turn:204.12.171.6:3478?transport=udp`, `turns:turn.arbos.life:443?transport=tcp` (443 via the Caddy layer4 SNI route → LiveKit's 5349 listener) | certificate comes from Caddy's `turn.arbos.life` vhost, copied by `arbos-livekit-sync-cert.timer` (daily; restarts LiveKit when it changes) |
| firewall | ufw: 7881/tcp, 3478/udp, 5349/tcp, 50000–50200/udp; 7880 stays closed (Caddy only) | 5349 stays open for clients that honour a custom port; LiveKit itself points them at 443 |
| webhook | `https://relay.arbos.life/livekit/webhook`, signed with the API key | |
| secrets | vault item **Arbos LiveKit** (`LIVEKIT_URL`, `LIVEKIT_API_KEY`, `LIVEKIT_API_SECRET`); `relay-server.sh` carries them into `/etc/arbos/relay.env` | |

**TURN/TLS on 443 with one IP.** Caddy owns 443 for the sites, and LiveKit always advertises its TURN/TLS server as `turns:<turn.domain>:443` (the `tls_port` only chooses the local listener). So the :443 Caddy is built with the [caddy-l4](https://github.com/mholt/caddy-l4) module (`scripts/deploy/livekit/build-caddy.sh`, xcaddy) and its global `servers :443 { listener_wrappers { layer4 … } }` block hands every TLS connection whose SNI is `turn.arbos.life` — except ACME `acme-tls/1` challenges, which Caddy keeps so it can renew that certificate — untouched to LiveKit's TURN/TLS listener on 127.0.0.1:5349; LiveKit terminates TLS with the certificate `arbos-livekit-sync-cert` copied from Caddy. Everything else on 443 is HTTPS as before (HTTP/3 on UDP 443 is unaffected). Verified from an outside host with every other port to the server dropped by iptables: a publisher and a subscriber joined over `turns:turn.arbos.life:443` alone with 0% loss. DNS: `livekit` and `turn` are DNS-only A records (no Cloudflare proxy) — the TLS-ALPN challenge and TURN both need the real address.

Checks after `setup.sh`: `curl https://livekit.arbos.life/` prints `OK`; `lk room list --url wss://livekit.arbos.life --api-key … --api-secret …` works from anywhere; `lk load-test --room t --publishers 1 --subscribers 1` joins two participants; the relay log says `LiveKit calls on` after its restart.

## 10. Chats on Matrix (Synapse, D88)

Arbos is moving every chat onto a [Matrix](https://matrix.org) room underneath the relay (design: the Project's `matrix-cutover-design.md`; users never see the word Matrix). The relay stays the only thing the apps talk to; it maps `chat_id ↔ room_id`, creates users through an appservice, and dual-writes every chat into the homeserver first (stage A) before any read is served from it (stage B, per-user flag).

### 10.1 The arbos.life deployment (`scripts/deploy/matrix/`)

Ring 0 runs [Synapse](https://github.com/element-hq/synapse) (community edition, unmodified, AGPL-3.0; config and modules only) with PostgreSQL on the relay host in Docker, at zero extra cost. `scripts/deploy/matrix/setup.sh` (root, idempotent) reproduces it:

| piece | where | notes |
|---|---|---|
| homeserver | `ghcr.io/element-hq/synapse:v1.163.0`, `docker-compose.yml`, host networking, `restart: unless-stopped`, json logs rotated 5×20 MiB | wrapped by `arbos-matrix.service` (systemd, after Docker); client API on `127.0.0.1:7708` only; no federation listener, port 8448 closed |
| database | `postgres:16` on `127.0.0.1:7709`, data in `/var/lib/arbos-matrix/postgres`, `LC_COLLATE=C` | the shadow-compare job reads it too |
| config | `/etc/arbos-matrix/synapse/homeserver.yaml` from `homeserver.yaml.tmpl`; tokens in `/etc/arbos-matrix/matrix.env` (root only) | `server_name: arbos.life` (fixed forever), `public_baseurl: https://relay.arbos.life/`, registration off, presence off, room version 12 default, `federation_domain_whitelist: [arbos.life]`, `rc_joins_per_room 10/s burst 200` |
| appservices | `/etc/arbos-matrix/synapse/appservices/{arbos-users,arbos-agents,arbos-bridges}.yaml` from `appservice.yaml.tmpl`, `url: http://127.0.0.1:7700` (the relay) | users `@gh_*`/`@u_*`/`@test_*` (non-exclusive; `/login` allowed so app devices get real tokens), agents `@_a_*`/`@_x_*` and bridges `@_b_*` (exclusive, MSC4190 device management, MSC3202/MSC2409 flags on) |
| media | `/var/lib/arbos-matrix/media` (uid 991) | object storage comes with ring 2 |
| signing key | `/etc/arbos-matrix/synapse/arbos.life.signing.key`, generated once by `setup.sh` | never regenerate |
| secrets | vault item **Arbos Matrix** (one field per `ARBOS_MATRIX_*` name); `relay-server.sh` carries the homeserver URL, server name and the six appservice tokens into `/etc/arbos/relay.env` | `ARBOS_MATRIX=off|dual|on` is an operator switch in `relay.env`, never read from the vault; default off |

Checks after `setup.sh`: `curl http://127.0.0.1:7708/health` prints `OK`; `curl http://127.0.0.1:7708/_matrix/client/versions` lists the spec versions; `docker logs arbos-matrix-synapse` shows the three appservices; a `POST /_matrix/client/v3/register {type: m.login.application_service, username: gh_x}` with the users `as_token` returns a user id, token and device id.

Kill switch for the relay side: `sudo touch /etc/arbos/matrix.disabled` stops dual-writes (the outbox keeps queueing); `sudo rm` it to resume.

### 10.2 Servers and channels (C1)

With `ARBOS_MATRIX=dual|on` the relay also serves **servers** (a Space on the homeserver) and **channels** (rooms inside it; messages live only there): `GET|POST /servers`, `GET|PATCH|DELETE /servers/{id}`, `POST /servers/{id}/channels`, `PATCH|DELETE /servers/{id}/channels/{cid}` (`restricted: true` = only owner/admins post, shown with a lock), `POST /servers/{id}/members {user_id}` (owner/admin adds someone by name — the main way in), `GET|POST /servers/{id}/invites`, `DELETE /servers/{id}/invites/{iid}` and `POST /servers/{id}/invite/reset` (owner only: several links per server, optional 7-day expiry and limited uses), `POST /servers/join {code}`, `GET /s/{code}` (the invite landing: hands the link to the installed app via `arbos://join/<code>`, download fallback; the site proxies `arbos.life/s/*` here and links are minted with `ARBOS_SITE_URL`), `PUT|DELETE /servers/{id}/members/{uid}` (roles, remove, `?ban=1`), `POST /servers/{id}/leave`. Each signed-in device gets its own Matrix identity from `GET /me/matrix` (`{homeserver, user_id, access_token, device_id}`, minted through the appservice login, revoked with the device) and talks to the homeserver through the relay's `/_matrix/client/*` proxy — the apps never see another host. The Space, the channel rooms (`join_rule: restricted` to the Space, `m.space.parent`/`m.space.child`, `life.arbos.chat {kind: channel}`), members and power levels are kept in step by the same outbox worker as the chats.

### 10.3 Chat export and REST post (bots, #1464)

A device that cannot run a Loro client (a bot) reads and writes people chats over plain HTTPS. `GET /chats/{id}/export?since=<ms>&after_id=<id>&limit=<1..1000>&format=json|md` (members and the admin) rebuilds the chat doc read-only — checkpoint + rows, the same way the Matrix backfill does — and returns the finished messages after the `(since, after_id)` cursor with `next`, `has_more` and `total`; attachments come as `GET /export/blob/{chat}/{key}?exp=&sig=` links signed with the relay's JWT key, valid one hour, no bearer. `POST /chats/{id}/messages {text, reply_to?}` posts as the calling device: the relay appends the row the apps would write, broadcasts it, mirrors it to Matrix and announces it like `/chats/{id}/notify`. Optionally a normal device mints a read-only token with `POST /auth/devices {name, kind:"reader"}`: it works only for `GET /chats`, the export and the signed blob route, every other call answers 403 `reader_only`; revoke it like any device.
