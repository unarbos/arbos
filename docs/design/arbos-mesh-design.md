# Arbos mesh: every Arbos reaches every other Arbos

Written 2026-09-13. Code: repo `unarbos/arbos`, branch `cursor/arbos-mesh` (built on `cursor/release-integration-52cd`), [PR #93](https://github.com/unarbos/arbos/pull/93) against `rust`.

## The idea in one paragraph

One small service, `arbos-hub`, sits at a public address. Every kernel and every worker Jacob runs connects **outbound** to it over a WebSocket and registers a machine name. Nothing on a laptop needs an open port. A client (desktop, phone, another kernel) then reaches a kernel **by machine name**, or **claims** a machine's worker so it starts a kernel in a checkout that machine already has. This is Cursor's self-hosted-worker model, applied to Arbos as it exists today.

## Part 1. How Cursor's self-hosted workers work (research, 2026)

Sources: [Self-Hosted Machines](https://cursor.com/docs/cloud-agent/self-hosted), [My Machines](https://cursor.com/docs/cloud-agent/self-hosted/my-machines), [Team Pools](https://cursor.com/docs/cloud-agent/self-hosted/pool), [Choose where Cloud Agents run](https://cursor.com/docs/cloud-agent/self-hosted/choose-runtime), [blog: Run cloud agents on machines you manage](https://cursor.com/blog/self-hosted-machines), [forum: routing for monorepos](https://forum.cursor.com/t/self-hosted-routing-for-monorepos/166117).

Terms:

- **Worker**: a daemon on your machine (`agent worker start`). It is the place tools run: file edits, terminal commands, browser, local MCP servers.
- **Agent loop**: the model calls, planning, and inference. In Cursor this stays in Cursor's cloud. Only tool execution moves to the worker.
- **Claim**: the moment a chat is bound to one worker. From then on every tool call of that chat goes to that worker.
- **Pool**: a named queue of workers (`gpu`, `ios`). A request waits in the pool until a worker claims it.
- **Label**: a `key=value` word on a worker. Routing matches labels. `repo=` (from the checkout's git remote) and `pool=` are reserved.

How it works, step by step:

1. **Outbound only.** The worker opens one long-lived HTTPS connection to `api2.cursor.sh`. Cursor never connects into your network. No inbound port, public IP, or VPN.
2. **Registration.** On start the worker sends: a name (`--name my-devbox`), one or more workspace roots (`--worker-dir`, each with a `repo=` label derived from its git remote), free labels (`--label team=backend`), a pool (`--pool gpu`, optional), and capabilities (`--computer-use`).
3. **Auth.** *My Machines* (personal): browser login (`agent login`), a personal API key, or a short-lived user-scoped token (`POST /v1/sub-tokens`, `--auth-token-file` for rotation). *Team Pools* (Enterprise): a service-account API key. A team admin must enable "Allow Self-Hosted Machines" or "Require".
4. **Routing.** A request names its target: `worker=my-devbox` (a personal machine; must belong to the requesting user and serve the target repo), or `pool=gpu` + `repo=owner/name` (any worker in that pool with that repo label). No match means the request fails; it never falls back to another environment.
5. **Claim.** The worker (or a **controller**, `agent worker controller --spawn ./spawn.sh`) watches `GET /v0/private-workers/pending-requests` (+ SSE `/stream`), then `POST /v0/private-workers/claim {id, workerId}`. A second claim on a live claim is rejected; `…/claims/<id>/release` frees it.
6. **Execution.** Tool calls stream to the worker; the worker returns file contents, terminal output, diffs, screenshots. The checkout, build cache, and machine-local credentials stay on the machine. Hooks `sessionStart`/`sessionEnd` fire on claim and release.
7. **Shared assignment.** Without `--pool`, one machine may serve several agents at once (My Machines). In pool mode one agent claims one worker at a time; `--idle-release-timeout` (default 3600 s) keeps the worker bound after the session for follow-ups, then the CLI exits 0 so a supervisor can recycle the machine. `workerReadyTimeoutSeconds` gives an offline claimed machine a reconnect window; hibernation keeps workspace locality.
8. **Limits.** 200 workers per user, 1000 per team. Health via `--management-addr` (`/healthz`, `/readyz`, `/metrics`).

The one difference that matters for Arbos: in Cursor the *agent loop* stays in the cloud and only tools run on the worker. In Arbos the kernel **is** the agent loop and lives where the `.arbos/` folder lives (the "agent's home" principle). So the Arbos worker does not execute tools for a remote loop; it **starts a whole kernel** in the machine's checkout, and the hub joins the claimer to that kernel as a client.

## Part 2. The Arbos design

### Pieces

| Piece | What it is | Where |
| --- | --- | --- |
| `arbos-hub` | The meeting point. Registry + router. Plain HTTP/WebSocket on loopback; a tunnel does TLS. | `crates/arbos-hub` (binary) |
| Hub wire | `HubFrame`: `register`, `registered`, `roster`, `open`, `frame`, `close`, `claim`, `claimed`, `error`, `unknown`. Attach `Frame`s ride inside `frame` on numbered channels. | `crates/arbos-core/src/hub.rs` |
| Kernel link | `arbos-kernel serve --hub URL --machine NAME [--project NAME]` (or `~/.config/arbos/hub.toml`). One outbound socket, reconnect with backoff, hub clients served as virtual attach clients. | `crates/arbos-kernel/src/hub_link.rs`, `attach.rs::HubChannel` |
| Worker | `arbos-kernel worker --dir DIR [--cap gpu] [--label word]`. Registers the checkouts under `DIR`; on a claim starts `arbos-kernel serve <checkout or worktree> --hub …`. | `crates/arbos-kernel/src/worker.rs` |
| Roster on disk | `.arbos/machines/<name>.toml` + `.arbos/machines.md`, rewritten on every hub push; one prompt line ("Hub machines: …"). | `arbos-core::hub::write_roster`, `arbos-engine/src/prompt.rs` |
| Routes in tools | `spawn host=<name>`: ssh when `machines.toml` has it, else the hub claim. `say to=<machine>/<agent>` or `<machine>/<project>/<agent>`. `arbos-kernel attach --hub <machine>[/<project>]`. | `remote.rs`, `tools.rs`, `cli.rs` |
| Deploy | Example `hub-server.toml`, client `hub.toml`, supervisors, worker launcher, tunnel ingress. | `deploy/hub/` |

### Routes on the hub

- `WS /register` — a kernel or worker. Token must be a `[[machine]]` token; the `register` frame's `machine` must match it. Kernels register a `project`; workers register their `projects` (checkouts). The hub pushes `roster` to every registrant on every change and pings every 30 s.
- `WS /attach/<machine>[/<project>]` — a client. Any token. The hub opens a channel on the kernel's socket (`open {chan, who, role}`), then copies frames both ways. The client sees exactly what a direct WebSocket to the kernel shows (`hello`, `snapshot`, replay, live frames). With one kernel on the machine, `project` may be omitted.
- `WS /claim/<machine>` — a client (owner or writer). First frame `claim {project, isolate, from}`. The hub forwards it to the machine's worker, waits for `claimed`, waits for the new kernel to register (90 s), replies `claimed`, then the **same socket becomes the attach** to the new kernel.
- `GET /list` (token) — the roster as JSON, for the desktop Opener and scripts. `GET /healthz` — no token.

### Auth

`hub-server.toml`: `[[machine]] name token|token_env` and `[[client]] name token|token_env role`. Per-machine tokens, 16+ chars, constant-time compared. A machine token also attaches as `machine:<name>` with the owner role (one of Jacob's machines is Jacob). The kernel receives `who` and `role` from `open` and applies its existing `access::Role` rules per frame. Later: a `trust = "cloudflare-access"` mode where identity comes from the `Cf-Access-Jwt-Assertion` header, as the file-system design specifies; the token mode stays for people without Cloudflare.

### `spawn host=<machine>` through the hub

1. `machines.toml` has the name → the ssh road (unchanged, #33/#36/#86).
2. Else `.arbos/machines/<name>.toml` exists and says `worker = true` → the hub road: connect `/claim/<machine>`, send `claim {project: <this place's folder name>, isolate: true, from: "<my machine>/<parent>"}`.
3. The worker requires an **existing checkout** `DIR/<project>`. It never syncs code (Cursor's rule: a request for repo A never runs on a checkout of repo B). Missing checkout → a clear refusal naming the checkouts it has.
4. `isolate` → `git worktree add <checkout>/.arbos/worktrees/<claim> -b arbos/<claim>` (the same `worktree::create` local `spawn isolate=worktree` uses). The kernel serves the worktree as project `<project>--<claim>`.
5. The worker starts `arbos-kernel serve <place> --hub … --machine … --project …` in its own process group, with the worker's own `config.toml` (model key stays on that machine) and returns `claimed`.
6. The claimer's socket is now attached to the child kernel. It sends `set_mode` (the parent's permission mode: auto/ask/plan applies remotely too) and the brief as a `user` frame to `root`.
7. Relay: the same code as the ssh road (now carrier-agnostic). On the remote root's `turn idle`, the hub road sends `history {agent: root, since: mirrored}` and mirrors the `replayed` lines into the local child folder; the last assistant text goes to the parent as a `say` from the child and queues a turn. `.arbos/remotes.json` records `route: "hub"` and `project`, so a kernel restart re-attaches via `/attach/<machine>/<project>`.

### `say to=<machine>/<agent>`

Only when `<machine>` is on the on-disk roster (a local agent whose name has a slash stays local). The kernel attaches through the hub, sends `user {agent, text: "[<my machine>/<sender>] …"}`, waits 1.5 s for an `error` naming the agent, closes. Receipt tells the sender the reply comes back the same way (`say to=<my machine>/<sender>`). Demonstrated both directions.

### Discovery on disk (file-system principle)

Every kernel with a hub writes `.arbos/machines/<name>.toml` (`name`, `user`, `host`, `labels`, `capabilities`, `worker`, `[[projects]] name/place/live`, `since`, `hub`, `seen`) and `.arbos/machines.md`. Files of machines that left are removed: the folder mirrors the hub, not a history. An agent finds other machines with `ls .arbos/machines/`; the panel's Resources can list the same files; the prompt carries one roster line.

### Reference deployment

- Hub: one `arbos-hub` process on a small always-on box, bound to `127.0.0.1:7010`, with a Cloudflare Tunnel (or any TLS reverse proxy) in front of it (`deploy/hub/hub-run.sh`, `deploy/hub/hub-quick-tunnel.sh`). The hub keeps its roster in memory; nothing else lives on that box.
- Tokens: one `[[machine]]` token per machine and one `[[client]]` token per client in `hub-server.toml` (`deploy/hub/hub-server.example.toml`). Keep them in a password manager; each machine reads its own from `~/.config/arbos/hub.toml` or `ARBOS_HUB_TOKEN`.
- Machines: a GPU box running a kernel and a worker (`deploy/hub/worker.sh`), a laptop running the desktop and a worker, and a cloud VM running a kernel. All three registered outbound through the tunnel.

### What the demo proved

1. `1-roster.txt`: `/list` through the tunnel shows `arboslife (worker; kernels: demo; linux, x86_64, 128-cores, gpu)` and `cloud`; no token → 401; kernel A has `.arbos/machines/{arboslife,cloud}.toml` and `machines.md`.
2. `2-spawn-via-hub.txt`: from A, `spawn host=arboslife` → hub claims the worker → worktree `projects/demo/.arbos/worktrees/c616190-1` on branch `arbos/c616190-1` → kernel registers as `demo--c616190-1` → A attached on channel 1 → the child reported ArbosLife's hostname and `nproc` = 128 back to A's root as a `say`; `remotes.json` shows `route: hub`. No ssh between A and B.
3. `3-say-both-ways-and-attach-by-name.txt`: `arbos-kernel attach --hub arboslife/demo` from the VM followed kernel B's live turn; A's `say to=arboslife/demo/root` reached B; B answered with `say to=cloud/root`, which landed on A's root as `[arboslife/root] pong from computeinstance-…`.
4. Hub restart: all three registrants reconnected within seconds (backoff 2 s).

### Not in this slice

- Cloudflare Access identity (JWT) on the hub; today tokens only.
- Desktop and phone UI for the roster.
- Pools and labels-based routing (Cursor's `pool=gpu`): today a claim names one machine. Labels/capabilities are carried and shown; the model picks the machine from the roster line.
- Worker-side clone when a checkout is missing (Cursor's `--clone-git-repos`). Refused with a clear message instead.
- `/list` for anonymous visitors; hub metrics; a hub supervisor that survives a pod reboot (tmux only).
- The hub keeps the roster in memory only. Restart = registrants re-register; nothing else is lost.

### Adding a machine

1. Build the kernel on that machine: `cargo build --release -p arbos-kernel`.
2. Put a model key in its `config.toml` (`arbos-kernel setup`) and the hub URL, machine name, and token in its `hub.toml` (`deploy/hub/hub.example.toml`).
3. Clone the projects the mesh may work on into `<dir>/projects/<name>` (the name must match the place's folder name on the calling side).
4. Start the worker:

```bash
arbos-kernel worker --dir ~/arbos-hub/projects --machine mac --cap xcode --cap ios
```

Other kernels now see the machine in `.arbos/machines/` and can `spawn host=mac`, `say to=mac/<project>/<agent>`, or `arbos-kernel attach --hub mac/<project>`.
