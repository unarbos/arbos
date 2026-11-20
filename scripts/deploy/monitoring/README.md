# Monitoring for the relay host

Prometheus, Grafana and the exporters that answer two questions about the host that runs the relay
and every box: *are we OK right now*, and *what is degrading*. Everything is a file in this folder;
nothing is configured by clicking.

The stack is not deployed by merging this. [Production runbook](#production-runbook) is the ordered
list of steps, each marked when it changes production. Steps 1–7 interrupt nobody and give a
working Grafana over an SSH tunnel; step 9 is the one user-interrupting window.

## What runs

One `docker compose` project (`arbos-mon`), host networking, every listener on `127.0.0.1`.

| service | image | listens on | memory cap | what it is for |
|---|---|---|---|---|
| `prometheus` | `prom/prometheus:v3.13.5` | 9090 | 2 GB | scrapes everything below, evaluates the alert rules; 15 days or 15 GB, whichever comes first |
| `prometheus-subnet` | same | 9091 | 256 MB | holds only the label-free `subnet:*` aggregates, for the Subnet view; 15 days or 1 GB |
| `alertmanager` | `prom/alertmanager:v0.34.2` | 9093 | 128 MB | sends alerts to one webhook (Discord) |
| `grafana` | `grafana/grafana:13.2.6` | 3300 | 1 GB | the dashboards; the only piece reachable from outside, through Caddy |
| `cadvisor` | `ghcr.io/google/cadvisor:v0.60.6` | 9180 | 1 GB, 2 CPUs | CPU, memory, pids, IO and network per container, and for the relay's systemd service. Unprivileged; see [what it is given](#what-cadvisor-is-given) |
| `node-exporter` | `prom/node-exporter:v1.12.1` | 9100 | 128 MB | host CPU, memory, disk, load, network, OOM kills |
| `blackbox` | `prom/blackbox-exporter:v0.29.1` | 9115 | 64 MB | probes the relay's `/healthz` on the host (the page), and `relay.arbos.life` / `arbos.life` over HTTPS (a warning; certificate expiry included) |

Files:

| path | |
|---|---|
| `docker-compose.yml` | the services above |
| `prometheus/prometheus.yml` | scrape jobs |
| `prometheus/rules.yml` | alerts |
| `prometheus/subnet-rules.yml`, `prometheus/subnet.yml` | the aggregates behind the Subnet view, and the Prometheus that holds only them |
| `prometheus/rules_test.yml` | unit tests for both rule files |
| `check-rules.py` | reads `subnet-rules.yml` and fails if a rule could let a counter or a label leave, or passes a count of machines or an amount of tokens or calls on uncoarsened; the rounding numbers are named there |
| `alertmanager/alertmanager.yml`, `blackbox/blackbox.yml` | their configs |
| `grafana/provisioning/` | datasource and dashboard loading for the main organisation |
| `grafana/dashboards/*.json`, `grafana/dashboards-subnet/subnet.json` | the dashboards, written by `dashboards.py` |
| `grafana-access.sh` | `--check` validates the env file (weak or malformed values are refused); without it, creates the Subnet organisation, its datasource, dashboard and the one viewer account |
| `Caddyfile.grafana` | the vhost for Grafana |
| `.env.example` | the secrets the stack needs; the filled-in copy is never committed. Compose refuses to start without them |
| `check.sh` | static checks (CI runs it) |
| `docker-compose.local.yml`, `local/` | running it on a laptop against fake targets |

## Dashboards

- **Arbos** (home, in both organisations): the front page, one screen. Six numbers (service,
  machines working, tokens per minute and in 24 hours, model calls succeeding, spend of the daily
  cap), the fleet in the centre with machines by state and host use beside it, then tokens,
  model calls and machine starts over time; below the fold spend, host and machines over time.
  It is generated once and written twice, `grafana/dashboards/arbos.json` and
  `grafana/dashboards-subnet/subnet.json`. The team's copy reads exact figures and the partner's
  the same figures rounded. They differ in the centre: the team has the swarm, one dot per
  box; the partner has machines by tokens per minute, as counts. See
  [What the partner sees of the machines](#what-the-partner-sees-of-the-machines). The team's copy also splits tokens by
  kind and links to the dashboards below.
- **Arbos fleet**, three rows of single stats then time series. Service: relay and public
  probes, kill switch, today's spend against the daily cap, model errors and response time, relay
  restarts, targets down. Boxes: running, slots used against `max_boxes`, asleep, waitlisted, start
  failures, wake queue. Host: CPU, memory, disk free and days until full, load, relay memory,
  sockets, database writer wait.
- **Boxes**: top-N boxes by CPU, memory, pids, disk IO and network; every box against its memory
  and pids limit; OOM kills and restarts; a drill-down by the `Box` variable.
- **Swarm**: every box at once, refreshed every 5 s. One bubble per box: up is tokens per minute
  its agents are using, right is CPU, area is memory, colour is state (working a turn, awake and
  idle, waking, asleep, error). A busy fleet rises off the floor and an idle one sits along it.
  Around it: tokens per second, per minute and per 24 hours, model calls, spend against the cap,
  boxes by state, token rate by kind, the busiest boxes, a table of every box with tokens, calls
  and spend beside CPU, memory, pids and network, and a drill-down by the `Box` variable. See
  [Swarm and the relay](#swarm-and-the-relay) for what it shows on which relay.
- **Agents**: turns started / finished / failed, turn duration percentiles, messages waiting,
  crashes, model calls, errors and response time, and how many awake boxes are reporting.
- **Subnet view** is what this page calls the partner's copy of Arbos: the one dashboard of the
  Subnet organisation and its home. A percentage with nothing to divide by reads 100 (nothing
  failed); a figure whose source is not reporting says so in grey, never a blank or a NaN. Turn
  counts and turn duration can only come from the machines themselves; the rules for them exist
  and are tested, and the panels are added when boxes run an image that reports them (#3096).

To change a panel: edit the query in `dashboards.py`, run `python3 dashboards.py`, commit both.
Grafana reloads the files within a minute; the Subnet view is reloaded by `grafana-access.sh`.

## Alerts

`prometheus/rules.yml`:

| alert | fires when | severity |
|---|---|---|
| `RelayDown` | the relay's `/healthz` on `127.0.0.1:7700` fails for 1 min | critical |
| `PublicProbeFailing` | `relay.arbos.life/healthz` or `arbos.life` fails from the host for 5 min (through DNS and, for the site, Cloudflare: it can fail while the relay is fine) | warning |
| `TLSExpirySoon` | a probed certificate has under 14 days left | warning |
| `ScrapeTargetDown` | a target that used to answer does not (the relay and box targets have their own alerts) | warning |
| `RelayMetricsOff` | the relay is healthy but its metrics endpoint does not answer for 10 min (the relay starts without it when the variable is unset or wrong, or the port is taken). Silenced in step 4 until the relay deploy of step 9 | warning |
| `BoxesUnreachable` | awake boxes do not answer the relay's metrics scrape for 5 min | warning |
| `BoxMetricsRejected` | a box's metrics answer was rejected by the relay or had lines dropped | warning |
| `DiskFreeLow` | `/` or `/var/lib/docker` under 15 % free | warning |
| `DiskFullWithin24h` | the same mounts on course to fill within a day, judged only once there is an hour of history | critical |
| `HostMemoryLow` | under 10 % memory available | critical |
| `BoxStartFailures` | any failed wake job in 10 min | warning |
| `Waitlist` / `WaitlistGrowing` | anyone waiting for 15 min / three more in 30 min | warning / critical |
| `BoxCapacityHigh` | 90 % of `max_boxes` admitted | warning |
| `SpendAt60Percent` / `SpendAt85Percent` | today's spend against the global daily cap, which trips the kill switch and stops every agent | warning / critical |
| `KillSwitchOn` | the kill switch is on | critical |
| `ModelErrorsHigh` | over 10 % of model calls not answering 2xx (4xx included: there is no baseline for those yet) | warning |
| `ModelProviderFailing` | over 10 % of model calls failing at the provider: 5xx or no answer | critical |
| `ModelSlow` | p95 over 30 s to first byte | warning |
| `BoxOOMKilled` | the kernel OOM-killed anything on the host | critical |
| `RelayRestarted` | the relay restarted in the last 15 min | warning |
| `RelayWalLarge` | the relay database's write-ahead log over 128 MB for 10 min (quiet until the relay exports `arbos_relay_db_bytes`) | warning |
| `RelayMemoryHigh` | relay memory over 40 GiB (it sat at ~22 GB when this was written; tune the number in the rule) | warning |

Disk alerts and the disk figures on the dashboards watch `/` and `/var/lib/docker` only, so `/boot`
or an EFI partition can neither alert nor become the partner's disk number. If the data lives on
another mount, change `mountpoint=~"/|/var/lib/docker"` in `prometheus/rules.yml`,
`prometheus/subnet-rules.yml` and `FS` in `dashboards.py`.

All go to one receiver. The severity is in the message title: a `critical` repeats every 4 hours
while it lasts, a `warning` every 12, so a standing condition such as a short waitlist does not
fill the channel.
Summaries never name a box or a user: they say what is wrong, and the admin dashboards say where.

**Pointing it at Discord.** In the channel that should get alerts: Server Settings → Integrations →
Webhooks → New Webhook → Copy Webhook URL. Put it in the env file as `ALERT_WEBHOOK_URL=…` and run
`docker compose up -d alertmanager`. The URL is a credential (anyone holding it can post to the
channel); it lives only in the env file. Send a test:

```bash
docker exec arbos-mon-alertmanager amtool alert add Test severity=warning --annotation='summary="test alert"' --alertmanager.url=http://127.0.0.1:9093
```

It arrives within 30 seconds and resolves itself after five minutes.

## Access model

- **Nothing but Grafana is reachable.** Prometheus, Alertmanager and the exporters have no
  authentication of their own, so they bind loopback and are never proxied.
- **Grafana**: anonymous access off, sign-up off, organisation creation off, public dashboards and
  external snapshots off, Secure + SameSite=strict cookies. The admin password comes from
  `/etc/arbos/monitoring.env` (root, mode 0600). There is no default: compose refuses to start
  without it (Grafana would otherwise fall back to `admin`/`admin`). A short, repetitive or
  dictionary password is refused twice: by `grafana-access.sh --check`, and by the Grafana
  container itself, which exits before starting.
- **Each container gets only its own secret**: Grafana its password and URL, Alertmanager the
  webhook URL. The viewer's password is read only by `grafana-access.sh`.
- **Two organisations.** The main one holds Fleet, Boxes and Agents and the full Prometheus; only
  the admin is in it. The **Subnet** organisation holds one dashboard and one datasource.
- **The viewer account** (`SUBNET_VIEWER_LOGIN`, default `tobias`) is a Viewer in the Subnet
  organisation and a member of nothing else. It has no Explore page, and its home is the Subnet
  view; each organisation's home is set on the organisation, with no instance-wide default that
  could show an admin dashboard's outline to the wrong one. `grafana-access.sh` tests both.
- **Why a second Prometheus.** A Grafana Viewer can send any query to any datasource of their
  organisation, whatever dashboards they are allowed to open; hiding a dashboard is not a
  boundary. So the Subnet organisation's datasource is an instance that only ever receives the
  `subnet:*` recording rules, each of which aggregates the identifying labels away. There is no
  query that returns a user id, a container name, a box id or a hostname, because that data is
  not there, and nothing in it is per machine. One label does leave: `bucket`, on a count of
  machines ([What the partner sees of the machines](#what-the-partner-sees-of-the-machines)). It has no scrape job for the relay at all, so it
  cannot be pointed at `/metrics/boxes` by changing a path; it federates and nothing else.
  `prometheus/rules_test.yml` feeds the rules labelled input and lists their complete output;
  `grafana-access.sh` ends by logging in as the viewer and failing unless the admin dashboards
  and datasource refuse it.
- Container names contain user ids. They appear on the Boxes and Swarm dashboards only: not in
  the Subnet view and not in alert messages. (The `box` label on agent and relay metrics is an
  opaque id; `arbos_relay_box_info` is the one series that ties it to a container, and it exists
  only in the main Prometheus, which scrapes it from a path of its own.)

## What cAdvisor is given

It runs unprivileged: every capability dropped except `SYSLOG`, `no-new-privileges`, a read-only
root filesystem, and four read-only mounts.

| given | for | without it |
|---|---|---|
| `/sys` | the cgroup tree: CPU, memory, pids, IO per container | nothing works |
| `/proc` (as `/rootfs/proc`) | network counters, read from each container's network namespace | no network panels |
| `/run/docker.sock` | container **names** and images | containers are anonymous cgroup ids, and a box cannot be told from any other container |
| `/run/containerd/containerd.sock` | cAdvisor's Docker support will not start without it when Docker uses the containerd image store. Production's storage driver is `overlayfs`, which is that store, so the mount is needed there | as above |
| `/dev/kmsg` + `SYSLOG` | OOM kills, read from the kernel log | the per-box OOM series is empty; the `BoxOOMKilled` alert still works (node_exporter's host counter) |

Not given: `privileged`, the host's `/`, the rest of `/run`, `/var/lib/docker`, `/dev/disk`.
Tested locally in this form (Docker 29, cgroup v2, containerd image store): names, CPU, memory, pids,
limits, network, disk IO, and a real OOM kill counted against the right container.

What remains is the sockets. A read-only mount of `docker.sock` does not make the Docker API
read-only: whoever controls that container controls Docker, and so the host. There is no narrower
way to learn container names; `containerd.sock` is as powerful and cannot be proxied, so a filtering
socket proxy would add a container without removing the exposure. The container listens on loopback
only, takes no input but Prometheus's scrape, and runs a pinned image. To run without any socket,
delete the two socket mounts: every host panel and the relay's own numbers keep working, and the
Boxes dashboard goes empty.

## What cAdvisor sees under gVisor

cAdvisor reads the host's cgroups. A `runsc` box is one sandbox in one cgroup, so cAdvisor measures
the box **from outside**: what it costs the host, not what happens inside it. It cannot see inside a
sandbox, and nothing here pretends to: every Boxes panel is built from cgroup data that exists for
a `runsc` container (CPU, working-set memory, host threads against the pids limit, network, IO,
start time). No panel depends on in-sandbox process counts, file descriptors or per-box filesystem
usage, so none is empty for that reason. How many turns a box is running comes from agentd, not
from cAdvisor.

| metric | under `runsc` |
|---|---|
| CPU (`container_cpu_usage_seconds_total`) | correct for the whole sandbox, the gVisor kernel's own overhead included |
| memory (`container_memory_working_set_bytes`, `…_usage_bytes`) | correct for the whole sandbox. Use these. |
| `container_memory_rss` | **misleading**: gVisor backs all application memory with one memfd, which the host counts as shared memory, not anonymous. RSS shows little more than the sandbox's own overhead. The dashboards do not use it for boxes. |
| pids (`container_threads`, `container_threads_max`) | **host threads of the sandbox**, not processes inside the box. The 512 limit is applied to those threads too, so "pids of limit" is still the number that matters for the limit, but it is not a process count. |
| processes, file descriptors, sockets inside the box | **invisible**. `container_processes` is the handful of `runsc` host processes. Not charted. |
| OOM kills | the host kernel kills the sandbox (the whole box dies, not one process); there is no OOM killer inside gVisor. cAdvisor reads the kill from the kernel log. The `BoxOOMKilled` alert uses the host-wide counter from node_exporter, because a killed box's own series can vanish before it is scraped twice. |
| network bytes and packets | read from the box's network namespace, where the sandbox uses the real veth; expected to be right. Drops inside gVisor's own network stack are not visible. |
| disk IO (`container_fs_reads/writes_bytes_total`) | what the sandbox's host processes read and wrote; a series exists only once a box has done IO. Not verified against gVisor's file proxy. |
| filesystem usage per box | not collected (and not meaningful: the box's root overlay lives inside the sandbox) |

Verified: the gVisor facts above are from gVisor's and cAdvisor's documentation and source, and
the cgroup metrics were exercised locally on `runc` containers. Not verified: any of it on a real
`runsc` container, because Docker Desktop has no gVisor. So two things are checked on the host in
step 5:
that cAdvisor lists every `arbos-box-*` by name, and that it reports the 16 GiB memory limit (if
Docker's cgroup driver and `runsc` disagree, the limit is not on the cgroup cAdvisor reads and the
"against their limits" table is empty). The same step looks at the boxes' network counters: gVisor's
network stack sends through the real veth in the box's namespace, so the host-side counters should
move; if they read 0 for every box, "Top boxes by network" and the drill-down Network panel are
flat for boxes and nothing else is affected.

For numbers from inside a box (process count, application-only memory) gVisor has its own
commands, `runsc events --stats <id>` and `runsc usage <id>`, and a Prometheus endpoint
(`runsc metric-server`) that needs a flag in `/etc/docker/daemon.json` and a Docker restart. Not
part of this stack.

## Relay and agent metrics

The names are the relay's and agentd's own, listed in [`docs/metrics.md`](../../../docs/metrics.md). Prometheus reaches them
through the relay's metrics listener, which is loopback-only and needs no credentials:

| job | scrapes |
|---|---|
| `relay` | `http://127.0.0.1:9108/metrics` |
| `relay-boxes` | `http://127.0.0.1:9108/metrics/boxes`, every 5 s: the relay's own view of each registered box (state, turn running, sockets, tokens, calls, spend), ten series per box |
| `arbos-agentd` | one target per awake box, discovered from `http://127.0.0.1:9108/metrics/targets`; the relay proxies each scrape to that box's agentd. The `box` label is an opaque keyed id: it is not a user id and cannot be derived from one. A sleeping box is absent, not down. A box is its user's machine, so its answer is untrusted: the relay passes on only agentd's own lines, and the job caps a scrape at 500 samples and 512 KB. |

The listener exists only when the relay runs with `ARBOS_RELAY_METRICS_LISTEN=127.0.0.1:9108` in
`/etc/arbos/relay.env` (`scripts/deploy/relay-server.sh` keeps the variable across deploys). The
relay reads it at start, and a relay restart drops every open WebSocket, so it is switched on in
the planned relay deploy and never by a restart of its own (runbook steps 8 and 9). Until then the
relay and agent panels show "–", the two jobs are down or empty, and nothing alerts on them
(`ScrapeTargetDown` ignores a target that has never answered). The agent metrics and box discovery
arrive with a later relay release and with boxes on a newer image (#3096).

A box on an image from before agentd had `/metrics` is listed but cannot be scraped. The relay
counts awake boxes by how their last scrape ended (`arbos_relay_box_scrapes`), and the Agents
dashboard shows that as "Boxes reporting", "On old image" and "Unreachable"; turn numbers cover only
the boxes that report. `BoxesUnreachable` warns about a box that should answer and does not; a box
on an old image raises nothing.

A counter with labels does not exist until its first event, so a healthy relay has no "failed"
series. Failure panels and the Subnet view read 0 in that case while the relay (or any agentd) is
reporting, and stay empty when there is no data at all. Relay up,
memory, CPU and restarts, and every host and container panel, do not depend on it.

## Swarm and the relay

The Swarm dashboard joins two sources per box: cAdvisor (CPU, memory, pids, network; there today)
and the relay's `/metrics/boxes` (state, tokens, calls, spend; there once the relay runs a release
with it, #3129). `arbos_relay_box_info{box, container}` is the join: `container` is the name
cAdvisor reports.

| | relay as deployed today | relay with `/metrics/boxes` |
|---|---|---|
| Swarm | every running box by CPU and memory, grey (state unknown), on the floor | tokens per minute on the vertical axis, colour by state, sleeping boxes at the origin |
| Network against CPU | live | live |
| Working, Running, Calls / min, Spend, Boxes by state | live (the relay's fleet gauges) | live |
| Tokens / s, / min, / 24 h; token charts; table columns Tokens, Calls, Spend | "after the next relay update" | live |
| Arbos, both copies: token numbers and token chart | "after the next relay update" | live |
| Arbos, the team's swarm | as Swarm above | as Swarm above |
| Arbos, the partner's machines by tokens per minute | "after the next relay update" | counts per group |
| Arbos, everything else | live | live |

Nothing is configured when the relay changes: the `relay-boxes` job answers 404 until then, alerts
on nothing (`ScrapeTargetDown` leaves out the relay's jobs; `RelayMetricsOff` is the alert for the
listener), and the panels fill by themselves.

A model call is counted when it ends, because the provider reports usage last. Token rates
therefore move in steps, one per finished call, not with the stream.

For about half a minute after a relay restart its counters are there but have no rate yet. The
token figures then say "relay just restarted", which is a different thing from "after the next
relay update" (a relay that does not count tokens at all).

The swarm's colour puts tokens first: a box that used tokens in the last minute is "working"
whatever the relay believes about it. The relay's own state can be stale (it has just restarted
and not heard from the box, or lost its heartbeat), and its turn flag drops at the first keepalive
after a turn while that turn's tokens are still in the rate.

"Tokens / 24 h" is the last 24 hours, not the calendar day, and counts from the relay's start if
that is more recent: the counters live in the relay's memory and restart at zero with it.

**Cost of the 5 s scrape.** Ten series per registered box: about 720 series for 72 boxes and 1,500
at the cap of 150, roughly 60–120 KB per scrape, written by the relay from memory with no
database read. In Prometheus that is 12–26 million samples a day; most are constants, so expect
under 0.5 GB over the 15 days of retention. The scrape is the only thing here faster than 15 s;
if it has to be slower, change `scrape_interval` of `relay-boxes` and the windows that are shorter
than four scrapes (`[20s]` in `dashboards.py`).

## What the partner sees of the machines

The team's copy of Arbos has the swarm: one dot per box. The partner's copy has the same fleet as
counts, **machines by tokens per minute**, and nothing per machine leaves the main Prometheus in
any form.

Why not anonymous dots. A dot per machine was built first, each under a random number redrawn
every five minutes, with rounded values. The label was untraceable and the dots were not: a dot
carries its machine's own numbers, the partner's Prometheus keeps 15 days of them, and a Viewer
can ask it for ranges. The dot at 37,000 tokens a minute, 1.3 cores and 2.5 GiB under one number
and the dot with the same figures under another, 30 seconds later, are one machine. Matching
nearest neighbours re-links every active machine at every draw, which makes it a per-machine
history under a pseudonym. So no per-machine series is exported, under any label.

What leaves instead is coarse on purpose, so that comparing two samples does not show one
machine arriving, leaving or changing what it does:

- **Every count of machines** leaves as 0, as "<5" for one to four, or rounded to the nearest
  five. That covers machines by tokens per minute (`subnet:boxes_by_token_rate{bucket}`: idle,
  under 10K, 10K to 100K, 100K to 1M, over 1M; five-minute average) and machines by state
  (working, awake and idle, waking, asleep, error, waiting, running). No exact total is exported
  beside them. `bucket` is the only label any `subnet:*` series carries.
- **Every amount of tokens or calls** leaves at two significant figures: tokens per minute,
  tokens in 24 hours, model calls per minute. An exact running total would let a reader take
  differences and get back a fine-grained rate. While fewer than 5 machines are using tokens the
  token rate has one significant figure.
- **Tokens are a total**, not split by kind. With public list prices, input, output and cache
  tokens separately are a dollar figure; the split is on the team's copy only. The total still
  bounds the spend loosely, and that is accepted.
- **Spend** is a share of the daily cap rounded to the whole percent (`subnet:spend_cap_ratio`),
  never an amount.
- The team's copy reads the exact figures behind these (`arbos:*` rules of the same names), which
  the partner's Prometheus does not have.

What is hidden and what is not.

- Hidden: any machine's own series; which machine is which; exact counts and exact totals, and
  so the single step of one machine moving between groups or states in most cases.
- Not hidden: a count crossing a rounding boundary (7 to 8 machines shows as 5 to 10; 4 to 5 as
  "<5" to 5; 0 to 1 as 0 to "<5"). One such step can coincide with one machine's move, and a
  reader sees that some machine moved, never which.
- Not hidden: when one machine is active alone, a reader sees which token band it is in ("<5" in
  one group, 0 in the other active groups) and the fleet's token rate to one figure. That is that
  machine's activity, coarsely. Nothing names it.
- Not hidden: events of the fleet as rates (machine starts and failed starts per minute, the
  share of model calls that failed). They say that something happened, not to whom.
- This is rounding, not a formal privacy guarantee.

`check-rules.py` holds the numbers (5, nearest 5, two figures, one percent) and the exact
expression every such rule must have; it fails if a rule could let a counter or a label leave or
passes a count or an amount on uncoarsened. `prometheus/rules_test.yml` proves the same on
labelled input, and `local/check-dashboards.py` asserts the label set of the running Subnet
datasource.

Not covered here: `subnet:turns_active` and `subnet:queue_depth` are exact sums of what agentd
reports. They are on no dashboard and have no data until boxes report (#3096); coarsen them the
same way before they are shown to the partner.

## Decisions

- **node_exporter, not the Netdata already on the host.** Netdata can export Prometheus format,
  but under its own names and as its own averages, with a chart for every one of 125 containers:
  the dashboards and alerts would be written against one Netdata version's naming, and its
  payload would dwarf everything else scraped here. node_exporter is one static binary using
  ~20 MB with the names every Prometheus rule and dashboard expects. Netdata is left untouched.
- **A second Prometheus for the Subnet view**, see Access model. It costs ~60 MB.
- **Host networking**, like LiveKit and Synapse here: the relay and its metrics listener are on
  loopback.
- **No setup script and no systemd unit**, unlike LiveKit and Synapse: `restart: unless-stopped`
  brings the containers back with Docker, and the first deployment is the explicit steps below,
  each of which can be read before it is run.
- **Relay memory, CPU and restarts come from cAdvisor** reading the `arbos-relay.service` cgroup,
  so they work before the relay has `/metrics`.
- **Alertmanager speaks Discord natively** (`discord_configs`); the URL reaches it through a file
  written at start from the env file, because Alertmanager does not expand variables.
- **Netdata may notice the new exporters.** Its collector auto-detects Prometheus endpoints on
  well-known local ports (9090, 9100), so charts for them can appear in Netdata. Harmless and a
  little extra work for Netdata; nothing here depends on it either way.
- **Versions are pinned** to the newest patch release that was at least a few weeks old on
  2026-11-18 (cAdvisor's images moved from gcr.io to ghcr.io after v0.55).

## Run it locally

Needs Docker Desktop and ~3 GB of disk for images.

```bash
cd scripts/deploy/monitoring
./grafana-access.sh --check local/local.env
docker compose --env-file local/local.env -f docker-compose.yml -f docker-compose.local.yml up -d
./grafana-access.sh local/local.env
python3 local/check-dashboards.py      # after ~2 minutes of data: every panel's query returns data
open http://127.0.0.1:3300             # admin / the password in local/local.env
```

The local override runs a fake relay (`local/fake-relay.py`: `/healthz`, `/metrics`, the box
discovery and per-box metrics, with the names from [`docs/metrics.md`](../../../docs/metrics.md) and counters that move) and
three containers named `arbos-box-test_*`. For its first five minutes the fake has no failure series,
like a healthy relay, and it lists one box that answers 502. The
config files are the production ones, unchanged. Differences from production: the containers share
one network namespace instead of the host's; the public probes hit the real `relay.arbos.life` and
`arbos.life` (two GETs every 30 s); the relay's systemd-service series are faked, since a Mac has
no such cgroup; and the test boxes run on `runc`, so nothing here exercises gVisor.

The fake also stands in for a fleet: 40 more boxes as cAdvisor and the relay would report them
(`/metrics/boxes`), so the Swarm dashboard has a swarm to show.

`FAKE_BOXES=0` makes the fake the relay production runs today: no box reports, no `/metrics/boxes`,
no token counter. The Subnet view and the Swarm must then have data everywhere except the token
figures, which say so:

```bash
FAKE_BOXES=0 docker compose --env-file local/local.env -f docker-compose.yml -f docker-compose.local.yml up -d
./grafana-access.sh local/local.env && python3 local/check-dashboards.py --relay-only
```

`./check.sh` runs the static checks without starting anything. Tear down, data and images included:

```bash
docker compose --env-file local/local.env -f docker-compose.yml -f docker-compose.local.yml down -v --rmi all
```

## Production runbook

On the relay host, as root, in this order. Each step says what it changes and how to undo it; the
undo commands stand alone. Nothing here touches the boxes, Docker's configuration or the Netdata
already on the host.

**Who is interrupted, and when.** Steps 1–7 interrupt nobody and leave a working stack that is used
over an SSH tunnel. Publishing Grafana needs a reload of the `:443` Caddy, and the relay's metrics
need a relay restart. Both close every open WebSocket (`scripts/deploy/arbos-deploy-worker` records
49–57 boxes dropped by each Caddy reload), so neither is done on its own: step 8 only stages them,
and step 9 is the **one** window: a single relay deploy, run by hand through the deploy lane, that
does both.

**Cost.** Memory: the caps add up to 4.6 GB; expect about 1.5 GB, most of it Grafana, which grows
to its cap (the host has 503 GB). CPU: well under one core; cAdvisor is capped at two. Disk: ~2.6 GB
of images, at most 16 GB of metrics (expect ~3–5 GB at 15 days with 125 containers), at most 210 MB
of logs — under 19 GB worst case, 1.5 % of the disk.

### 1. Check the host — changes nothing

```bash
ss -ltnH | grep -E ':(3300|9090|9091|9093|9100|9115|9180)\s' || echo "ports free"
docker compose version
df -h / /var/lib/docker
ls -l /run/docker.sock /run/containerd/containerd.sock
docker info --format '{{.Driver}} / {{.CgroupDriver}} / cgroup v{{.CgroupVersion}}'
getent hosts relay.arbos.life; curl -s https://api.ipify.org; echo
curl -fsS -o /dev/null -w '%{http_code}\n' http://127.0.0.1:7700/healthz
curl -fsS -o /dev/null -w '%{http_code}\n' https://relay.arbos.life/healthz
```

Expected: `ports free`, both sockets listed, `overlayfs` as the driver, `relay.arbos.life` resolving
to the server's own public address (the second line; it is a DNS-only record, not proxied, which
is why the partner's "Reachable from outside" tile is built on it and not on `arbos.life`), `200`
twice. Stop if a port
is taken (it would have to change in `docker-compose.yml`, `prometheus/prometheus.yml` and the
Grafana datasource together) or a socket is missing.

### 2. Put the files on the host — adds `/etc/arbos-monitoring`

From a checkout of `main` at the merged commit. `safe.directory` is for running git as root in a
checkout another user owns; CI has already run `check.sh` on that commit, so it is not run here
(it pulls images).

```bash
REPO=/path/to/the/arbos/checkout
install -d -m 0755 /etc/arbos-monitoring
git -c safe.directory="$REPO" -C "$REPO" archive HEAD scripts/deploy/monitoring | tar -x -C /etc/arbos-monitoring --strip-components=3
ls /etc/arbos-monitoring/docker-compose.yml
```

Undo: `rm -r /etc/arbos-monitoring`

### 3. Write the env file — adds `/etc/arbos/monitoring.env`

```bash
test -e /etc/arbos/monitoring.env && echo "exists: keeping it" || install -m 0600 /etc/arbos-monitoring/.env.example /etc/arbos/monitoring.env
openssl rand -hex 24      # run twice: the Grafana admin password, then the viewer password
$EDITOR /etc/arbos/monitoring.env
/etc/arbos-monitoring/grafana-access.sh --check
```

Fill in `GF_SECURITY_ADMIN_PASSWORD`, `SUBNET_VIEWER_PASSWORD` and `ALERT_WEBHOOK_URL` (see Alerts);
`GF_SERVER_ROOT_URL` is already right. The last command changes nothing and must print
`/etc/arbos/monitoring.env: ok`. Grafana reads its admin password once, at first start, and will
not start on a weak one.

Undo: `rm /etc/arbos/monitoring.env`

### 4. Start the stack — **changes production: pulls ~2.6 GB of images, starts 7 containers**

```bash
cd /etc/arbos-monitoring
docker compose --env-file /etc/arbos/monitoring.env up -d
docker compose --env-file /etc/arbos/monitoring.env ps
```

All seven are `Up` (cAdvisor `healthy` after 30 s). Every listener is on loopback: nothing is
reachable from outside, and no existing service was touched.

The relay has no metrics endpoint until step 9, which `RelayMetricsOff` would report to Discord
ten minutes from now and every 12 hours after. Silence it for a day; the silence expires by itself:

```bash
docker exec arbos-mon-alertmanager amtool silence add alertname=RelayMetricsOff --duration=24h \
  --comment='until the relay deploy that turns on its metrics listener' --alertmanager.url=http://127.0.0.1:9093
```

Undo, keeping the data: `cd /etc/arbos-monitoring && docker compose --env-file /etc/arbos/monitoring.env down`
Undo, deleting data and images: `cd /etc/arbos-monitoring && docker compose --env-file /etc/arbos/monitoring.env down -v --rmi all`

### 5. Check what could not be checked off the host — changes nothing

```bash
# every target: 1 = scraped. relay reads 0 until step 9. No box target is listed until the relay
# release with box discovery (#3096) is deployed and boxes run an image that reports.
curl -s '127.0.0.1:9090/api/v1/query?query=up' | python3 -c 'import json,sys; [print(r["value"][1], r["metric"]["job"], r["metric"]["instance"]) for r in json.load(sys.stdin)["data"]["result"]]'
# every probe: 1 = answered. The relay on loopback and relay.arbos.life must be 1; arbos.life may be 0.
curl -s '127.0.0.1:9090/api/v1/query?query=probe_success' | python3 -c 'import json,sys; [print(r["value"][1], r["metric"]["job"], r["metric"]["instance"]) for r in json.load(sys.stdin)["data"]["result"]]'
# cAdvisor names every box: these two numbers are equal
curl -s 127.0.0.1:9180/metrics | grep -c '^container_last_seen{.*name="arbos-box-'
docker ps -q --filter name=arbos-box- | wc -l
# gVisor boxes report their 16 GiB limit (1.7179869184e+10), not 0
curl -s 127.0.0.1:9180/metrics | grep '^container_spec_memory_limit_bytes{.*name="arbos-box-' | head -3
# gVisor boxes have moving network counters: a non-zero number on eth0 for a box that is in use
curl -s 127.0.0.1:9180/metrics | grep '^container_network_receive_bytes_total{.*interface="eth0".*name="arbos-box-' | head -3
# cAdvisor could open the kernel log: this prints nothing
docker logs arbos-mon-cadvisor 2>&1 | grep -i 'disabling OOM events'
```

Until box discovery exists, Prometheus logs one line a minute, `Unable to refresh target groups …
config=arbos-agentd` (connection refused before step 9, HTTP 404 after it). That is the box job
finding no list of boxes; it is expected and stops by itself with #3096.

What each outcome means:

- **A probe reads 0**: for `http://127.0.0.1:7700/healthz` the relay is not answering on loopback
  and `RelayDown` will page: stop and look. For `https://relay.arbos.life/healthz` see the note on
  `/etc/hosts` below. `https://arbos.life/` may read 0 while the site is fine: it is behind
  Cloudflare, which can answer a probe from a server with a challenge. That costs one warning
  (`PublicProbeFailing`) and nothing on the partner's view, which does not use that probe.
- **Box count 0, or lower than `docker ps`**: cAdvisor cannot reach Docker or containerd.
  `docker logs arbos-mon-cadvisor 2>&1 | grep -i factory` names the socket. The Boxes dashboard is
  empty until that is fixed; every host panel and the relay's own numbers are unaffected. Do not
  give the container more privileges to get past it; record the log line in the PR.
- **Limit reads 0**: `runsc` did not put the limit on the cgroup cAdvisor reads. "Boxes against
  their limits" and "Over 80 % memory/pids" stay empty; CPU, memory and pids per box still work.
- **Network counters 0 or absent for boxes**: the box network panels are flat; nothing else changes.
- **The OOM line is printed**: the per-box OOM series is empty; the `BoxOOMKilled` alert still works.

If the public probes fail from the host but the site works from outside, the host cannot reach its
own public IP; add `127.0.0.1 relay.arbos.life arbos.life` to `/etc/hosts` (the probe then still
goes through Caddy and checks the certificate).

### 6. Subnet organisation, viewer, home pages — **changes Grafana only**

```bash
/etc/arbos-monitoring/grafana-access.sh
```

It ends with `viewer '…' can open the Subnet view and nothing else`, having logged in as the viewer
and tested that, that the viewer has no Explore, and that the viewer's home is the Subnet view.
Safe to run again. **Do not hand the viewer login over yet** (step 10).

Undo: not needed on its own; the account lives in Grafana's volume and goes with `down -v`.

### 7. Use it: SSH tunnel and a test alert — posts one message to the Discord channel

From your own machine, in Chrome or Firefox (they accept Grafana's Secure cookie on `localhost`;
Safari does not):

```bash
ssh -N -L 3300:127.0.0.1:3300 <the relay host>
```

then open `http://localhost:3300` and log in as `admin`: Arbos is the home page. Host,
container, probe and relay-process panels have data now; the relay's own and the agent panels show
"–" until step 9.

```bash
docker exec arbos-mon-alertmanager amtool alert add Test severity=warning --annotation='summary="test alert"' --alertmanager.url=http://127.0.0.1:9093
```

The message arrives within 30 seconds and resolves itself after five minutes. If nothing arrives,
`docker logs arbos-mon-alertmanager` shows the refusal; fix `ALERT_WEBHOOK_URL` and run
`docker compose --env-file /etc/arbos/monitoring.env up -d alertmanager`.

The stack is complete for the operators at this point. Everything below is about the public address
and the relay's own metrics.

### 8. Stage the public address and the relay's metrics — **changes production: one DNS record, two files; interrupts nobody**

Nothing in this step reloads or restarts anything.

DNS: add an A record `grafana.arbos.life` → the server's public IP, **DNS-only** (not proxied), like
`relay.arbos.life`; the certificate comes from the TLS-ALPN challenge on 443.

```bash
WWW_DIR=/home/const/arbos-www
sed 's#@GRAFANA_HOST@#grafana.arbos.life#g' /etc/arbos-monitoring/Caddyfile.grafana > /etc/arbos/sites.d/grafana.caddy
CLOUDFLARE_DNS_API_TOKEN=validate-only-placeholder "$WWW_DIR/bin/caddy" validate --config "$WWW_DIR/Caddyfile" --adapter caddyfile \
  || { rm /etc/arbos/sites.d/grafana.caddy; echo "INVALID: removed the vhost again"; }
grep -q '^ARBOS_RELAY_METRICS_LISTEN=' /etc/arbos/relay.env || echo 'ARBOS_RELAY_METRICS_LISTEN=127.0.0.1:9108' >> /etc/arbos/relay.env
```

`validate` only reads the configuration; a vhost that does not validate is removed on the spot, so
it cannot break the next reload. The relay reads its variable only at start, and
`scripts/deploy/relay-server.sh` keeps it across deploys.

Both now wait for the next Caddy reload and the next relay start. If something else reloads Caddy
first (`refresh-mac-download` does when a download snippet changes), Grafana's login page becomes
reachable earlier than planned; that is safe (login required, no default password) and is not an
interruption of ours.

Undo: `rm /etc/arbos/sites.d/grafana.caddy`, `sed -i '/^ARBOS_RELAY_METRICS_LISTEN=/d' /etc/arbos/relay.env`, delete the DNS record.

### 9. The window: one relay deploy — **changes production: interrupts every connected user and box once**

Relay deploys are manual on this host (`AUTO_DEPLOY_RELAY=0` in `/etc/arbos/deploy.env`), so
nothing does this by itself. One command deploys the relay at a commit of `main` that contains its
metrics endpoint (#3086 or later, [`docs/metrics.md`](../../../docs/metrics.md)): it builds, runs
`relay-server.sh`, restarts the relay and reloads `arbos-www`. The relay comes back with its metrics
listener and Caddy starts serving `grafana.arbos.life` in the same interruption. Chats show
"Reconnecting…" and boxes reconnect, once.

It is run through the deploy lane, as the deploy worker runs it, so that no canary smoke or other
reload overlaps it:

```bash
SHA=        # the commit of main to deploy, in full
( [[ $SHA =~ ^[0-9a-f]{40}$ ]] || { echo "set SHA to the full commit id first"; exit 1; }
  . /usr/local/bin/deploy-state.sh; export STATE=/var/lib/arbos-deploy LANE_MARKER_MAX_S=1800
  deploy_lane_run relay env BUILD_USER=const bash /home/const/arbos-build/repo/scripts/feedback/deploy-relay.sh "$SHA" ); echo "exit $?"
curl -fsS -o /dev/null -w '%{http_code}\n' http://127.0.0.1:7700/healthz
```

`exit 0` and `200`: deployed. `exit 75`: a canary smoke or another deploy holds the lane; nothing
happened, run it again when that is done. If it keeps answering 75 with no `relay.deploying`,
`agentd.canary` or `www.reloading` file in `/var/lib/arbos-deploy`, a process left by an earlier lane
command holds the lock: `find /proc/[0-9]*/fd -lname /var/lib/arbos-deploy/.lane.lock` names it. `exit 1` with `set SHA to the full commit id first`: the commit
was not filled in and nothing ran (the deploy script would otherwise take main's head). Any other exit: the
deploy script failed; its output is above, and the relay keeps running what it ran (the script
never rolls back).

After a successful relay deploy, and only then, lift the silence from step 4, so that a relay that
later loses its metrics endpoint is reported:

```bash
docker exec arbos-mon-alertmanager sh -c 'amtool silence expire $(amtool silence query -q alertname=RelayMetricsOff --alertmanager.url=http://127.0.0.1:9093) --alertmanager.url=http://127.0.0.1:9093'
```

Do not run `systemctl reload arbos-www` or restart the relay by hand for this stack. If Grafana has
to be published with no relay deploy in sight, that is a user-interrupting step of its own and also
goes through the lane:

```bash
( . /usr/local/bin/deploy-state.sh; export STATE=/var/lib/arbos-deploy LANE_MARKER_MAX_S=1800
  deploy_lane_run www systemctl reload arbos-www.service ); echo "exit $?"
```

That reload does not turn on the relay's metrics, so the silence from step 4 stays.

After the window:

```bash
curl -s '127.0.0.1:9090/api/v1/query?query=up{job="relay"}' | grep -o '"value":[^}]*'      # ends in "1"
curl -sS -o /dev/null -w '%{http_code}\n' https://grafana.arbos.life/login                           # 200
curl -sS -o /dev/null -w '%{http_code}\n' https://grafana.arbos.life/metrics                         # 404
curl -sS -o /dev/null -w '%{http_code}\n' https://grafana.arbos.life/api/dashboards/uid/arbos-fleet  # 401
```

The relay panels fill within a minute: boxes by state, slots used, spend against the cap, kill
switch, model calls, wake queue.

Undo: `rm /etc/arbos/sites.d/grafana.caddy` and remove the variable as in step 8; each takes effect
at the next reload or relay start, or at once at the cost of another interruption.

### 10. Hand over the Subnet view — after step 9, not before

Log in at `https://grafana.arbos.life` as the viewer in a private window. The Subnet view is the
home page and there is nothing else. Before step 9 most of it reads "no data yet", because it is
built on the relay's metrics; after step 9 every tile and every panel on it is live. Only then give
the viewer the address and their login, over a private channel.

It does not show agent turns: those figures can only come from the machines, which do not report
them yet (#3096 and an image roll). Say so when handing over; the panels are added then.

## Updating a running stack — interrupts nobody

For a change to dashboards, rules or scrape jobs (nothing in `docker-compose.yml`). On the relay
host, as root, from a checkout of `main` at the merged commit:

```bash
REPO=/path/to/the/arbos/checkout
git -c safe.directory="$REPO" -C "$REPO" archive HEAD scripts/deploy/monitoring | tar -x -C /etc/arbos-monitoring --strip-components=3
docker exec arbos-mon-prometheus promtool check config /etc/prometheus/prometheus.yml
docker kill -s HUP arbos-mon-prometheus
/etc/arbos-monitoring/grafana-access.sh
```

`promtool` must end in `SUCCESS` before the next line; `HUP` makes Prometheus re-read its
configuration and rules without restarting (no gap in the data). Grafana loads new and changed
admin dashboards from the files within a minute. `grafana-access.sh` reloads the Subnet view; it
is safe to run again. No container restarts, and nothing outside the stack is touched.

If `docker-compose.yml` itself changed, `cd /etc/arbos-monitoring && docker compose --env-file
/etc/arbos/monitoring.env up -d` re-creates only the services whose definition changed.

Undo: the same commands from the previous commit.

## Rolling back everything

Each step's undo above stands alone. To remove the whole stack:

```bash
rm -f /etc/arbos/sites.d/grafana.caddy          # takes effect at the next Caddy reload; do not reload for it
cd /etc/arbos-monitoring && docker compose --env-file /etc/arbos/monitoring.env down -v --rmi all
rm -r /etc/arbos-monitoring /etc/arbos/monitoring.env
```

Until that next reload Caddy answers `grafana.arbos.life` with a 502, since nothing listens behind
it. Then delete the DNS record and the Discord webhook. The relay's metrics listener can stay: it
is loopback-only. To stop one noisy piece without the rest:
`docker compose --env-file /etc/arbos/monitoring.env stop cadvisor` (the container panels go empty,
everything else keeps working).
