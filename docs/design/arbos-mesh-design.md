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
## Part 3. Federated stores: context crosses machines by address (2026-09-15)

Jacob's decision: **every node keeps its own `.arbos/` on its own machine.** No central store, no copying the parent's store around. When nodes discover each other they learn *where* each other's stores are, and read and write across the link.

Closes gap 1 of `internal/projects-post-gaps-2026-09-15.md` (a remote worker sees the code but never the Project's memory) with federation instead of sync, mount, or relay.

Terms:

- **Node**: one kernel serving one place on one machine. It owns that place's `.arbos/` (its **store**).
- **Address**: a name for a file or folder in some node's store that any node on the same hub can resolve.
- **Fast path**: the local store, read and written as plain files, as today. The address form is only for a store on another machine.

### 1. The store address

```
arbos://<machine>/<project>/<path>
```

- `<machine>`: the hub roster name (`MachineInfo.name`; `cloud`, `arboslife`, `mac`).
- `<project>`: the name the kernel registered under (`ProjectInfo.name`; the place's folder name or `--project`; a worktree kernel is `<project>--<claim>`).
- `<path>`: relative to that place's `.arbos/`: `notes.md`, `docs/project-context.md`, `internal/x.md`, `media/mesh/1.txt`, `agents/root/plan.md`. Empty path = the store root (a listing).

The same two names already identify a node in `say to=<machine>/<project>/<agent>` (`MeshTarget`) and in `hello` (#233 puts the project's face on `hello`; `hello.store` now carries the node's own address). No second naming scheme: `arbos://cloud/demo/docs/x.md` is the URL form of the `machine/project` tuple plus a store path. Names are Jacob's; renaming a machine renames its addresses, on purpose.

Why the store only: the checkout is *work*, and each machine has its own (Cursor's rule too). Context is the store. An address never points into a checkout.

### 2. Discovery carries store locations and rights

The roster already lists machines and projects. Each `ProjectInfo` gains:

- `store`: the project's address root, `arbos://<machine>/<project>/`, filled by the hub.
- `share`: the project's sharing mode from its `project.toml` `[share] mode` (`private`, `mesh` = default, `open`), sent by the registering kernel or worker like the face is.
- `access`: what the *recipient* of this roster may do there: `owner`, `writer`, `reader`, or `none`. The hub computes it per recipient when it pushes the roster (rosters were already sent per registrant), and per requester on `GET /list`.

On disk, `.arbos/machines/<name>.toml` therefore shows every peer store and the node's rights on it, and `machines.md` renders `- arboslife — arbos://arboslife/demo/ (owner)`. `ls .arbos/machines/` answers "what stores exist on my peers and which may I touch".

### 3. Read and write across the link

- `read`, `ls`, `tail` (and `grep --scope`, later) accept an address. `write` and `edit` accept one too.
- Resolution order in the engine's path resolver: a plain path is the local store or checkout as today (fast path, unchanged). An `arbos://` address whose machine is *this* node's name resolves to the local file (fast path again). Any other address goes through a new `Hooks::store_read / store_list / store_write` the kernel implements: attach through the hub to `<machine>/<project>` (an existing `/attach` route) and send `read`/`tail`/`list` (#78/#88) or the new `put` frame.
- **Failure is loud.** A peer that is unreachable, or a hub that is down, returns a tool error: `arbos://arboslife/demo/notes.md: arboslife is not reachable through the hub (connect timed out after 10 s); nothing was read`. There is no cache to fall back to in this design, so an agent can never get stale or empty context and think it is current. (An offline read-through cache marked `stale` is a later, opt-in slice.)
- **Write** is `put {path, text, base_hash?}` → `written {path, size, hash}`: compare-and-swap on the sha-256 of the current content (`base_hash = ""` means "must not exist yet"); a mismatch returns `conflict {current_hash}` and the writer re-reads and retries. One writer per file at a time, no lost updates, and the receiving kernel does the write with its own atomic rules (`.tmp` + rename), so a remote `put` is indistinguishable from a local `write` to everyone else on that machine.
- The receiving kernel applies the **same rules as a local write**: `notes.md`, `docs/project-context.md`, `archived.md` are root-owned → a peer's `put` is refused with the same words (propose with `say to=<machine>/<project>/root`); `PROTECTED` files (`project.toml`, `access.toml`, skills, hooks) are never served or written; `agents/<id>/` is read-only from outside.

### 4. Permission

Two layers, as in the file-system design:

1. **Who you are**: the hub token (later the Cloudflare Access email). Every `hub-server.toml` row gains `user` (default `owner`, the hub's operator). A machine token is `user = owner` unless the row says otherwise; a person's client token names them (`user = "alice"`, `role = "writer"`).
2. **What you may do** on a project = `min(token role, project share mode)`: `private` → owner-user identities keep their role, everyone else `none`; `mesh` (default) → your token's role; `open` → at least `reader` for anyone the hub admits. Then the per-file rules above.

Today all of Jacob's machines hold `user = owner` tokens, so they read and write each other's `docs/`, `internal/`, `media/` freely, and none may rewrite another node's `notes.md` (that stays the local root's). When Alice connects with her own client token, a `mesh` project gives her `writer` on `docs/…`; a `private` one gives her nothing; and every file she writes lands with her name in the receiving kernel's log (`store_put who=hub:alice path=…`).

**The default flips by itself (decided 2026-09-15).** A project whose `project.toml` sets no `[share] mode` is `mesh` while every token on the hub belongs to one user, and `private` the moment a second person's token appears in `hub-server.toml` (`Auth::default_share`). So sharing the hub with another person cannot silently expose a project: everything unset closes, and Jacob opens what he means to share with one line. The hub enforces this at `/attach` (an identity with `none` is refused before any frame reaches the kernel) and hands the kernel the *effective* role, so a `writer` token on an `open` project is a writer there and nothing more.

### 5. What a spawn brief becomes

A kickoff brief already carries paths, not content (`Read first: .arbos/docs/project-context.md, then .arbos/notes.md`, `Output: … under .arbos/docs/`). For a remote child those paths are rewritten to the parent's addresses before the brief leaves:

```
Read first: arbos://cloud/demo/docs/project-context.md, then arbos://cloud/demo/notes.md
Output: deliverables under arbos://cloud/demo/docs/, notes under arbos://cloud/demo/internal/, captures under arbos://cloud/demo/media/<topic>/
```

and the first prompt tells the child: your parent's store is `arbos://cloud/demo/`; your own is `arbos://arboslife/demo--c1/`. A child then reads the Project's memory by address and cannot mistake its fresh worktree store for the Project. Rule: every store-relative token in a brief (`.arbos/…`, `docs/…`, `internal/…`, `media/…`, `notes.md`, `archived.md`) is prefixed with the parent's store address; anything else (code paths) stays as it is, because code is on the child's machine.

### 6. Where a remote child's outputs live

- **Deliverables** (`docs/`, `internal/`, `media/`): written by address into the **parent's** store, because that is the Project the user opens, and a worktree kernel's store is short-lived (archived, then removed). The `Output:` line says so (above). The done report names the addresses it wrote; the parent reads them as local files.
- **The child's own record** (its `agents/root/` folder: transcript, plan, jobs, images) stays on its machine. The parent's local stand-in already mirrors the transcript (#237, #239, #243); anything else the parent wants it reads by address: `arbos://arboslife/demo--c1/agents/root/plan.md`. The done report includes the child's store root so the parent can.
- Large media (a recording) over the link: `put` is one message today; a chunked `put` (`from`, `append`) is the follow-up when the first recording hits the cap.

### Slices

| # | Slice | Touches | Status |
| --- | --- | --- | --- |
| 1 | **Addressing and discovery**: `StoreAddress` parse/format; `ProjectInfo.{store,share,access}`; `[share] mode` in `project.toml`; hub computes `access` per recipient; `hello.store`; `.arbos/machines/` and `machines.md` show addresses and rights | `arbos-core::hub`, `project`, `wire`; `arbos-hub`; `hub_link`, `worker`, `serve` (one line) | [PR #251](https://github.com/unarbos/arbos/pull/251) on `main` |
| 2 | **Read by address**: `read`/`ls` on `arbos://`; `Hooks::store_read/list`; kernel client with a 10 s timeout and loud failure; the hub refuses `none` at `/attach` and passes the effective role; the private-by-default switch; `arbos-kernel store read|ls <address>` for people | `arbos-engine::tools::{fs,mod}`, `access`; kernel `hub_link`, `sched`, `store_cmd`; `arbos-hub` | [PR #252](https://github.com/unarbos/arbos/pull/252), stacked on #251 |
| 3 | **Write by address**: `put`/`written` frames with compare-and-swap on the content hash; receiving rules in `files.rs` (root-owned pages and protected files refused, shared folders only, tmp + rename); `write`/`edit` on addresses (edit = fetch, apply locally to a copy, put with the read hash); `store put` | `wire`, kernel `files.rs`, `serve.rs`, engine | same PR, second commit |
| 4 | **Briefs by address**: rewrite store paths in remote kickoffs; store sentence in `first_prompt`; done report names addresses | `remote.rs` (coordinate with the features agent: #243 mid-rebase, #237/#239 today), `tools.rs` | after 3; after #243 lands |
| 5 | Later: opt-in stale cache for offline reads; chunked media `put`; Cloudflare Access identities replacing tokens | | not planned yet |

### Decided (2026-09-15, via the coordinator; Jacob may veto)

1. **A remote owner may not write another node's `notes.md`.** Root-owned pages stay the local root's; peers propose with `say`. Enforced in `files::put` whatever the peer's role (`put_tests::a_put_obeys_the_places_own_rules`).
2. **`mesh` is the default while every token on the hub is Jacob's; `private` the moment another person's token joins.** Built into the hub (`Auth::default_share`), not left for later.
3. **A remote child's deliverables land in the parent's store by address.** The parent's store is the Project Jacob opens. The child's own `agents/root/` record stays where it runs. Slice 4 makes the brief say so.
