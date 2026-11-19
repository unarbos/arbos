# Monitoring for the relay host

Prometheus, Grafana and the exporters that answer two questions about the host that runs the relay
and every box: *are we OK right now*, and *what is degrading*. Everything is a file in this folder;
nothing is configured by clicking.

The stack is not deployed by merging this. [Production runbook](#production-runbook) is the ordered
list of steps, each marked when it changes production.

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
| `blackbox` | `prom/blackbox-exporter:v0.29.1` | 9115 | 64 MB | probes the relay's `/healthz` on the host, and `relay.arbos.life` / `arbos.life` over HTTPS (certificate expiry included) |

Files:

| path | |
|---|---|
| `docker-compose.yml` | the services above |
| `prometheus/prometheus.yml` | scrape jobs |
| `prometheus/rules.yml` | alerts |
| `prometheus/subnet-rules.yml`, `prometheus/subnet.yml` | the aggregates behind the Subnet view, and the Prometheus that holds only them |
| `prometheus/rules_test.yml` | unit tests for both rule files |
| `alertmanager/alertmanager.yml`, `blackbox/blackbox.yml` | their configs |
| `grafana/provisioning/` | datasource and dashboard loading for the main organisation |
| `grafana/dashboards/*.json`, `grafana/dashboards-subnet/subnet.json` | the dashboards, written by `dashboards.py` |
| `grafana-access.sh` | `--check` validates the env file (weak or malformed values are refused); without it, creates the Subnet organisation, its datasource, dashboard and the one viewer account |
| `Caddyfile.grafana` | the vhost for Grafana |
| `.env.example` | the secrets the stack needs; the filled-in copy is never committed. Compose refuses to start without them |
| `check.sh` | static checks (CI runs it) |
| `docker-compose.local.yml`, `local/` | running it on a laptop against fake targets |

## Dashboards

- **Arbos fleet** (home), three rows of single stats then time series. Service: relay and public
  probes, kill switch, today's spend against the daily cap, model errors and response time, relay
  restarts, targets down. Boxes: running, slots used against `max_boxes`, asleep, waitlisted, start
  failures, wake queue. Host: CPU, memory, disk free and days until full, load, relay memory,
  sockets, database writer wait.
- **Boxes**: top-N boxes by CPU, memory, pids, disk IO and network; every box against its memory
  and pids limit; OOM kills and restarts; a drill-down by the `Box` variable.
- **Agents**: turns started / finished / failed, turn duration percentiles, messages waiting,
  crashes, model calls, errors and response time, and how many awake boxes are reporting.
- **Subnet view** (separate organisation): service up, reachable from outside, machines running,
  turn success rate and duration, queue, host headroom. Aggregates only.

To change a panel: edit the query in `dashboards.py`, run `python3 dashboards.py`, commit both.
Grafana reloads the files within a minute; the Subnet view is reloaded by `grafana-access.sh`.

## Alerts

`prometheus/rules.yml`: `RelayDown`, `HealthProbeFailing`, `TLSExpirySoon` (< 14 days),
`ScrapeTargetDown`, `DiskFreeLow` (< 15 %), `DiskFullWithin24h`, `HostMemoryLow` (< 10 %
available), `BoxStartFailures` (failed wake jobs), `Waitlist` (anyone waiting for 15 minutes),
`WaitlistGrowing` (three more in 30 minutes), `BoxCapacityHigh` (90 % of
`max_boxes` admitted), `SpendAt60Percent` and `SpendAt85Percent` (of the global daily cap, which
trips the kill switch and stops every agent), `KillSwitchOn`, `ModelErrorsHigh` (> 10 % of calls),
`ModelSlow` (p95 > 30 s to first byte), `BoxesUnreachable` (awake boxes not answering the relay's
metrics scrape), `BoxOOMKilled`, `RelayRestarted`, `RelayMemoryHigh`
(> 40 GiB; the relay sat at ~22 GB when this was written — tune the number in the rule).

All go to one receiver. Each carries a severity, shown in the message title: `critical` (relay
down, public probe failing, disk full within a day, memory low, kill switch on, spend at 85 %,
waitlist growing, model errors, OOM kill) repeats every 4 hours while it lasts; `warning` (the
rest) every 12, so a standing condition such as a short waitlist does not fill the channel.
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
  without it (Grafana would otherwise fall back to `admin`/`admin`), and
  `grafana-access.sh --check` refuses a short, repetitive or dictionary password before the
  first start.
- **Each container gets only its own secret**: Grafana its password and URL, Alertmanager the
  webhook URL. The viewer's password is read only by `grafana-access.sh`.
- **Two organisations.** The main one holds Fleet, Boxes and Agents and the full Prometheus; only
  the admin is in it. The **Subnet** organisation holds one dashboard and one datasource.
- **The viewer account** (`SUBNET_VIEWER_LOGIN`, default `tobias`) is a Viewer in the Subnet
  organisation and a member of nothing else.
- **Why a second Prometheus.** A Grafana Viewer can send any query to any datasource of their
  organisation, whatever dashboards they are allowed to open; hiding a dashboard is not a
  boundary. So the Subnet organisation's datasource is an instance that only ever receives the
  `subnet:*` recording rules, each of which aggregates every label away. There is no query that
  returns a user id, a container name or a hostname, because that data is not there.
  `prometheus/rules_test.yml` feeds the rules labelled input and fails if a label comes out;
  `grafana-access.sh` ends by logging in as the viewer and failing unless the admin dashboards
  and datasource refuse it.
- Container names contain user ids. They appear on the Boxes dashboard only: not in the Subnet
  view and not in alert messages. (The `box` label on agent metrics is an opaque id.)

## What cAdvisor is given

It runs unprivileged: every capability dropped except `SYSLOG`, `no-new-privileges`, a read-only
root filesystem, and four read-only mounts.

| given | for | without it |
|---|---|---|
| `/sys` | the cgroup tree: CPU, memory, pids, IO per container | nothing works |
| `/proc` (as `/rootfs/proc`) | network counters, read from each container's network namespace | no network panels |
| `/run/docker.sock` | container **names** and images | containers are anonymous cgroup ids, and a box cannot be told from any other container |
| `/run/containerd/containerd.sock` | cAdvisor's Docker support will not start without it when Docker uses the containerd image store | as above |
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
step 5 before anything is published:
that cAdvisor lists every `arbos-box-*` by name, and that it reports the 16 GiB memory limit (if
Docker's cgroup driver and `runsc` disagree, the limit is not on the cgroup cAdvisor reads and the
"against their limits" table is empty).

For numbers from inside a box (process count, application-only memory) gVisor has its own
commands, `runsc events --stats <id>` and `runsc usage <id>`, and a Prometheus endpoint
(`runsc metric-server`) that needs a flag in `/etc/docker/daemon.json` and a Docker restart. Not
part of this stack.

## Relay and agent metrics

The names are the relay's and agentd's own, listed in `docs/metrics.md`. Prometheus reaches them
through the relay's metrics listener, which is loopback-only and needs no credentials:

| job | scrapes |
|---|---|
| `relay` | `http://127.0.0.1:9108/metrics` |
| `arbos-agentd` | one target per awake box, discovered from `http://127.0.0.1:9108/metrics/targets`; the relay proxies each scrape to that box's agentd. The `box` label is an opaque id, not a user id. A sleeping box is absent, not down. |

The listener exists only when the relay runs with `ARBOS_RELAY_METRICS_LISTEN=127.0.0.1:9108`.
`/etc/arbos/relay.env` is rebuilt by `scripts/deploy/relay-server.sh` from its key lists, so the
variable has to be in those lists and the relay restarted; that belongs to the relay's release, not
to this stack, and the restart drops every open WebSocket (see step 9). Until then the relay and
agent panels show "–", the two jobs are down or empty, and nothing alerts on them
(`ScrapeTargetDown` ignores a target that has never answered).

A box on an image from before agentd had `/metrics` is listed but cannot be scraped. The relay
counts awake boxes by how their last scrape ended (`arbos_relay_box_scrapes`), and the Agents
dashboard shows that as "Boxes reporting", "On old image" and "Unreachable"; turn numbers cover only
the boxes that report. `BoxesUnreachable` warns about a box that should answer and does not; a box
on an old image raises nothing.

A counter with labels does not exist until its first event, so a healthy relay has no "failed"
series. Failure panels and the Subnet view read 0 in that case while the relay (or any agentd) is
reporting, and stay empty when there is no data at all. Relay up,
memory, CPU and restarts, and every host and container panel, do not depend on it.

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
discovery and per-box metrics, with the names from `docs/metrics.md` and counters that move) and
three containers named `arbos-box-test_*`. For its first five minutes the fake has no failure series,
like a healthy relay, and it lists one box that answers 502. The
config files are the production ones, unchanged. Differences from production: the containers share
one network namespace instead of the host's; the public probes hit the real `relay.arbos.life` and
`arbos.life` (two GETs every 30 s); the relay's systemd-service series are faked, since a Mac has
no such cgroup; and the test boxes run on `runc`, so nothing here exercises gVisor.

`./check.sh` runs the static checks without starting anything. Tear down, data and images included:

```bash
docker compose --env-file local/local.env -f docker-compose.yml -f docker-compose.local.yml down -v --rmi all
```

## Production runbook

On the relay host, as root, in this order. Each step says what it changes and how to undo it; the
undo commands stand alone. Nothing here touches the relay, the boxes, Docker's configuration or the
Netdata already on the host. The only existing service affected is the `:443` Caddy, reloaded once
in step 8.

**Cost.** Memory: the caps add up to 4.6 GB; expect about 1.5 GB, most of it Grafana, which grows
to its cap (the host has 503 GB). CPU: well under one core; cAdvisor is capped at two. Disk: ~2.6 GB
of images, at most 16 GB of metrics (expect ~3–5 GB at 15 days with 125 containers), at most 210 MB
of logs — under 19 GB worst case, 1.5 % of the disk.

### 1. Check the host — changes nothing

```bash
ss -ltnH | grep -E ':(3300|9090|9091|9093|9100|9115|9180)\s' || echo "ports free"
docker compose version
df -h /var/lib/docker
ls -l /run/docker.sock /run/containerd/containerd.sock
docker info --format '{{.Driver}} / {{.CgroupDriver}} / cgroup v{{.CgroupVersion}}'
curl -fsS -o /dev/null -w '%{http_code}\n' http://127.0.0.1:7700/healthz
curl -fsS -o /dev/null -w '%{http_code}\n' https://relay.arbos.life/healthz
ss -ltnH | grep -E ':9108\s' || echo "relay metrics listener not up yet (step 9)"
```

Expected: `ports free`, both sockets listed, `200` twice. Stop if a port is taken (it would have to
change in `docker-compose.yml`, `prometheus/prometheus.yml` and the Grafana datasource together).
If `/run/containerd/containerd.sock` does not exist, delete that one mount line from
`docker-compose.yml` after step 2; Docker would otherwise create a directory in its place.

### 2. Put the files on the host — adds `/etc/arbos-monitoring`

From a checkout of `main` at the merged commit (CI has already run `check.sh` on it; do not run it
here, it pulls images):

```bash
install -d -m 0755 /etc/arbos-monitoring
git archive HEAD scripts/deploy/monitoring | tar -x -C /etc/arbos-monitoring --strip-components=3
```

Undo: `rm -r /etc/arbos-monitoring`

### 3. Write the env file — adds `/etc/arbos/monitoring.env`

```bash
install -m 0600 /etc/arbos-monitoring/.env.example /etc/arbos/monitoring.env
openssl rand -hex 24      # run twice: the Grafana admin password, then the viewer password
$EDITOR /etc/arbos/monitoring.env
/etc/arbos-monitoring/grafana-access.sh --check
```

Fill in `GF_SECURITY_ADMIN_PASSWORD`, `SUBNET_VIEWER_PASSWORD` and `ALERT_WEBHOOK_URL` (see Alerts);
`GF_SERVER_ROOT_URL` is already right. The last command must print
`/etc/arbos/monitoring.env: ok`; it changes nothing. Do not go on until it does: Grafana reads its
admin password once, at first start.

Undo: `rm /etc/arbos/monitoring.env`

### 4. DNS — **changes production: one DNS record**

Add an A record `grafana.arbos.life` → the server's public IP, **DNS-only** (not proxied), like
`relay.arbos.life`: the certificate comes from the TLS-ALPN challenge on 443. Nothing answers on
that name until step 8.

Undo: delete the record.

### 5. Start the stack — **changes production: pulls ~2.6 GB of images, starts 7 containers**

```bash
cd /etc/arbos-monitoring
docker compose --env-file /etc/arbos/monitoring.env up -d
docker compose --env-file /etc/arbos/monitoring.env ps
```

All seven are `Up` (cAdvisor `healthy` after 30 s). Nothing is reachable from outside yet. Then
check what could not be checked off the host:

```bash
# every target: 1 = scraped. relay reads 0, and no box is listed, until step 9.
curl -s '127.0.0.1:9090/api/v1/query?query=up' | python3 -c 'import json,sys; [print(r["value"][1], r["metric"]["job"], r["metric"]["instance"]) for r in json.load(sys.stdin)["data"]["result"]]'
# cAdvisor names every box: these two numbers are equal
curl -s 127.0.0.1:9180/metrics | grep -c '^container_last_seen{.*name="arbos-box-'
docker ps -q --filter name=arbos-box- | wc -l
# gVisor boxes report their 16 GiB limit (1.7179869184e+10), not 0
curl -s 127.0.0.1:9180/metrics | grep '^container_spec_memory_limit_bytes{.*name="arbos-box-' | head -3
# cAdvisor could open the kernel log: this prints nothing
docker logs arbos-mon-cadvisor 2>&1 | grep -i 'disabling OOM events'
```

If the first number is 0, cAdvisor could not reach Docker: `docker logs arbos-mon-cadvisor 2>&1 | grep -i factory`
says which socket. If the host's security profile blocks the unprivileged container where Docker
Desktop did not, the fallback is `privileged: true` on the `cadvisor` service in place of
`cap_drop`/`cap_add`/`read_only`; note it in the PR if it was needed.

If the public probes fail from the host but the site works from outside, the host cannot reach its
own public IP; add `127.0.0.1 relay.arbos.life arbos.life` to `/etc/hosts` (the probe then still
goes through Caddy and checks the certificate).

Undo, keeping the data: `cd /etc/arbos-monitoring && docker compose --env-file /etc/arbos/monitoring.env down`
Undo, deleting data and images: `cd /etc/arbos-monitoring && docker compose --env-file /etc/arbos/monitoring.env down -v --rmi all`

### 6. Subnet organisation and viewer — **changes Grafana only**

```bash
/etc/arbos-monitoring/grafana-access.sh
```

It ends with `viewer '…' can open the Subnet view and nothing else`, having logged in as the viewer
and tested exactly that. Safe to run again.

Undo: not needed on its own; the account lives in Grafana's volume and goes with `down -v`.

### 7. Send a test alert — posts one message to the Discord channel

```bash
docker exec arbos-mon-alertmanager amtool alert add Test severity=warning --annotation='summary="test alert"' --alertmanager.url=http://127.0.0.1:9093
```

It arrives within 30 seconds and resolves itself after five minutes. If nothing arrives,
`docker logs arbos-mon-alertmanager` shows the refusal; fix `ALERT_WEBHOOK_URL` and run
`docker compose --env-file /etc/arbos/monitoring.env up -d alertmanager`.

### 8. Publish Grafana — **changes production: reloads the :443 Caddy**

```bash
WWW_DIR=/home/const/arbos-www
sed 's#@GRAFANA_HOST@#grafana.arbos.life#g' /etc/arbos-monitoring/Caddyfile.grafana > /etc/arbos/sites.d/grafana.caddy
CLOUDFLARE_DNS_API_TOKEN=validate-only-placeholder "$WWW_DIR/bin/caddy" validate --config "$WWW_DIR/Caddyfile" --adapter caddyfile \
  && systemctl reload arbos-www.service
```

If `validate` fails, nothing was reloaded: `rm /etc/arbos/sites.d/grafana.caddy` and stop. A reload
keeps open connections, the relay's WebSockets included. Then, from any machine:

```bash
curl -sS -o /dev/null -w '%{http_code}\n' https://grafana.arbos.life/login                           # 200
curl -sS -o /dev/null -w '%{http_code}\n' https://grafana.arbos.life/metrics                         # 404
curl -sS -o /dev/null -w '%{http_code}\n' https://grafana.arbos.life/api/dashboards/uid/arbos-fleet  # 401
```

Log in as `admin`: Arbos fleet is the home page. Log in as the viewer in a private window: the
Subnet view is the home page and there is nothing else. Then hand the viewer
`https://grafana.arbos.life` and their login over a private channel.

Undo: `rm /etc/arbos/sites.d/grafana.caddy && systemctl reload arbos-www.service`

### 9. Turn on the relay's metrics — **changes production: restarts the relay**

Not part of this stack, and not needed for steps 1–8: it ships with the relay release that adds
`/metrics`. It is listed here because it is the one step that interrupts users. The relay reads
`ARBOS_RELAY_METRICS_LISTEN=127.0.0.1:9108` only at start, so it takes a relay restart, and a
relay restart drops every open WebSocket (devices and boxes reconnect; chats show "Reconnecting…").
Do it **in the same restart** that deploys that relay release, not as a restart of its own: add the
variable through `scripts/deploy/relay-server.sh`'s key lists (it rebuilds `/etc/arbos/relay.env`)
before that deploy. Afterwards, with no change here:

```bash
curl -s '127.0.0.1:9090/api/v1/query?query=up{job="relay"}' | grep -o '"value":[^}]*'      # ends in "1"
curl -s 127.0.0.1:9108/metrics/targets | python3 -c 'import json,sys; print(len(json.load(sys.stdin)), "boxes listed")'
```

The relay panels fill within a minute. Agent panels fill box by box as boxes move to an image whose
agentd serves `/metrics`; until then "On old image" on the Agents dashboard counts the rest.

Undo: remove the variable and restart the relay again (a second interruption), or leave it: the
listener is loopback-only.

## Rolling back everything

Each step's undo above stands alone. To remove the whole stack, in this order:

```bash
rm /etc/arbos/sites.d/grafana.caddy && systemctl reload arbos-www.service
cd /etc/arbos-monitoring && docker compose --env-file /etc/arbos/monitoring.env down -v --rmi all
rm -r /etc/arbos-monitoring /etc/arbos/monitoring.env
```

Then delete the `grafana.arbos.life` DNS record and the Discord webhook. To stop one noisy piece
without the rest: `docker compose --env-file /etc/arbos/monitoring.env stop cadvisor` (the
container panels go empty, everything else keeps working).
