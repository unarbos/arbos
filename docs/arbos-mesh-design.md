> **RECOVERED from the repository mirror — close to the original but not identical.** Source: `docs/design/arbos-mesh-design.md` in `unarbos/arbos` at commit `a4a466f` (2026-09-15 19:21 UTC). That mirror is a repository-adapted copy, so some store-relative links and path suffixes differ from the lost original (24,319 bytes there against 23,984 in the store's last listing). Three passages written on 2026-09-16 are recovered verbatim and appended at the end of this file, out of their original position, because their anchors do not exist in the mirrored text.
>
> The original was lost together with the whole `docs/` directory on 2026-09-16 between 07:43 and 09:01 UTC. Restored by the store-recovery worker `bc-0b112226-cf98-5cab-92c3-2671518dd9b9`. Cause, timeline and the full recovery inventory: `internal/store-docs-loss-2026-09-16.md`.

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
| 2 | **Read by address**: `read`/`ls` on `arbos://`; `Hooks::store_read/list`; kernel client with a 10 s timeout and loud failure; the hub refuses `none` at `/attach` and passes the effective role; the private-by-default switch; `arbos-kernel store read|ls <address>` for people | `arbos-engine::tools::{fs,mod}`, `access`; kernel `hub_link`, `sched`, `store_cmd`; `arbos-hub` | [PR #254](https://github.com/unarbos/arbos/pull/254), stacked on #251 |
| 3 | **Write by address**: `put`/`written` frames with compare-and-swap on the content hash; receiving rules in `files.rs` (root-owned pages and protected files refused, shared folders only, tmp + rename); `write`/`edit` on addresses (edit = fetch, apply locally to a copy, put with the read hash); `store put` | `wire`, kernel `files.rs`, `serve.rs`, engine | same PR, second commit |
| 4 | **Briefs by address**: `address_brief` rewrites store paths in a remote kickoff to the parent's addresses; `first_prompt` names the parent's store and the child's own; `Output:` lands in the parent's store; `Record.store`; the done message names the child's store root with a `read` example. Hub road only; ssh road unchanged | `arbos-core::hub`, `remote.rs` | [PR #257](https://github.com/unarbos/arbos/pull/257), stacked on #254 |
| 5 | Later: opt-in stale cache for offline reads; chunked media `put`; Cloudflare Access identities replacing tokens | | not planned yet |

### Decided (2026-09-15, via the coordinator; Jacob may veto)

1. **A remote owner may not write another node's `notes.md`.** Root-owned pages stay the local root's; peers propose with `say`. Enforced in `files::put` whatever the peer's role (`put_tests::a_put_obeys_the_places_own_rules`).
2. **`mesh` is the default while every token on the hub is Jacob's; `private` the moment another person's token joins.** Built into the hub (`Auth::default_share`), not left for later.
3. **A remote child's deliverables land in the parent's store by address.** The parent's store is the Project Jacob opens. The child's own `agents/root/` record stays where it runs. Slice 4 makes the brief say so.


---

## Recovered additions of 2026-09-16 (appended out of position)

These three passages were written into `docs/arbos-mesh-design.md` on 2026-09-16 at 02:12, 07:34 and 07:55 UTC, after the repository mirror above was last updated (2026-09-15 19:21 UTC). Their text is recovered verbatim from the mesh worker's own edit commands. Their original anchors do not exist in the repository-adapted text above, so they are appended here rather than spliced into the wrong place. The mesh worker (`bc-22d20d79-de36-524a-ae31-3e1c44c03b98`) should fold them back where they belong.

### Deployment state (2026-09-16)

- Pod hub rebuilt from `main` + [PR #275](https://github.com/unarbos/arbos/pull/275): frames are relayed as raw JSON, so a kernel wire change never needs a hub redeploy; junk is refused with a reason. Tokens rotated on 2026-09-16 (`credential` = desktop, new `client-phone` = iPhone; item `6uihrhmgfwncp3jz3vxtfxklhi`). The `.arbos/machines/` roster and `hello.store` are live.
- ArbosLife `~/arbos-hub` kernel and worker run the `main` build (`put` with bytes, `history before`, addresses).
- `hub-api.arbos.life` still resolves to another origin: the CNAME to `7012ad90-12a5-4347-acc4-5b1f2760f399.cfargotunnel.com` is still Jacob's.

**ArbosLife path standing (2026-09-16 07:25 UTC), pod untouched.** Under `/home/const/arbos-hub/`: `hub/` (the #275 hub build, same `hub-server.toml` as the pod, `127.0.0.1:7010`), `phone/` (a copy of Jacob's phone kernel store from the pod, the `main` kernel on `127.0.0.1:7788`, registered on the local hub as `arboslife/phone`, model `google/gemini-2.5-flash`), `tunnel/` (connector token of the new named tunnel `arboslife-hub`, id `4bfb0184-123a-47cd-a061-019ffbd0bd0e`, ingress `hub-api` → 7010, `kernel-api` → 7788), `stack.sh up|status|down` (tmux sessions `hub`, `hub-quick`, `phone`, `phone-quick`, `cf-tunnel`). Interim quick-tunnel URLs in the vault item as `arboslife-hub-url` and `arboslife-kernel-url`. Proven from outside with real turns on both the direct and the hub path. Cutover plan and what remains: `internal/features-inbox/2026-09-16-hub-and-phone-kernel-on-arboslife-cutover.md`. Not yet: the sessions do not survive an ArbosLife reboot (no `@reboot`/systemd entry, to stay out of Jacob's crontab until he says so). **07:55 UTC: the pod is a forwarder** — its quick tunnels are unchanged but `7788`/`7010` behind them are ArbosLife's (ssh `-L`, `/root/arbos-hub/fwd.sh`); the pod's own phone kernel and hub are stopped, the voice gateway attaches to the ArbosLife kernel over `wss://`, and every ArbosLife registrant is on the local hub. The pod can be released once the phone build and the `mac` machines carry the ArbosLife addresses (or the CNAMEs exist); only the voice model then remains on rented hardware.

**Feedback place and scoped tokens (2026-09-16 18:35 UTC).** `arbos://arboslife/feedback/` (`~/arbos-hub/projects/feedback`, `[share] mode = "mesh"`, `cap_usd = 5`) receives the desktop's reports under `internal/feedback/<utc>-<n>/`. Two tokens of their own users — `client-desktop-feedback` (writer) and `client-parity-rig` (reader) — are writer/reader there and `none` on every other project, because with three users on the hub an unset project now defaults to `private` (decision 2, live). Details: `internal/features-inbox/2026-09-16-desktop-feedback-hub-delivery-answer.md`.

**Reachable machines, as of 2026-09-17 04:06 UTC.** ArbosLife (`const@204.12.171.6`) answers to the cloud VM's `~/.ssh/arbos_agents` key through the `arboslife` ssh alias. Templar (`const@204.12.168.71`) had **no** alias and was not reachable until that sweep: the default key was refused (`Permission denied (publickey)`), the same `arbos_agents` key passed once named explicitly (`ssh -i ~/.ssh/arbos_agents const@204.12.168.71`), and its ED25519 host key was accepted then for the first time. Templar has no `~/.config/arbos/hub.toml`, so nothing there is on the hub; its kernels are the QA loop's test places. The voice pod (`voicepod` alias, `root@216.243.220.25:40300`) runs no `arbos-kernel` or `arbos-hub` since the forwarder cutover. Jacob's Mac and `chakanaone` are not reachable from cloud VMs. The stale-binary sweep that found this: `internal/mesh-stale-binary-sweep-2026-09-17.md`.

**Hub on ArbosLife, as of 2026-09-17 12:09 UTC:** built from `main` `cbbe9922`, deployed swap-first (file renamed, then the loop's process stopped; the loop relaunched it in one second; all seven registrants back within five). It now carries #394 (every log line stamped with its UTC second), #417 (a refused peer is released the moment it hangs up, 2 s ceiling; the two refusals that still closed bare — an unregistered machine, a failed claim — now wait too) and #411 (the roster row is no longer one build per machine: `/list` has `builds[]` with `role`, `project`, `git_sha`, `built_at` per registrant and `worker: true`, and the top-level `git_sha` is gone — a client that read `machine.git_sha` must read `builds[]`). The row today shows the phone kernel on `efcab58f` from 16 Sep 17:17, the oldest registrant; it is not stale (its file is intact) but it is behind, and moving it is a phone-cutover matter, not a sweep one.

**The roster proved in production, 2026-09-17 12:28–12:34 UTC** (throwaway machine `podtest`, kernels on dev build 1410 = `cbbe992`, registered through the public tunnel; nothing of Jacob's touched):
- *Mixed builds → no machine-wide build.* Three kernels, two on `cbbe992` and one on `0f2a8bc6`: the row carried `builds[]` with each, and no top-level `git_sha`/`built_at` (the keys are absent, so a client reading the old field gets nothing, not a wrong sha).
- *A file swapped under two idle kernels → they healed themselves.* Swap at 12:31:30.7; at 12:31:32.37 both logged `binary_gone` then `reexec restarting onto <path>`; same pids, re-registered 0.2 s later; the hub saw unregister/register. On this build the roster flag is the fallback, not the first line.
- *A process that cannot re-exec → the roster names it.* The file replaced by a non-executable copy: `reexec_failed … Permission denied; the old image serves on`; 30 s later `hub_build_revised binary_gone=true` on the open link, and `/list` read `binary_gone: true` on **those two registrants and on the machine**, while a sibling on its own file stayed clean (no key) — the case that hid ArbosLife's daemon for 5.5 h, now visible from the roster.
- *Refusals arrive through the tunnel.* `attach/nowhere` with a reader token: `{"type":"error","detail":"hub: no machine named \"nowhere\" is registered (known: podtest, arboslife)"}` in 0.4 s and a clean close; a peer that held the socket was closed at 2.1 s; a private project answered `no access to arboslife/subnet120: the project is not shared with you`. No bare close anywhere.
Nothing of the three was still wrong. Test kernels and the pod's `~/mesh` removed after; the `podtest` machine row stays in `hub-server.toml` until the pod expires (marked throwaway).

**Staged for Jacob, 12:45 UTC — two things, one word each:**
1. *Reboot survival.* `~/arbos-hub/systemd/` on ArbosLife holds eight user units and `arbos-mesh.target` (hub, both quick tunnels, the named tunnel, phone kernel, `demo`, `feedback`, worker), each `Restart=always`, logs appended under `~/arbos-hub/logs/`, verified with `systemd-analyze --user verify`; `install.sh` enables the target, stops the tmux loops (loops before kernels, so nothing relaunches into a lock), then starts it; `uninstall.sh` goes back. **Linger is already on for `const`** (`loginctl show-user const -p Linger` → `yes`) and the user manager is running, so no root and no crontab; the only missing thing is his approval to run at boot on his box. **His word: "install the mesh units"** — then `bash ~/arbos-hub/systemd/install.sh`, one minute, and a reboot afterwards is survivable.
2. *The voice stack and the pod's expiry (2026-09-20).* What the pod's 96 GB GPU actually holds: ~87 GB is the NVIDIA NIM stack for the Nemotron duplex voice model (tritonserver + two vLLM engines, `/opt/nim`); the voice server the phone uses today (`voice_server`: whisper `large-v3-turbo`, Kokoro TTS, silero VAD, replying through OpenRouter and attaching to the ArbosLife phone kernel) uses **2.6 GB**. Everything movable is sealed at `~/arbos-hub/voice-move/arbos-voice-2026-09-17.tar.gz` on ArbosLife (3.8 GB: `src/`, `models/`, `hf/`, `bin/cloudflared`, `env`, `tunnel.token` for the `arbos-voice` named tunnel; 0600) with `bring-up.sh` beside it that stands the server and the tunnel up on any Linux host with an 8 GB GPU, or CPU. **His word is one of three, with prices checked 2026-09-17 13:15 UTC** (RunPod secure tier unless said; the pod today is $1.19/h ≈ $860/month): **(A)** "voice server only" — an RTX 4090 24 GB at $0.74/h ≈ $530/month (community tier $0.34/h ≈ $245/month), the phone's calls keep working exactly as today, and the Nemotron duplex model goes with the pod; **(B)** "keep the duplex model" — it used 87 GB of the pod's 96, so an 80 GB A100/H100 does not fit it as deployed; a 96 GB GH200 is $2.29/h ≈ $1,650/month on Lambda, plus redeploying NIM (`/opt/nim/deploy_s2s_model.sh` on the pod is the recipe); **(C)** "extend the pod" — nothing moves, $860/month continues, and the decision comes back at the next expiry. Two things die with the pod that are not the voice stack and need no GPU decision: the forwarders behind the phone build's old quick-tunnel URLs (the iOS loop's cutover to ArbosLife addresses must land before the 20th), and `hub-api`/`kernel-api`/`voice-api.arbos.life`, whose DNS still points at the pod's tunnel — the CNAME question above, now with a date.

**The hostname, as of 2026-09-17 06:20 UTC — one question for Jacob.** The named tunnel `arboslife-hub` (`4bfb0184-123a-47cd-a061-019ffbd0bd0e`) is healthy with four connectors and already carries ingress for `hub-api.arbos.life` → 7010 and `kernel-api.arbos.life` → 7788. What is missing is only DNS: our Cloudflare API token manages tunnels but has no DNS permission on any zone (`Authentication error` on `dns_records` for `arbos.life`, `affine.io`, `constantinople.cloud`), and no other DNS credential exists in the vault. Today `hub-api.arbos.life`, `kernel-api.arbos.life` and `voice-api.arbos.life` all resolve to another origin of Jacob's (a FastAPI service: `/healthz` → `{"status":"ok"}`, `/list` → `{"detail":"Not Found"}`, `/` → an "Arbos" HTML page) — not the hub, not the pod. A client configured with `wss://hub-api.arbos.life` gets a 404 on every attach. **Jacob's one action**: in Cloudflare DNS for `arbos.life`, make `hub-api` and `kernel-api` proxied CNAMEs to `4bfb0184-123a-47cd-a061-019ffbd0bd0e.cfargotunnel.com` (after confirming nothing of his depends on those two names today), *or* add `Zone → DNS → Edit` on `arbos.life` to the account API token (vault `pz4t7dalfdaldzi7el6rivobsi`) and the mesh worker does it. Until then the working addresses stay the quick-tunnel URLs in the vault (`arboslife-hub-url`, `arboslife-kernel-url`), which change when `cloudflared` restarts.

**Where the hub and the voice server should live (the pod expires 2026-09-20).** The pod costs $1.19/h ≈ $860/month and only the voice model needs its GPU (RTX PRO 6000, 96 GB). ArbosLife has no GPU, 128 cores, 503 GB RAM, 958 GB free, and is Jacob's, always on, free. Proposal: (1) hub and the phone kernel move to ArbosLife under `~/arbos-hub/` with a **second named Cloudflare tunnel** of their own (free; `hub-api` and `kernel-api` ingress there; two connectors on one remotely-managed tunnel would split requests between hosts, so a separate tunnel is the clean way) — cost $0, and the hub stops moving when a rental ends; (2) the voice server stays on a rented GPU, but a 24–48 GB card at $0.30–0.70/h ($220–500/month) on Lium or Prime Intellect is likely enough once the duplex model's real VRAM need is measured — keep its tunnel (`voice-api`) on that box. Net: from ≈$860/month to ≈$300/month, with the hub permanent. The moves are a morning's work each; what they need from Jacob is the two CNAMEs.
