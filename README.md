<p align="center">
  <img src="site/assets/logo/arbos-macos-icon-1024.png" width="96" alt="Arbos">
</p>

<h1 align="center">Fully open source Grokbot clone that you can run on your own infrastructure.</h1>

<p align="center">
  <a href="https://arbos.life/download/mac"><b>Download for Mac</b></a>
  &nbsp;·&nbsp;
  <a href="https://testflight.apple.com/join/ec5fWCuJ"><b>iPhone (TestFlight)</b></a>
  &nbsp;·&nbsp;
  <a href="https://arbos.life"><b>arbos.life</b></a>
  &nbsp;·&nbsp;
  <a href="docs/self-host.md"><b>Self-host</b></a>
</p>

<p align="center">
  <img src="docs/assets/hero.png" width="100%" alt="The Arbos Mac app and the iPhone app showing the same chat with a project agent">
</p>

Arbos is a messenger for working with AI agents. You talk to a project agent in a chat — on the Mac, on the iPhone, or from a terminal — and it does the work on your own cloud computer: it answers, acts in a few tool calls, or hands bigger jobs to subagents that run in the background and report back in prose. Every user has **one** machine; every project is a workspace on it with its own folder, agent sessions, memory and history. Provider keys never leave the relay.

Use it hosted at [arbos.life](https://arbos.life) (sign in with GitHub), or [run the whole thing yourself](docs/self-host.md).

## What is in this repository

| Path | What |
| --- | --- |
| `crates/arbos-mac` | The Mac app (GPUI): agent and people list, chat with tabs, composer with voice and attachments, calls, approval cards, profile pages, Settings (Apple-style groups), machine status, in-app updates, Send feedback. Also builds on Linux. |
| `apps/ios/Arbos` | The iPhone app (SwiftUI): one chat per agent over the relay, home with machine status, people chats and groups, calls, profile pages, attach sheet, voice memos, notifications, Settings. |
| `crates/arbos-relay` | The one service outside the machines: sign-in, project registry and sync rooms (SQLite), machine lifecycle on Daytona, model proxy with per-project metering and spend caps, blob store, connectors/vault broker, routines, channels, group and people chats. |
| `crates/arbos-agentd` | The machine daemon: one process per machine supervising one workspace per project — chat doc host, SQLite+FTS5 history and memory, project MCP server, the pi main-agent driver, the subagent harness registry, per-workspace isolation. |
| `crates/arbosctl` | Headless client: projects, chats (send/stream/stop/answer), machine start/stop/status, routines, skills, channels, groups, blobs, admin. |
| `crates/arbos-local` | Helper that runs on your own computer so an agent can run a command or read a file there, behind approval cards. |
| `crates/arbos-update` | Appcast + Ed25519 payload signatures + version order shared by the release pipeline and the app's Update button. |
| `crates/arbos-core` | Shared types: ids, room frames, the chat-doc schema, the `route` schema, approval rules. |
| `harness/arbos-pi-ext` | The pi extension (TypeScript): `route`, `ask_user`, deny+audit hook, act-call limits, output offload, working set. |
| `image/` | The `arbos-agent` machine image (pi + Prime + Claude Code + agent-browser/Chrome), hook scripts, Prime skills; `register-snapshot.sh`. |
| `assets/avatars/trees` | Retired tree characters (replaced by coloured-initial avatars, D79); the brand mark is the acacia tree in `site/assets/logo` (D84). |
| `site/` | arbos.life. |
| `scripts/` | `bundle-mac.sh` (DMG, signing, notarization), `release/`, `deploy/` (relay server, site sync), `runner/` (self-hosted Mac runner), `relay-start.sh`, `site/` (social preview), `readme-hero/` (this page's hero image). |
| `docs/` | [Self-hosting guide](docs/self-host.md) and the README assets. |

## How it works

- **Relay** (`arbos-relay`): the only internet-facing piece. Devices sign in with GitHub (or pair with a one-time code when you self-host) and talk to it over HTTPS/WebSocket; machines dial back to its public URL. It owns the chat rooms, wakes machines, forwards model calls with metering, and keeps every secret: provider keys, connector tokens, the vault.
- **One machine per user**: a Daytona sandbox from the `arbos-agent` image (4 vCPU / 8 GiB / 10 GiB). Each project is a workspace on it: `/work/<project>` owned by its own Unix user, its own pi session, subagents, SQLite memory/history and cgroup slice. Machines stay on for signed-in users.
- **Harness**: the main agent is **pi** with the `arbos-pi-ext` extension; every turn starts with a `route` call (`reply`, `act` with a few tool calls, or hand off to a subagent). Subagents run through the harness registry — **Prime Agent** by default, **pi**, or **Claude Code** — up to 4 per workspace and 8 per machine, and report back as a prose turn written by the main agent.
- **Clients**: the Mac app, the iPhone app and `arbosctl` are peers of the same chat documents (Loro CRDT replicas), so a conversation continues on any device. On the iPhone each agent is one conversation; the Mac adds tabs for work that runs in parallel.
- **Avatars**: agents and people get a coloured circle with their initials (a 7-colour palette, the colour derived from the stable id, the same on every device and on the relay at `GET /avatar-colors`); a GitHub photo replaces the circle.
- **What a chat shows**: only prose from the agent, written like a colleague, plus real attachments and one subtle working indicator. Tool calls and raw subagent output live behind an off-by-default debug setting.

## Install

- **Mac**: [arbos.life/download/mac](https://arbos.life/download/mac) gives you the newest stable DMG (Apple silicon and Intel, signed and notarized). The app updates itself; an **Update** button appears in the rail when a new build exists.
- **iPhone**: join the public beta on [TestFlight](https://testflight.apple.com/join/ec5fWCuJ).
- **Terminal**: `cargo install --path crates/arbosctl`, or take the static `arbosctl` from a [release](https://github.com/unarbos/arbos/releases).
- **Your own server**: see [docs/self-host.md](docs/self-host.md) — relay, machines, image, pointing the apps at it.

## Build from source

Rust stable (pinned in `rust-toolchain.toml`), Node 22.19+ and pnpm 9 for `harness/`.

```bash
cargo build --workspace
cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
cd harness && pnpm install && pnpm -r typecheck
```

Run the Mac app against the hosted relay, a fixture, or your own relay:

```bash
cargo run -p arbos-mac                                  # https://relay.arbos.life (compiled-in default)
ARBOS_RELAY_URL=fixture cargo run -p arbos-mac          # seeded chats + scripted agent, no relay needed
ARBOS_RELAY_URL=ws://127.0.0.1:7700 cargo run -p arbos-mac   # a local relay
```

The quickest full stack with no cloud account is the **process box**: the relay runs `arbos-agentd` as a child process on your own host and every tool, the pi main agent and the subagents run for real. Steps in [docs/self-host.md §2](docs/self-host.md#2-first-run-relay--process-box-on-one-host).

## Features

- **Questions and widgets** — `ask_user(question, widget?)` is the one way an agent asks you something. With a widget (`choice`, `multi_select`, `text`, `date`, `form`) the chat shows a card you answer with the mouse or keyboard; `arbosctl chat answer … --answer-json` does it from a terminal.
- **Approvals** — risky actions wait for you: outbound messages (`draft_message`), sending connectors, vault requests, commands on your own computer, destructive shell commands. Each becomes a card with Approve / Reject; decisions are audited (`arbosctl audit <project>`).
- **Voice** — hold the mic to talk; the transcript lands in the composer or is sent directly in Voice mode, and replies are read aloud in the voice you picked for that agent (Settings → the agent's profile). Speech goes through the relay so the OpenAI key never reaches a device.
- **Calls with agents** — call a project agent from the chat header. One-to-one it is a full voice conversation; in a group call it hears everyone and speaks only when it has something to add, using the same tools as the text agent.
- **Memory** — every agent keeps its whole history and a background index of it; before each reply a small decision model picks the threads, episodes and tool results that matter, so you never repeat yourself and nothing is summarised away from the thread.
- **Files and images** — the agent produces documents and media in the workspace (python-pptx, python-docx, openpyxl, reportlab, Pillow, ffmpeg) and hands them over with `deliver_file`; `generate_image` makes pictures. They arrive as attachment cards with Download.
- **Connectors** — Gmail/Calendar, Slack, GitHub and Harness Comms, brokered by the relay so tokens never enter a machine. Writes and sends go through an approval card first. Settings → Connectors.
- **Routines** — prompts that run while you are away: on a cron schedule, when a webhook is called, or on an event. `arbosctl routines create <project> --name "Stand-up" --prompt "…" --cron "0 9 * * 1-5"`; Settings → Routines.
- **Skills** — Markdown how-tos shared across projects; the agent picks up to three relevant ones per turn and can save new ones. `arbosctl skills list|show|add`; Settings → Skills.
- **Channels** — talk to an agent from Slack, Telegram, WhatsApp or Beeper: mint a code in Settings → Channels and send `/pair <code>` from that chat. Unpaired senders get one hint and never reach a machine.
- **People** — direct messages and shared projects with other Arbos users; invite someone to a project and their devices join its chats, or add a project's agent to a DM.
- **Voice and video calls between people (self-hosted LiveKit)** — start a call from the header of a DM or a shared project; the relay decides who may join and mints the tokens, media goes through [your own LiveKit server](docs/self-host.md#9-calls-livekit) and never through the relay.
- **Profile pages** — tap a person, an agent or a group to open its page: who it is, what it works on, members, and the groups you have in common.
- **Group chats** — one chat shared by several projects' agents, with loop guards (depth, cycles, rate). `arbosctl group create --host Alpha --with Beta,Gamma`.
- **Vault** — logins and API keys the agent may *use* but never *see*: values are encrypted on the relay, the agent only handles `arbos-secret://` placeholders, and every use is a request you approve. Settings → Vault.
- **Your computer** — `arbos-local` runs on your own machine so an agent can run a command or read a file there, each behind an approval card. Settings → This Mac.
- **Notifications** — Mac and iPhone notify you when an agent finishes or a person writes, with the agent's avatar as the sender picture; nothing else.

## Releases

Every green push to `main` becomes an installable build (`.github/workflows/release.yml`): **edge** builds `edge-<version>.<n>` with the DMG, the updater tarball, static Linux `arbos-relay` / `arbos-agentd` / `arbosctl` and an `appcast.xml`; a `v*` tag publishes the **stable** release the download link and the stable update feed point at. Payloads are signed with an Ed25519 key (`crates/arbos-update`, public half committed); the app verifies the signature before swapping itself. The iPhone app goes to TestFlight from every push to `main` (`.github/workflows/ios.yml`). The Mac job runs on a self-hosted Mac runner (`scripts/runner/`) or `macos-15`.

## Contributing

`.github/workflows/ci.yml` runs on pushes and pull requests: `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`, the harness typecheck, shellcheck and actionlint. Bug reports and screenshots from the apps' **Send feedback** land as issues in this repository and feed an automated fix loop.

The hero image at the top is built from real screenshots of the current Mac and iPhone builds; regenerate it with `scripts/readme-hero/` (capture scripts plus `compose.py`).

## History

This tree replaced the previous Arbos on 2026-10-15. The earlier `main` — the zeron-based desktop app, the fx harness and the phase-6 phone — is kept as branch `backup/main-2026-10-15`; the generation before that (the first mobile app, voice server, deploy scripts) is branch and tag `legacy-2026-10`. Their releases (`v0.1.x`, `v0.2.x`, the old `edge`) stay on GitHub.
