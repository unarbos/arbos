# Box Fleet Scaling & Tiered Suspend Specification (1,000+ Users)

This document specifies the compute, storage, room fan-out, and fleet-scheduling architecture to scale Arbos from ~80 always-on user sandboxes on a single host to **1,000+ registered users** (~100–180 concurrently active sandboxes within a 15-minute window) with sub-second to <2 s perceived wake latency and ~$0.50/user/month infrastructure unit economics (excluding LLM inference spend).

Companion documents:
- [`docs/self-host.md`](self-host.md) — relay and machine deployment reference.
- [`docs/crdt-sync.md`](crdt-sync.md) — room wire protocol, box lifecycle frames (`STATUS`, `WAKE`, `PRESENCE`), and catch-up semantics.

---

## 1. Problem Statement & Baseline Constraints

Every Arbos user owns one isolated Linux machine ("box") supervising all of their project workspaces (`/work/<project>`). Under the initial host-fleet architecture (`crates/arbos-relay/src/fleet.rs`, D89):

1. **Always-on policy blocks scale-to-zero:** With `ARBOS_ALWAYS_ON` enabled, `boxes::spawn_always_on` (`crates/arbos-relay/src/boxes.rs`) sweeps every 60 s and wakes any stopped sandbox. Even though `arbos-agentd` parks idle `pi` and subagent worker processes after 10 minutes (`PI_PARK_IDLE_MS = 10m` in `crates/arbos-agentd/src/agent.rs`), every user's gVisor sentry + `agentd` (~168 MB RSS) and loop mount (`/dev/loopN`) remain pinned 24/7.
2. **Row-based admission waitlists new sign-ups:** `fleet::capacity` sets `resident = count_boxes_of_provider("gvisor")` — counting database rows rather than live memory-holding sandboxes. Once 80 users have ever been provisioned, user #81 is waitlisted even when 75 of those 80 users are inactive.
3. **Docker-managed `runsc` breaks checkpoint/restore:** Running `docker run --runtime=runsc` unmounts the container's overlay root filesystem as soon as `runsc checkpoint` exits the sandbox, causing `runsc restore` to fail (`failed to stat mount "/var/lib/docker/rootfs/overlayfs/..."`).
4. **Three 1,000-user scaling traps on local NVMe & kernel state:**
   - **Empty ext4 metadata amplification:** Formatting a 128 GiB sparse `work.img` with default `mkfs.ext4` allocates **1.1 GB** of inode tables and journal blocks immediately. At 1,000 users, `1,000 × 1.1 GB = 1.1 TB` of disk is consumed before a single user writes one byte.
   - **Redundant RAM-checkpoint I/O:** Because `pi` and subagent workers already exit after 10 minutes idle, ~95% of boxes reaching 15 minutes idle hold zero user processes in workspace cgroups — only `agentd`, whose durable state already lives in `agentd.db`, Loro, and session JSONLs. Checkpointing and shipping 1,000 RAM dumps to NVMe and object storage wastes terabytes of writes and cold-restore bandwidth.
   - **Pinned loop mounts & `keep_open` rooms:** Keeping `/dev/loopN` attached while asleep leaks 1,000 kernel loop + `jbd2` threads. Inside `agentd`, `PROJECT_DOC_CHAT_ID` and `people/*` rooms are marked `keep_open` (`crates/arbos-agentd/src/sync.rs`), bypassing both the 15-minute idle close and `MAX_OPEN_ROOMS` because the relay's room-presence check (`room.has_box(&owner)`) is keyed per owner rather than per `(owner, room)`.

---

## 2. Measured Hardware & Runtime Baselines

Measured on a single reference bare-metal host (128 vCPU, 503 GB RAM, 1.3 TB NVMe ext4, `runsc` with `--platform=systrap`):

| Operation / Resource | Measured Value | Notes |
| --- | --- | --- |
| Idle box RSS (`agentd` + `runsc` sentry, `pi` parked) | **~168 MB** | ~0.004–0.03 CPU cores when room count is bounded |
| Active box RSS (`pi` + Prime/Claude subagents) | **0.5 – 2.0 GB** typical | Ceiling 16 GB (overcommitted with host pressure eviction) |
| `runsc checkpoint` write rate | **~0.81 s / GB** (~1.2 GB/s) | Linear from 30 MB (0.19 s) to 15.5 GB (12.61 s) |
| `runsc restore` read rate (process up) | **~0.20 s / GB** (~5.0 GB/s) | 0.12 s at 168 MB; 0.47 s at 2 GB; 1.59 s at 8 GB |
| End-to-end warm restore to HTTP healthcheck | **1.72 s** (at 1.5 GiB RSS) | Self-managed OCI bundle (`runsc` direct, no Docker) |
| Cloudflare R2 snapshot fetch throughput | **~186 MB/s** | ~8 s for a 1.5 GB checkpoint -> 10–16 s unprefetched cold wake |
| Checkpoint compression (`flate` / `zstd` on dirty heap) | **1.00x – 500x** | 1.00x on incompressible heap; cold SLA cannot rely on compression |
| Existing 49-box `/work` footprint (sparse ext4) | **25 GB real** vs. 168 GB apparent | ~0.51 GB real written data per user |

With 64 GB RAM and 16 vCPU reserved for `arbos-relay`, PostgreSQL/Synapse, LiveKit, and page cache, **439 GB RAM and 112 vCPU** remain for sandboxes — enough for **150–180 simultaneously `Warm` boxes** on a single host (~1,000–1,500 registered users at 10–15% 15-minute concurrency) once idle boxes release RAM, sockets, and loop mounts.

---

## 3. Target Box Lifecycle & Storage Tiers

Boxes run as **self-managed OCI bundles** directly under `runsc` (no Docker daemon in the sandbox path) and move across five explicit states:

| State | Entry Condition | Host RAM & CPU | `/dev/loopN` | Local NVMe Footprint | Object Store (R2) | Wake Latency Target |
| --- | --- | --- | --- | --- | --- | --- |
| **`Warm` (Active)** | Turn/job running or active < 10 min ago | Held (0.5–2+ GB) | Mounted | Elastic `work.img` | — | **0 ms** (connected) |
| **`Warm` (Parked)** | 10–15 min idle (`pi` & workers exited) | ~168 MB (`agentd` only) | Mounted | Elastic `work.img` | — | **~0.8–1.0 s** (`pi` respawn) |
| **`Asleep-Clean`** | >= 15 min idle, **0** user bg PIDs (~95% of sleeps) | **0** | Detached | `work.img` only (**0 MB** ckpt) | — | **~0.4–0.8 s** (`mount` + `runsc run`) |
| **`Asleep-Checkpointed`** | >= 15 min idle, **>0** user bg PIDs (~5% of sleeps) | **0** | Detached | `work.img` + `ckpt/` | — | **< 2.0 s** (`mount` + `runsc restore` + reconnect) |
| **`Cold`** | >= 3 d asleep (`ckpt/`) or >= 14 d asleep (`work.img`) | **0** | Detached | Reclaimed when free < 150–250 GB | `ckpt.tar` / `work.img.zst` | **< 2.0 s perceived** with `focused_chat` prefetch (10–16 s unprefetched) |

### 3.1 Four-Condition Idle Definition (Warm -> Asleep)

A box transitions from `Warm` to `Asleep` after **15 continuous minutes** where all four conditions hold:

1. **Agent idle:** `GET /box/state` reports `turn.running == false` and `sends_waiting == 0` across every workspace.
2. **Jobs idle:** Zero running or capacity-queued background subagent jobs on the box.
3. **Routine horizon clear:** The user has no scheduled routine with `next_run_at <= now + 20m` (checkpointing and restoring inside a 20-minute window costs more I/O than staying warm).
4. **Relay obligation clear:** No pending wake job or unacknowledged client send in the relay's view.

**Open approval cards do not block sleep.** Approval cards are persisted in SQLite without wall-clock or monotonic expiry timers; an unanswered card holds zero CPU/RAM while asleep and wakes the box normally when the user approves or rejects it.

### 3.2 Zero-Byte Clean Stop vs. Stateful Checkpoint

Before stopping a box, the supervisor calls `POST /box/prepare-sleep` on `arbos-agentd`:
- `agentd` purges ephemeral caches (`/tmp`, `~/.cache/pip`, `~/.cache/uv`, `~/.npm`, Playwright cache), deletes `/work/.trash` entries older than 7 days, runs `PRAGMA wal_checkpoint(TRUNCATE)` (and `VACUUM` when free pages exceed 25%), and measures the `{ files_bytes, scratch_bytes, trash_bytes }` storage breakdown.
- `agentd` inspects every workspace cgroup's `cgroup.procs` for processes outside `agentd` itself.
- **If `active_user_pids == 0` (`Asleep-Clean`):** `agentd` flushes and exits cleanly; the supervisor unmounts `/work` and detaches `/dev/loopN`. Zero checkpoint bytes are written to NVMe or R2. Waking mounts `work.img` and executes `runsc run` on the pre-unpacked rootfs bundle (`agentd` is a static Rust binary and boots in hundreds of milliseconds).
- **If `active_user_pids > 0` (`Asleep-Checkpointed`):** The supervisor runs `runsc checkpoint --image-path=...`, then unmounts `/work` and detaches `/dev/loopN` so user background processes (dev servers, REPLs, `tmux` sessions) resume intact on wake. Above the measured warm-restore memory watermark, slow restores are logged as explicit SLA misses.

### 3.3 Elastic Sparse `work.img` (8 -> 32 -> 128 GiB)

To eliminate the 1.1 TB empty-filesystem metadata trap while giving every user a 128 GiB workspace ceiling:
- **Creation:** New `work.img` files are created sparse at **8 GiB** with `mkfs.ext4 -L arbos-work -m 0 -E lazy_itable_init=1,lazy_journal_init=1` (~25 MB initial disk allocation -> **~25 GB across 1,000 new users**).
- **Online expansion:** When allocated blocks on a mounted `work.img` cross **75%** of its current apparent size, the supervisor runs `truncate -s <next_tier> work.img` followed by online `resize2fs /dev/loopN` (`8 GiB -> 32 GiB -> 128 GiB`), completing live in milliseconds without interrupting running turns.
- **Cold archival:** Checkpoints older than 3 days (or earlier when host free disk drops below **150 GB**) upload to R2 (`ckpt.tar`) and delete locally. Untouched `work.img` images asleep for >= 14 days compress with `zstd --sparse` to R2 when host free disk drops below **250 GB**; the relay serves `GET /machine` from the cached storage breakdown without waking or fetching the cold box. Below **80 GB** free disk, new box creation halts and alerts.

---

## 4. Architecture & Subsystem Specifications

### 4.1 `arbos-boxd` — Self-Managed OCI Supervisor & Multi-Node Seam

New crate: `crates/arbos-boxd`.

Instead of shelling out to `docker` and privileged helper containers from inside `arbos-relay`, sandbox execution moves behind `arbos-boxd` — a small root daemon on each compute host speaking authenticated HTTP/mTLS to `arbos-relay`.

- **Why `arbos-boxd` instead of in-process `fleet.rs` shell-outs:**
  1. Keeps `arbos-relay` unprivileged (removes `SupplementaryGroups=docker` and helper containers).
  2. Owns `/var/lib/arbos-boxd/images/<version>/rootfs` (unpacked once per `arbos-agent` release tag), `/var/lib/arbos-boxd/boxes/<user_id>/{work.img, mnt/, config.json, ckpt/}`, loop devices, and `/var/run/arbos-runsc`.
  3. **Horizontal scale without a rewrite:** Node 0 runs on loopback (`127.0.0.1`) on the primary host; adding bare-metal worker nodes (`node-1..N`) for users 700 -> 10,000+ uses the exact same `arbos-boxd` RPC interface via `boxes.node_id`.

**`arbos-boxd` internal HTTP surface:**

| Endpoint | Responsibility |
| --- | --- |
| `POST /v1/boxes/{user_id}/ensure` | Create sparse 8 GiB `work.img` if absent; write OCI `config.json` (8 vCPU, 16 GiB cap, systrap, bind mounts for `/work`, `/home/daytona`, `/etc`, `resolv.conf`) |
| `POST /v1/boxes/{user_id}/wake` | Fetch `work.img.zst` / `ckpt.tar` from R2 if cold (joining any in-flight prefetch), attach loop + mount `/work`, run `runsc restore` + `SIGUSR1` (if `ckpt/` exists) or `runsc run` |
| `POST /v1/boxes/{user_id}/prefetch` | Background R2 -> local NVMe staging triggered on `focused_chat` or routine lookahead (idempotent) |
| `POST /v1/boxes/{user_id}/sleep` | Call `agentd` `POST /box/prepare-sleep`, execute Clean Stop or `runsc checkpoint`, unmount `/work`, detach `/dev/loopN`, return storage split |
| `POST /v1/boxes/{user_id}/grow` | Online `truncate` + `resize2fs` to next tier (`32 GiB`, `128 GiB`) |
| `POST /v1/boxes/{user_id}/archive` | Stream `ckpt/` or `work.img.zst` to R2 and unlink local copy |
| `GET /v1/host/status` | Report `mem_available_gb`, `swap_used_gb`, `load5`, `disk_free_gb`, `resident_boxes`, `present_boxes`, and per-box `stat` block usage |

### 4.2 Relay Admission, Placement & Predictive Wake (`crates/arbos-relay`)

1. **Split `resident` vs. `present` admission (`crates/arbos-relay/src/fleet.rs`):**
   - **Single source of truth:** Lifecycle state (`warm`, `waking`, `asleep_clean`, `asleep_ckpt`, `cold`, `error`) lives in `boxes.lifecycle_state` in SQLite, updated on every supervisor transition and rendered directly by `GET /machine`.
   - `resident` counts only `warm` + `waking` boxes on a node, gated by `max_resident_boxes` (default **160** on a 503 GB node) and `MemAvailable >= 64 GB`.
   - `present` counts boxes with local NVMe files, gated only by `disk_free_gb >= 80 GB`.
   - **New user sign-up gate:** Checks node `disk_free_gb >= 80 GB` (not total historical box rows). If `resident == max_resident_boxes` when a box wakes, the relay immediately sleeps the least-recently-active idle `Warm` box rather than waitlisting.
2. **Predictive wake triggers (`crates/arbos-relay/src/rooms.rs`, `routines.rs`):**
   - **`focused_chat` on `PRESENCE`:** When a client focuses a chat belonging to a `Cold` box, fire `POST /v1/boxes/{user_id}/prefetch` immediately; if `Asleep-Clean`, optionally pre-wake so `agentd` is already connected before the user hits Send.
   - **Typing in agent chats:** Extend the existing `PRESENCE` typing signal (currently sent only in people chats) to project/agent chats on macOS and iOS to wake `Asleep` boxes ~1–3 s before message submission.
   - **Routine scheduler lookahead:** Every 15 s tick queries `routines.next_run_at`: start R2 prefetch at **`T - 5 min`** for `Cold` boxes and wake at **`T - 60 s`** for `Asleep` boxes.

### 4.3 Positive Restore Reconnect & Room Bounds (`crates/arbos-agentd` & `crates/arbos-relay`)

1. **Positive reconnect trigger on `runsc restore`:**
   Client TCP/WebSocket connections inside a restored gVisor sentry are dead on the wire even though file descriptors appear open to `agentd`. Immediately after `runsc restore`, `arbos-boxd` signals `agentd` (`SIGUSR1` / local control endpoint), causing `agentd` to drop all relay HTTP/WebSocket pools and dial `/box/hello` + active rooms immediately without restarting the process.
2. **Per-room socket tracking & removing `keep_open`:**
   - **Step 1 (Relay):** Change `room.has_box(&owner)` (`crates/arbos-relay/src/rooms.rs`, `state.rs`) to track presence per **`(owner, room_name)`**. Whenever a row lands in a room with no socket for that specific `(owner, room_name)`, `after_person_row` queues `chat_open` / `people_chat_open` regardless of sibling workspace sockets.
   - **Step 2 (`agentd`, deployed after Step 1):** Remove the `keep_open` exemption on `PROJECT_DOC_CHAT_ID` and `people/*` in `crates/arbos-agentd/src/sync.rs`, allowing all idle rooms to close after `ROOM_IDLE_SECS` (15 min), and enforce a **box-wide `MAX_BOX_OPEN_ROOMS = 128` ceiling** across all workspaces (below the ~205-room 2-core saturation threshold).

### 4.4 Relay SQLite Concurrency for 1,000+ Users (~2,500 Client Sockets)

1,000 active accounts correspond to ~1,500–2,500 concurrent backgrounded Mac + iPhone WebSockets:
- Separate a read-only WAL connection pool (`synchronous = NORMAL`, `busy_timeout = 5000`, `mmap_size = 1 GiB`) from the single writer handle in `crates/arbos-relay/src/db.rs` so history/admin reads never stall room `PUSH` acknowledgements.
- Coalesce `PRESENCE` `last_seen_at` updates in memory and flush in a single batched transaction every 60–300 s rather than issuing per-frame SQLite writes.

---

## 5. Phased Implementation & Verification Plan

### Phase 0: Immediate Headroom, Room Bounds & SQLite Read/Write Split
- **Changes:**
  - Enable `arbos-agent` image tag pruning (`IMAGE_TAG_PRUNE=1`, `IMAGE_TAGS_KEEP=4`, keeping running snapshots exempt).
  - Ship per-`(owner, room)` box-socket tracking in `crates/arbos-relay/src/rooms.rs`, then remove `keep_open` and add `MAX_BOX_OPEN_ROOMS = 128` in `crates/arbos-agentd/src/sync.rs`.
  - Split relay SQLite reader pool from writer and batch `last_seen_at` writes (`crates/arbos-relay/src/db.rs`).
- **Verification Gate:** Multi-workspace box with >150 rooms drops to <= 1 open room after 15 min idle; messages to closed `people/*` and `project-doc` rooms wake and deliver with zero dropped rows.

### Phase 1: `agentd` Sleep Hooks & `arbos-boxd` Supervisor
- **Changes:**
  - Remove wall-clock and monotonic approval-card expiry timers in `crates/arbos-agentd/src/approval.rs`.
  - Add `POST /box/prepare-sleep`, `/work/.trash` 7-day retention, and `SIGUSR1` positive relay reconnect in `crates/arbos-agentd`.
  - Implement `crates/arbos-boxd` (OCI rootfs unpacker, 8 -> 32 -> 128 GiB elastic `work.img` loop manager, `Asleep-Clean` `runsc run`, and `Asleep-Checkpointed` `runsc checkpoint`/`restore`).
- **Verification Gate:**
  - Fresh 8 GiB `work.img` consumes `< 35 MB` real disk; online `grow` to 32 GiB and 128 GiB succeeds under active write load.
  - `Asleep-Clean` -> `Warm` healthcheck completes in `< 800 ms` with `0 MB` checkpoint written.
  - `Asleep-Checkpointed` -> `Warm` healthcheck completes in `< 2.0 s` with background child PID intact and relay WebSocket reconnected.

### Phase 2: Fleet Cutover, Admission Split & 15-Minute Sleep (`80 -> 500+` Users)
- **Changes:**
  - Integrate `Provider::Boxd` and `boxes.node_id` in `crates/arbos-relay/src/boxes.rs` and `fleet.rs`.
  - Split `fleet::capacity` into `resident` (`max_resident_boxes = 160`) vs. `present` (disk-gated); replace `spawn_always_on` on `boxd` boxes with the 15-minute 4-condition sleep supervisor + 20m/60s routine lookahead.
  - Migrate remaining external sandboxes and convert existing local Docker `runsc` boxes in-place during sustained-idle windows (`GET /box/state` idle for 30 s; `arbos-boxd` mounts each box's existing `work.img` directly with zero data copy).
- **Verification Gate:** Steady-state idle boxes drop to `0 MB` RAM and `0` loop mounts after 15 min; `GET /me` keeps `signup: "open"` past 80 provisioned accounts.

### Phase 3: R2 Cold Tier, Prefetch & Client Machine Card (`500 -> 1,000` Users)
- **Changes:**
  - Add R2 cold-tier archival and restore in `arbos-boxd` (reusing the relay's SigV4 S3 signing pattern in `crates/arbos-relay/src/sites.rs`) with automatic early eviction below 150 GB free NVMe.
  - Wire `focused_chat` R2 prefetch and 5-minute cold routine lookahead in `crates/arbos-relay`.
  - Ship spaced macOS (`crates/arbos-mac`) and iOS (`apps/ios/Arbos`) updates: Machine card `awake / asleep / cold` + `Your files / Scratch / Trash` breakdown (served from cached metadata when cold) and typing presence in agent chats.
- **Verification Gate:** Opening a chat on a `Cold` box prefetches from R2 and completes wake with `< 2.0 s` delay after message send; viewing Settings -> Machine on a `Cold` box causes zero wakes.

### Phase 4: Multi-Node Compute Pool & 1,000-User Load Gate (`1,000 -> 3,000+` Users)
- **Changes:**
  - Attach a second bare-metal NVMe worker node running `arbos-boxd` (`node-1`) over WireGuard/mTLS and enable free-headroom node placement in `arbos-relay`.
- **Verification Gate:** Synthetic 1,000-box load test (850 `Asleep`/`Cold`, 150 concurrently `Warm`, 30 wakes/min): `waitlist == 0`, p95 `Asleep-Clean` wake `< 1.0 s`, p95 `Asleep-Checkpointed` wake `< 2.0 s`, relay SQLite write lock wait p99 `< 10 ms`.

---

## 6. Target Infrastructure Unit Economics at 1,000 Users

Excluding metered LLM inference:

| Component | Monthly Cost (Est.) | Role |
| --- | --- | --- |
| Primary Bare-Metal Host (`node-0`) | ~$250 – $350 / mo | `arbos-relay`, Synapse/Postgres, LiveKit, warm box shard 0 |
| Secondary Bare-Metal NVMe Worker (`node-1`) | ~$180 / mo | Dedicated `arbos-boxd` shard 1 (doubles RAM & NVMe, isolates relay) |
| Cloudflare R2 Cold Storage (~1 TB compressed cold state) | ~$15 / mo | `$0.015/GB-mo`, `$0` egress on prefetch/wake |
| External Per-Hour Sandbox Providers | **$0 / mo** | Fully retired |
| **Total Infrastructure (1,000 Users)** | **~$450 – $550 / mo** | **~$0.50 / user / month** |
