# Launch day on one host: boxes

How many machines the arbos.life host holds, the settings that make it so, what to clean before a wave of sign-ups, what to watch, and what to do when it fills. Written 2026-11-18 for the host as it was that day; the method outlasts the numbers. The machinery is in `docs/self-host.md` §5 and `crates/arbos-relay/src/fleet.rs`.

Steps marked **[production]** change the host. Everything else only reads.

## 1. What a new user gets, and what limits it

With `ARBOS_DEFAULT_PROVIDER=gvisor` a new user's machine is a `runsc` container made by the relay (`fleet::create_box`): a sparse ext4 image for `/work`, mounted through a loop device, and one container from the current `arbos-agent` image. Always-on keeps every machine running whether its user is there or not, so each sign-up is a container that stays.

A sign-up is refused a machine, and put on the waitlist, when either of these holds (`fleet::capacity`):

| Gate | Value | Where it is set |
|---|---|---|
| machines, running or stopped by their user, ≥ the cap | cap: `PUT /admin/fleet {"max_boxes": N}` (kept in the database, wins), else `ARBOS_HOST_MAX_BOXES`, else 80 | runtime, no restart |
| free disk on the relay data volume < the floor | `ARBOS_HOST_MIN_FREE_DISK_GB`, default 40 | env, relay restart |

Memory is not part of the gate: a host short of memory still accepts sign-ups and relies on eviction (§6). The cap is what keeps it from getting there.

The waitlist is first come, first served, and slow: once a minute, when there is room, the one person at its head is taken off it, told their computer is ready, and woken. Sixty people in line take an hour. Keep the cap above demand so the line stays empty; the Mac app in particular leaves a new user who is waitlisted on its first-run screen. A machine its user stopped keeps its slot, so Start is never refused.

## 2. The capacity number

The host: 128 vCPU, 503 GB memory, 1.3 TB disk. On 2026-11-18, with 72 user machines and 53 other containers running: 456 GB memory available, 2.5 cores in use, 342 GB disk free. Measured on those 72 machines over 24 hours: 83 MiB of memory on average (770 MiB at most), 4 % of a core on average. A machine's `/work` image is 128 GiB on paper and 67 MiB on disk when made; the 103 images on the host hold 660 MB each on average (median 202 MiB, largest 5.6 GB).

Each resource gives a ceiling; the cap is the lowest, with a margin.

| Resource | Budget | Planned per machine | Ceiling |
|---|---|---|---|
| Disk | 342 GB free − 40 GB floor − 20 GB for new image tags = 282 GB | 1 GB (one and a half times today's average) | 280 more |
| CPU | 128 vCPU − 16 for the relay, Postgres, Synapse and LiveKit − 6 = 106 | 0.45 of a core on average, ten times today's | 235 more |
| Memory | 456 GB available − 64 GB eviction floor = 392 GB | 1 GB on average, twelve times today's | 390 more |

**Cap: 300 machines**, 228 more than the 72 of that day. Disk and CPU run out first; memory is not the limit until machines average well over 1.5 GB each. The planned figures are guesses about people who have not arrived: on launch day read the real averages in `GET /admin/fleet` → `samples.last_24h` and move the cap with one call.

A machine's own limits (`ARBOS_BOX_CPUS=8`, `ARBOS_BOX_MEM_GB=16`) are what one user may use at once, not what the host has set aside: 300 machines × 16 GB is ten times the host, and fourteen machines pinning 8 cores each fill every core. That is deliberate. It is safe for memory because of the eviction loop (§6) and the kernel order in §3; for CPU, Docker's shares keep machines fair to each other but do not keep a full host fast, so watch load.

Not limits on this host: the machines' bridge is a /16, loop devices are made on demand, and connection tracking is at 1 % of its table.

## 3. Settings

1. **[production]** Raise the cap. Takes effect at once, restarts nothing, touches no running machine:

   ```bash
   curl -sS -X PUT -H "Authorization: Bearer $ARBOS_RELAY_ADMIN_TOKEN" -H 'Content-Type: application/json' -d '{"max_boxes":300}' http://127.0.0.1:7700/admin/fleet
   ```

   The answer is the new capacity; `max_boxes` must be 300 and `has_room` `true`. The value is kept in the relay's database and wins over `ARBOS_HOST_MAX_BOXES`, so editing the env file alone changes nothing once it has been set this way (on this host it has: 80 until 2026-11-18, 150 since). Raising the cap does not empty a waitlist at once: people in line are let through one a minute. Do not raise it while the spend kill switch is on (§7): each person let through then is told their computer is ready, is refused the wake, and is left with a machine in `error` until they press Start.

2. **[production]** In `/etc/arbos/relay.env` (read at the next relay restart; a restart does not stop machines, they reconnect within seconds):

   ```
   ARBOS_HOST_MAX_BOXES=300
   ARBOS_HOST_EVICT_MIN_AVAILABLE_GB=64
   ARBOS_HOST_MIN_FREE_DISK_GB=40
   ```

   The first only matters if the database value is ever cleared; it is there so the two do not disagree. The other two are the defaults, written down so the host's numbers are in one place: the first is the available memory under which idle machines are stopped, the second the free disk under which no machine is made. The deploy script keeps all three when it rewrites the file. Machines made from now on can be given a smaller ceiling than today's 16 GB / 8 CPU with `ARBOS_BOX_MEM_GB` and `ARBOS_BOX_CPUS`; existing machines keep theirs until they are re-created by an image roll.

3. **[production]** Shrink what one newcomer can write. `ARBOS_BOX_WORK_GB` is the size of a new machine's `/work` image; at 128, three people who fill theirs take all the free disk, and a full disk stops every machine (2026-10-25). The image is sized once, when the machine is made, so a smaller number applies to machines made after the restart and existing ones keep theirs:

   ```
   ARBOS_BOX_WORK_GB=32
   ```

   A user who needs more is given it by hand; nothing grows an image by itself yet.

4. **[production]** Keep the kernel away from the databases. Once the relay built from a commit after 2026-11-18 is deployed (#3090), the relay unit carries `OOMScoreAdjust=-900` and new machines start at `+500`, so a host that runs out of memory loses a sandbox (whose files are on its `/work` image) first. Postgres, Synapse and LiveKit run as plain containers at score 0; until they are re-created with `--oom-score-adj=-900`, set it on the running processes (lost when the container restarts):

   ```bash
   for c in arbos-matrix-postgres arbos-matrix-synapse livekit; do for p in $(docker top "$c" -o pid | tail -n +2); do echo -900 | sudo tee /proc/$p/oom_score_adj >/dev/null; done; done
   ```

   After that deploy, make one canary machine (a test user, `POST /admin/users/test`, then its first sign-in or `POST /machine/start`) before the wave and read its score: `cat /proc/$(docker inspect --format '{{.State.Pid}}' arbos-box-<user>)/oom_score_adj` must print `500`. If the machine does not start, `journalctl -u arbos-relay | grep 'wake failed'` has Docker's reason, and the flag is the first suspect.

## 4. Before the wave

1. Read what is reclaimable: `docker system df` (56 GB of images on 2026-11-18: forty agent image tags back to 0.1.166, of which machines run four).
2. **[production]** Remove agent image tags no machine runs. List them first; a tag in use is refused by Docker, so this cannot break a running machine:

   ```bash
   docker ps -a --format '{{.Image}}' | sort | uniq -c
   docker images ghcr.io/unarbos/arbos/arbos-agent --format '{{.Tag}}\t{{.Size}}\t{{.CreatedSince}}'
   docker rmi ghcr.io/unarbos/arbos/arbos-agent:<tag no container uses>
   ```

   To keep it that way, `IMAGE_TAG_PRUNE=1` in `/etc/arbos/deploy.env` lets the deploy worker keep the deployed tag, the default and the newest four.
3. **[production]** Move the stragglers still on old agent images to the current one (the deploy worker's roll does this); their tags then become removable. A roll restarts that one machine on its kept `/work` image.
4. Read what else is on the host: `docker ps -a --filter status=created --filter status=exited`. A user machine is named `arbos-box-…` and carries the label `arbos.fleet=gvisor`; anything else there was not made by the relay (on 2026-11-18: thirteen leftovers of benchmark runs from September, holding no memory). **[production]** `docker rm <name>` removes one once its owner agrees.
5. Make sure the current agent image is already on the host (`docker images | grep <current tag>`), so the first sign-up after a release does not wait for a 3.8 GB pull.

## 5. What to watch

`GET /admin/fleet` is the one page: `capacity` (cap, resident, waitlist, free disk, `has_room`), `pressure` (available memory, swap, load, free disk), every machine with its live CPU and memory, and the last 50 fleet events.

| Sign | Where | Meaning |
|---|---|---|
| `capacity.waitlist` > 0 | `/admin/fleet` | a gate in §1 is shut; `capacity` says which |
| event `waitlisted` | `/admin/fleet` → `events`, journal "host full; user waitlisted" | same, with the numbers at that moment |
| event `memory_low` | events | available memory within 32 GB of the eviction floor |
| event `disk_low` | events | free disk within 20 GB of the floor; sign-ups stop at the floor |
| event `evicted` | events | the host stopped an idle machine under pressure; its user's next message starts it |
| event `died` | events | a machine's container went away (out of memory at its own cap, or killed); the relay restarted it |
| machine stuck `waking`, or `error` | `/admin/fleet` → `sandboxes[].state` | a create or start that did not finish, after three tries (two on a relay older than #3117); `journalctl -u arbos-relay | grep 'wake failed'` has Docker's reason. Its user's Start tries again |
| `wake failed` naming `losetup`, `flock` or "gave up after 120 s" | journal | machines are made eight at a time and each claims a loop device under a shared lock (#3110), waiting 20 s for it at most; a helper that hangs is given up on after 120 s. One line is noise, since the wake is retried; a run of them means a helper container is stuck: `docker ps --filter ancestor=ubuntu:24.04` shows it, and removing it frees the lock |
| load5 > 100 | `pressure` | eviction by load starts, one machine every 30 s |

## 6. When the host is 80 % full

Eighty per cent is 240 machines, or free disk under 100 GB, or available memory under 130 GB, whichever comes first.

1. Find what is short in `/admin/fleet`: `capacity` and `pressure`.
2. Disk: repeat §4. Then look for the largest `/work` images (`sandboxes[].work_img_used_mb`).
3. Memory or CPU: the host already sheds load by itself. Under the memory floor or over 20 GB of swap it stops idle machines, least recently used first, never one in the middle of a turn, five at most every 30 s; over load 100, one every 30 s. Nothing is deleted; a stopped machine starts again on its user's next message.
4. **[production]** To shed load before that point, stop the machines nobody is using (a relay built after 2026-11-18, #3092; an older one has no such call):

   ```bash
   curl -sS -X POST -H "Authorization: Bearer $ARBOS_RELAY_ADMIN_TOKEN" -H 'Content-Type: application/json' -d '{"count":20}' http://127.0.0.1:7700/admin/fleet/evict
   ```

   Up to 20 per call, the same choice and the same effect as step 3. Three cautions. Not within two minutes of a relay restart: the relay learns which machines are mid-turn from their keepalives, and for the first minute it knows of none. Let the call finish: each stop waits up to 30 s for the machine to shut down, and an interrupted call leaves the rest running. And a relay restart forgets who was evicted: always-on starts every one of them again within a minute, so evict after a deploy, not before.

   Do not use `docker stop` by hand: the relay sees a dead container and starts it again within ten seconds.
5. **[production]** To stop taking sign-ups while you work, lower the cap to the current resident count with the call in §3; newcomers are waitlisted in order and let through one a minute once it is raised.
6. A second host. The gVisor fleet in this document is one host only. More hosts are supported only through the `boxd` provider (`ARBOS_BOXD_URL`, `ARBOS_BOXD_NODES_FILE`; `crates/arbos-boxd/README.md`, "More than one host"), which is not deployed on this host and is not a launch-day switch: it needs the daemon installed on each host, mutual TLS between them, and the relay moved onto the daemon's bridge. Until then the levers are the ones above and a bigger disk.

## 7. The spend cap that stops everyone

Machines are one limit; money is the other, and it has a switch that acts on every user at once.

- A new account starts with `ARBOS_DEFAULT_GRANT_USD` (default 100) and a daily cap of `ARBOS_DEFAULT_DAILY_CAP_USD` (default none). At zero balance or at its cap, that account's model calls are refused (402) and its machine keeps running.
- `ARBOS_GLOBAL_DAILY_CAP_USD` (default 500) is the whole relay's model spend since 00:00 UTC, every account and test user included. When it is reached the kill switch turns itself on: every model call answers 503 "Arbos is in read-only mode", no machine is woken or created, and new sign-ups get no machine. Running machines are not stopped. It clears itself at 00:00 UTC.
- Turning the switch off by hand on the same day does not hold: the next request sees spend still over the cap and turns it on again. The cap has to be raised.

Read where it stands (cap, spend today, whether the switch is on):

```bash
curl -sS -H "Authorization: Bearer $ARBOS_RELAY_ADMIN_TOKEN" http://127.0.0.1:7700/admin/killswitch
```

**[production]** To change the cap, set `ARBOS_GLOBAL_DAILY_CAP_USD=<number>` in `/etc/arbos/relay.env` and restart the relay (`systemctl restart arbos-relay`, or a deploy). The restart closes every app's connection for a few seconds, the apps reconnect by themselves, and machines keep running with their turns. If the switch had already tripped that day, the first request after the restart clears nothing by itself: turn it off once with `PUT /admin/killswitch {"on":false}`; with spend now under the new cap it stays off.

One account, at once and without a restart: `PUT /admin/users/{id}/credits` with `{"grant_usd": n}`, `{"daily_cap_usd": n}` (`0` freezes the account, `null` removes the cap).

## 8. What a deploy does to running machines

A relay deploy restarts the relay, not the machines: containers keep running, their agents reconnect within seconds, turns in flight continue. Changes to how a machine is started (its limits, flags, image) reach a machine only when it is next created or re-created by an image roll.
