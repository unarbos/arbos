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
| `cadvisor` | `ghcr.io/google/cadvisor:v0.60.6` | 9180 | 1 GB, 2 CPUs | CPU, memory, pids, IO and network per container, and for the relay's systemd service |
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
| `grafana-access.sh` | creates the Subnet organisation, its datasource, dashboard and the one viewer account |
| `Caddyfile.grafana` | the vhost for Grafana |
| `.env.example` | the secrets the stack needs; the filled-in copy is never committed |
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
available), `BoxStartFailures` (failed wake jobs), `Waitlist`, `BoxCapacityHigh` (90 % of
`max_boxes` admitted), `SpendAt60Percent` and `SpendAt85Percent` (of the global daily cap, which
trips the kill switch and stops every agent), `KillSwitchOn`, `ModelErrorsHigh` (> 10 % of calls),
`ModelSlow` (p95 > 30 s to first byte), `BoxOOMKilled`, `RelayRestarted`, `RelayMemoryHigh`
(> 40 GiB; the relay sat at ~22 GB when this was written — tune the number in the rule).

All go to one receiver. Summaries never name a box or a user: they say what is wrong, and the
admin dashboards say where.

**Pointing it at Discord.** In the channel that should get alerts: Server Settings → Integrations →
Webhooks → New Webhook → Copy Webhook URL. Put it in the env file as `ALERT_WEBHOOK_URL=…` and run
`docker compose up -d alertmanager`. The URL is a credential (anyone holding it can post to the
channel); it lives only in the env file. Send a test:

```bash
docker exec arbos-mon-alertmanager amtool alert add Test --annotation=summary='test alert' --alertmanager.url=http://127.0.0.1:9093
```

It arrives within 30 seconds and resolves itself after five minutes.

## Access model

- **Nothing but Grafana is reachable.** Prometheus, Alertmanager and the exporters have no
  authentication of their own, so they bind loopback and are never proxied.
- **Grafana**: anonymous access off, sign-up off, organisation creation off, public dashboards and
  external snapshots off, Secure + SameSite=strict cookies. The admin password comes from
  `/etc/arbos/monitoring.env` (root, mode 0600).
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

## What cAdvisor sees under gVisor

cAdvisor reads the host's cgroups. A `runsc` box is one sandbox in one cgroup, so cAdvisor measures
the box **from outside**: what it costs the host, not what happens inside it.

| metric | under `runsc` |
|---|---|
| CPU (`container_cpu_usage_seconds_total`) | correct for the whole sandbox, the gVisor kernel's own overhead included |
| memory (`container_memory_working_set_bytes`, `…_usage_bytes`) | correct for the whole sandbox. Use these. |
| `container_memory_rss` | **misleading**: gVisor backs all application memory with one memfd, which the host counts as shared memory, not anonymous. RSS shows little more than the sandbox's own overhead. The dashboards do not use it for boxes. |
| pids (`container_threads`, `container_threads_max`) | **host threads of the sandbox**, not processes inside the box. The 512 limit is applied to those threads too, so "pids of limit" is still the number that matters for the limit, but it is not a process count. |
| processes, file descriptors, sockets inside the box | **invisible**. `container_processes` is the handful of `runsc` host processes. |
| OOM kills | the host kernel kills the sandbox (the whole box dies, not one process); there is no OOM killer inside gVisor. cAdvisor reads the kill from the kernel log. The `BoxOOMKilled` alert uses the host-wide counter from node_exporter, because a killed box's own series can vanish before it is scraped twice. |
| network bytes and packets | read from the box's network namespace, where the sandbox uses the real veth; expected to be right. Drops inside gVisor's own network stack are not visible. |
| disk IO (`container_fs_reads/writes_bytes_total`) | what the sandbox's host processes read and wrote; a series exists only once a box has done IO. Not verified against gVisor's file proxy. |
| filesystem usage per box | not collected (and not meaningful: the box's root overlay lives inside the sandbox) |

Not confirmed anywhere public, so the runbook checks them on the host after the first start:
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
to this stack. Until then the relay and agent panels show "–", the two jobs are down or empty, and
nothing alerts on them (`ScrapeTargetDown` ignores a target that has never answered). Relay up,
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
docker compose --env-file local/local.env -f docker-compose.yml -f docker-compose.local.yml up -d
./grafana-access.sh local/local.env
python3 local/check-dashboards.py      # after ~2 minutes of data: every panel's query returns data
open http://127.0.0.1:3300             # admin / the password in local/local.env
```

The local override runs a fake relay (`local/fake-relay.py`: `/healthz`, `/metrics`, the box
discovery and per-box metrics, with the names from `docs/metrics.md` and counters that move) and
three containers named `arbos-box-test_*`. The
config files are the production ones, unchanged. Differences from production: the containers share
one network namespace instead of the host's; the public probes hit the real `relay.arbos.life` and
`arbos.life` (two GETs every 30 s); the relay's systemd-service series are faked, since a Mac has
no such cgroup; and the test boxes run on `runc`, so nothing here exercises gVisor.

`./check.sh` runs the static checks without starting anything. Tear down, data and images included:

```bash
docker compose --env-file local/local.env -f docker-compose.yml -f docker-compose.local.yml down -v --rmi all
```

## Production runbook

On the relay host, as root. Steps 1–2 change nothing. Every later step says what it changes; each
can be undone on its own ([Rolling back](#rolling-back)).

**Cost.** Memory: the caps add up to 4.6 GB; expect about 1.5 GB, most of it Grafana, which grows to its cap (the host has 503 GB). CPU: well
under one core; cAdvisor is capped at two. Disk: ~2.6 GB of images, at most 16 GB of metrics
(expect ~3–5 GB at 15 days with 125 containers), at most 210 MB of logs — under 19 GB worst case,
1.5 % of the disk. Nothing is restarted except, in step 7, a reload of the `:443` Caddy.

### 1. Check the host (read-only)

```bash
ss -ltnH | grep -E ':(3300|9090|9091|9093|9100|9115|9180)\s' || echo "ports free"
ss -ltnH | grep -E ':9108\s' || echo "relay metrics listener not up yet (app panels stay empty)"
docker compose version
df -h /var/lib/docker
ls -l /run/docker.sock /run/containerd/containerd.sock
docker info --format '{{.CgroupDriver}} cgroup v{{.CgroupVersion}}'
curl -fsS -o /dev/null -w '%{http_code}\n' http://127.0.0.1:7700/healthz
curl -fsS -o /dev/null -w '%{http_code}\n' https://relay.arbos.life/healthz
```

Stop if a port is taken (change it in `docker-compose.yml`, `prometheus/prometheus.yml` and the
datasource together). The relay listens on `127.0.0.1:7700`; port 8080 on this host is something
else.
`/run/containerd/containerd.sock` may be absent on an older Docker; cAdvisor then needs only
`docker.sock`.

### 2. Put the files on the host (new files only)

From a checkout of `main` at the merged commit:

```bash
install -d -m 0755 /etc/arbos-monitoring
git archive HEAD scripts/deploy/monitoring | tar -x -C /etc/arbos-monitoring --strip-components=3
/etc/arbos-monitoring/check.sh
```

### 3. Write the env file (new file only)

```bash
install -m 0600 /etc/arbos-monitoring/.env.example /etc/arbos/monitoring.env
openssl rand -hex 24      # run twice: the Grafana admin password and the viewer password
$EDITOR /etc/arbos/monitoring.env
```

Fill in `GF_SECURITY_ADMIN_PASSWORD`, `SUBNET_VIEWER_PASSWORD`, `ALERT_WEBHOOK_URL` (see Alerts)
and `GF_SERVER_ROOT_URL`. That file is the stack's only secret.

### 4. DNS — **changes production (DNS only)**

Add an A record `grafana.arbos.life` → the server's public IP, **DNS-only** (not proxied), like
`relay.arbos.life`: the certificate comes from the TLS-ALPN challenge on 443.

### 5. Start the stack — **changes production: pulls images, starts 7 containers**

```bash
cd /etc/arbos-monitoring && docker compose up -d
docker compose ps
```

All seven are `Up` (cAdvisor `healthy` after 30 s). Then check what could not be checked off the host:

```bash
# every target: 1 = scraped. relay stays 0, and no box is listed, until the relay has its metrics listener.
curl -s '127.0.0.1:9090/api/v1/query?query=up' | python3 -c 'import json,sys; [print(r["value"][1], r["metric"]["job"], r["metric"]["instance"]) for r in json.load(sys.stdin)["data"]["result"]]'
# cAdvisor names every box: these two numbers are equal
curl -s 127.0.0.1:9180/metrics | grep -c '^container_last_seen{.*name="arbos-box-'
docker ps -q --filter name=arbos-box- | wc -l
# gVisor boxes report their 16 GiB limit (1.7179869184e+10), not 0
curl -s 127.0.0.1:9180/metrics | grep '^container_spec_memory_limit_bytes{.*name="arbos-box-' | head -3
```

If the public probes fail from the host but the site works from outside, the host cannot reach its
own public IP; add `127.0.0.1 relay.arbos.life arbos.life` to `/etc/hosts` (the probe then still
goes through Caddy and checks the certificate).

### 6. Subnet organisation and viewer — **changes Grafana only**

```bash
/etc/arbos-monitoring/grafana-access.sh
```

It ends with `viewer '…' can open the Subnet view and nothing else`, having tested exactly that.

### 7. Publish Grafana — **changes production: reloads the :443 Caddy**

```bash
WWW_DIR=/home/const/arbos-www
sed 's#@GRAFANA_HOST@#grafana.arbos.life#g' /etc/arbos-monitoring/Caddyfile.grafana > /etc/arbos/sites.d/grafana.caddy
CLOUDFLARE_DNS_API_TOKEN=validate-only-placeholder "$WWW_DIR/bin/caddy" validate --config "$WWW_DIR/Caddyfile" --adapter caddyfile \
  && systemctl reload arbos-www.service
```

If `validate` fails, nothing was reloaded: delete `/etc/arbos/sites.d/grafana.caddy` and stop.
A reload keeps open connections (the relay's WebSockets included).

### 8. Verify from outside

```bash
curl -sS -o /dev/null -w '%{http_code}\n' https://grafana.arbos.life/login     # 200
curl -sS -o /dev/null -w '%{http_code}\n' https://grafana.arbos.life/metrics   # 404
curl -sS -o /dev/null -w '%{http_code}\n' https://grafana.arbos.life/api/dashboards/uid/arbos-fleet   # 401
```

Log in as `admin`: Arbos fleet is the home page. Log in as the viewer in a private window: the
Subnet view is the home page and there is nothing else. Send the test alert (Alerts). Then hand
the viewer `https://grafana.arbos.life` and their login over a private channel.

## Rolling back

Each of these stands alone; do as many as needed, in this order.

```bash
# unpublish Grafana (undoes step 7): reloads the :443 Caddy
rm /etc/arbos/sites.d/grafana.caddy && systemctl reload arbos-www.service

# stop everything, keep the data (undoes step 5)
cd /etc/arbos-monitoring && docker compose down

# …or stop and delete the data and images too
cd /etc/arbos-monitoring && docker compose down -v --rmi all

# remove the files and secrets (undoes steps 2–3)
rm -r /etc/arbos-monitoring /etc/arbos/monitoring.env
```

Then delete the `grafana.arbos.life` DNS record and the Discord webhook. Nothing in the relay, the
boxes, Docker's configuration or the existing Netdata was touched, so there is nothing else to undo.

To stop one noisy piece without the rest: `docker compose stop cadvisor` (the container panels go
empty, everything else keeps working).
