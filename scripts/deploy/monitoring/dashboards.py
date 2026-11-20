#!/usr/bin/env python3
"""Writes the Grafana dashboards under grafana/. The JSON files are what Grafana loads and are
committed; edit a query here, run `python3 dashboards.py`, commit both. `--check` exits 1 when the
committed JSON is not what this file produces (check.sh runs it)."""
import json, pathlib, sys

HERE = pathlib.Path(__file__).parent
FS = '{mountpoint=~"/|/var/lib/docker"}'  # the root filesystem and Docker's data; not /boot, not EFI
BOX = 'name=~"arbos-box-.+"'
RELAY = 'id="/system.slice/arbos-relay.service"'
GREEN, ORANGE, RED = "green", "orange", "red"
UPDOWN = [{"type": "value", "options": {"0": {"text": "DOWN", "color": RED}, "1": {"text": "UP", "color": GREEN}}}]


def steps(*pairs):
    """steps((None, GREEN), (80, ORANGE), (90, RED)) → Grafana threshold steps."""
    return [{"value": v, "color": c} for v, c in pairs]


OK = steps((None, GREEN))
ANY_IS_BAD = steps((None, GREEN), (1, RED))
PCT_HIGH_BAD = steps((None, GREEN), (75, ORANGE), (90, RED))


def targets(ds, exprs, instant=False):
    return [
        {"refId": chr(65 + i), "datasource": ds, "expr": e, "legendFormat": legend,
         "instant": instant, "range": not instant, **({"format": "table"} if instant and legend == "" else {})}
        for i, (e, legend) in enumerate(exprs)
    ]


def stat(title, expr, unit="short", thresholds=OK, mappings=(), decimals=None, w=3, desc=""):
    return {"type": "stat", "title": title, "description": desc, "w": w, "h": 4, "exprs": [(expr, "")],
            "fieldConfig": {"defaults": {"unit": unit, "decimals": decimals, "mappings": list(mappings),
                                         "thresholds": {"mode": "absolute", "steps": thresholds},
                                         "noValue": "–"}, "overrides": []},
            "options": {"reduceOptions": {"calcs": ["lastNotNull"]}, "colorMode": "background",
                        "graphMode": "none", "textMode": "value"}}


def ts(title, exprs, unit="short", w=12, stack=False, desc="", max_=None):
    return {"type": "timeseries", "title": title, "description": desc, "w": w, "h": 8, "exprs": exprs,
            "fieldConfig": {"defaults": {"unit": unit, "min": 0, "max": max_, "custom": {
                "lineWidth": 1, "fillOpacity": 25 if stack else 5, "showPoints": "never", "spanNulls": False,
                "stacking": {"mode": "normal" if stack else "none"}}}, "overrides": []},
            "options": {"legend": {"displayMode": "table" if len(exprs) == 1 else "list", "placement": "bottom",
                                   "calcs": ["lastNotNull", "max"] if len(exprs) == 1 else []},
                        "tooltip": {"mode": "multi", "sort": "desc"}}}


def table(title, exprs, columns, w=24, desc=""):
    """One row per series, the instant queries joined on `name`; `columns` maps refId → (header, unit)."""
    overrides = [{"matcher": {"id": "byName", "options": f"Value #{ref}"}, "properties": [
        {"id": "displayName", "value": head}, {"id": "unit", "value": unit},
        {"id": "custom.cellOptions", "value": {"type": "color-background" if unit == "percentunit" else "auto"}}]}
        for ref, (head, unit) in columns.items()]
    return {"type": "table", "title": title, "description": desc, "w": w, "h": 9, "exprs": exprs, "instant": True,
            "fieldConfig": {"defaults": {"thresholds": {"mode": "absolute", "steps": steps((None, "transparent"), (0.8, ORANGE), (0.95, RED))}},
                            "overrides": overrides},
            "transformations": [{"id": "joinByField", "options": {"byField": "name", "mode": "outer"}},
                                {"id": "organize", "options": {"excludeByName": {"Time": True, **{f"Time {i}": True for i in range(1, 9)}}}}],
            "options": {"sortBy": [{"displayName": next(iter(columns.values()))[0], "desc": True}], "cellHeight": "sm"}}


def row(title):
    return {"type": "row", "title": title, "w": 24, "h": 1, "collapsed": False}


def dashboard(uid, title, panels, ds="prometheus", variables=(), nav=True, refresh="30s", span="now-3h"):
    ds = {"type": "prometheus", "uid": ds}
    out, x, y, line_h = [], 0, 0, 0
    for i, p in enumerate(panels, 1):
        p = dict(p)
        w, h = p.pop("w"), p.pop("h")
        if p.pop("under", False):  # below the previous panel, in its column
            top = out[-1]["gridPos"]["y"] + out[-1]["gridPos"]["h"]
            p.update(id=i, gridPos={"x": x - w, "y": top, "w": w, "h": h})
            x, line_h = x - w, max(line_h, top + h - y)
        else:
            if x + w > 24:
                x, y, line_h = 0, y + line_h, 0
            p.update(id=i, gridPos={"x": x, "y": y, "w": w, "h": h})
        if p["type"] != "row":
            p.update(datasource=ds, targets=targets(ds, p.pop("exprs"), p.pop("instant", False)))
        out.append(p)
        x, line_h = x + w, max(line_h, h)
    return {"uid": uid, "title": title, "tags": ["arbos"], "editable": False, "schemaVersion": 39,
            "timezone": "browser", "refresh": refresh, "time": {"from": span, "to": "now"},
            "graphTooltip": 1, "templating": {"list": list(variables)}, "panels": out,
            "links": [{"type": "dashboards", "tags": ["arbos"], "title": "Arbos", "asDropdown": False,
                       "includeVars": False, "keepTime": True}] if nav else []}


# A trend needs history: with under an hour of samples (240 at 15 s) the stat says so (-1).
DAYS_TO_FULL = (f"(clamp_max(min(node_filesystem_avail_bytes{FS} / "
                f"clamp_min(-deriv(node_filesystem_avail_bytes{FS}[6h]), 1)) / 86400, 999) "
                f"and on() min(count_over_time(node_filesystem_avail_bytes{FS}[6h])) >= 240) or on() vector(-1)")
CPU_USED = '100 * sum(rate(node_cpu_seconds_total{mode!="idle"}[5m])) / count(node_cpu_seconds_total{mode="idle"})'
MEM_AVAIL = "100 * node_memory_MemAvailable_bytes / node_memory_MemTotal_bytes"
TARGETS_DOWN = 'count(up{job!="arbos-agentd"} == 0 and on (job, instance) max_over_time(up[1d]) == 1) or vector(0)'
RELAY_RSS = f'max(process_resident_memory_bytes{{job="relay"}}) or max(container_memory_rss{{{RELAY}}})'
SERVICE = 'id=~"/system.slice/.+"'

def quantile(q, metric="arbos_agentd_turn_duration_seconds"):
    return f"histogram_quantile({q}, sum by (le) (rate({metric}_bucket[5m])))"


# A counter with labels does not exist until its first event, so a healthy system has no "failed"
# series. `x or R0` reads 0 while the relay reports (A0: while any agentd reports) and stays empty
# when there is no data at all.
R0 = "0 * max(arbos_relay_uptime_seconds)"
A0 = "0 * count(arbos_agentd_uptime_seconds)"
ONOFF = [{"type": "value", "options": {"0": {"text": "OFF", "color": GREEN}, "1": {"text": "ON", "color": RED}}}]
UP = dict(mappings=UPDOWN, thresholds=steps((None, RED), (1, GREEN)))
PCT_ERRORS = steps((None, GREEN), (2, ORANGE), (10, RED))
SLOTS_USED = '100 * sum(arbos_relay_boxes_registered{provider=~"gvisor|boxd|docker"}) / max(arbos_relay_boxes_max)'
SPEND_PCT = "100 * max(arbos_relay_spend_today_usd) / max(arbos_relay_spend_daily_cap_usd > 0)"
# `> 0` on the denominator: with no call in the window the result is absent ("–"), not 0/0 = NaN.
MODEL_ERR_PCT = f'100 * (sum(rate(arbos_relay_model_requests_total{{class!="2xx"}}[5m])) or {R0}) / (sum(rate(arbos_relay_model_requests_total[5m])) > 0)'
MODEL = "arbos_relay_model_response_seconds"

fleet = dashboard("arbos-fleet", "Arbos fleet", [
    # service
    stat("Relay", 'min(probe_success{job="relay-health"})', **UP, desc="The relay's /healthz, asked on the host."),
    stat("Public HTTPS", 'min(probe_success{job="public-https"})', **UP,
         desc="relay.arbos.life/healthz and arbos.life through DNS, Caddy and TLS."),
    stat("Kill switch", "max(arbos_relay_kill_switch)", mappings=ONOFF, thresholds=ANY_IS_BAD,
         desc="On: every model call is refused and no agent can work. Trips when today's spend reaches the daily cap."),
    stat("Spend of cap", SPEND_PCT, "percent", steps((None, GREEN), (60, ORANGE), (85, RED)), decimals=0,
         desc="Relay-wide spend since 00:00 UTC against the cap that trips the kill switch."),
    stat("Model errors", MODEL_ERR_PCT, "percent", PCT_ERRORS, decimals=1, desc="Upstream model calls that did not answer 2xx."),
    stat("Model p95", quantile(0.95, MODEL), "s", steps((None, GREEN), (15, ORANGE), (30, RED)), decimals=1,
         desc="Time until the provider starts answering."),
    stat("Relay restarts (24h)", f"changes(container_start_time_seconds{{{RELAY}}}[24h])", thresholds=ANY_IS_BAD),
    stat("Targets down", TARGETS_DOWN, thresholds=ANY_IS_BAD, desc="Targets that answered in the last day and do not now. Boxes are on the Agents dashboard."),
    # boxes
    stat("Boxes running", f"count(container_last_seen{{{BOX}}}) or vector(0)", desc="arbos-box-* containers Docker is running."),
    stat("Box slots used", SLOTS_USED, "percent", steps((None, GREEN), (80, ORANGE), (90, RED)), decimals=0,
         desc="Admitted boxes against max_boxes. At 100% new sign-ups go on the waitlist."),
    stat("Boxes asleep", f'sum(arbos_relay_boxes{{state="asleep"}}) or {R0}'),
    stat("Waitlisted", "sum(arbos_relay_waitlist_length)", thresholds=ANY_IS_BAD),
    stat("Start fails (1h)", f'sum(increase(arbos_relay_box_starts_total{{result="failed"}}[1h])) or {R0}', thresholds=ANY_IS_BAD, decimals=0,
         desc="Wake jobs whose box did not start."),
    stat("Wake queue", "sum(arbos_relay_wake_queue_depth)", thresholds=steps((None, GREEN), (5, ORANGE), (20, RED)),
         desc="Wake jobs queued or in flight."),
    stat("Boxes working", "sum(arbos_relay_boxes_turn_running)", desc="Boxes with a turn running in the last 3 minutes."),
    stat("Containers", 'count(container_last_seen{name!=""})', desc="Every Docker container cAdvisor sees. A drop to none means Docker or cAdvisor is down."),
    # host and relay process
    stat("CPU used", CPU_USED, "percent", PCT_HIGH_BAD, decimals=0),
    stat("Memory available", MEM_AVAIL, "percent", steps((None, RED), (10, ORANGE), (20, GREEN)), decimals=0),
    stat("Disk free", f"100 * min(node_filesystem_avail_bytes{FS} / node_filesystem_size_bytes)", "percent",
         steps((None, RED), (15, ORANGE), (25, GREEN)), decimals=0, desc="The fuller of the root filesystem and Docker's data."),
    stat("Days to disk full", DAYS_TO_FULL, "d", steps((None, RED), (2, ORANGE), (14, GREEN)), decimals=0,
         mappings=[{"type": "value", "options": {"999": {"text": "not filling"}, "-1": {"text": "needs 1 h", "color": "#6e7681"}}}],
         desc="Free space divided by how fast it shrank over the last 6 hours."),
    stat("Load per core (1m)", 'node_load1 / scalar(count(node_cpu_seconds_total{mode="idle"}))', "short",
         steps((None, GREEN), (0.7, ORANGE), (1, RED)), decimals=2),
    stat("Relay memory", RELAY_RSS, "bytes", steps((None, GREEN), (32 * 2**30, ORANGE), (40 * 2**30, RED)), decimals=1),
    stat("Relay sockets", f'sum(arbos_relay_sockets) or sum(container_sockets{{{RELAY}}})',
         desc="Open room websockets (devices and boxes); before the relay reports them, all sockets its processes hold."),
    stat("DB wait p95", quantile(0.95, "arbos_relay_db_writer_wait_seconds"), "s", steps((None, GREEN), (0.25, ORANGE), (1, RED)),
         decimals=3, desc="How long a caller waits for the relay's SQLite writer lock."),

    ts("Boxes", [(f"count(container_last_seen{{{BOX}}})", "containers running"), ("sum by (state) (arbos_relay_boxes)", "relay: {{state}}"),
                 ('sum(arbos_relay_boxes_registered{provider=~"gvisor|boxd|docker"})', "admitted"), ("max(arbos_relay_boxes_max)", "max_boxes"),
                 ("sum(arbos_relay_waitlist_length)", "waitlisted"), ("sum(arbos_relay_boxes_evicted)", "evicted")]),
    ts("Box starts per minute", [('60 * sum by (result) (rate(arbos_relay_box_starts_total[5m]))', "{{result}}"),
                                 ("sum(arbos_relay_wake_queue_depth)", "wake queue")]),
    ts("Spend today", [("max(arbos_relay_spend_today_usd)", "spent"), ("max(arbos_relay_spend_daily_cap_usd)", "cap (kill switch)"),
                       ("0.6 * max(arbos_relay_spend_daily_cap_usd)", "60%"), ("0.85 * max(arbos_relay_spend_daily_cap_usd)", "85%")], "currencyUSD"),
    ts("Box wake time", [(quantile(0.5, "arbos_relay_box_wake_duration_seconds"), "p50"), (quantile(0.95, "arbos_relay_box_wake_duration_seconds"), "p95")],
       "s", desc="Wake started until the box connected back: what a waiting user feels."),
    ts("Model calls per minute", [("60 * sum by (provider, class) (rate(arbos_relay_model_requests_total[5m]))", "{{provider}} {{class}}")]),
    ts("Model response time", [(quantile(0.5, MODEL), "p50"), (quantile(0.95, MODEL), "p95")], "s"),
    ts("Host CPU", [('100 * sum by (mode) (rate(node_cpu_seconds_total{mode!="idle"}[5m])) / scalar(count(node_cpu_seconds_total{mode="idle"}))', "{{mode}}")],
       "percent", stack=True, max_=100),
    ts("Host memory", [("node_memory_MemTotal_bytes - node_memory_MemAvailable_bytes", "used"), ("node_memory_MemAvailable_bytes", "available"),
                       ("node_memory_SwapTotal_bytes - node_memory_SwapFree_bytes", "swap used")], "bytes"),
    ts("Disk free", [(f"node_filesystem_avail_bytes{FS}", "{{mountpoint}}")], "bytes"),
    ts("Load", [("node_load1", "1m"), ("node_load5", "5m"), ("node_load15", "15m"), ('count(node_cpu_seconds_total{mode="idle"})', "cores")]),
    ts("Relay memory", [(f'process_resident_memory_bytes{{job="relay"}}', "process RSS"), (f"container_memory_rss{{{RELAY}}}", "service RSS"),
                        (f"container_memory_working_set_bytes{{{RELAY}}}", "service working set")], "bytes"),
    ts("Relay CPU and sockets", [(f"rate(container_cpu_usage_seconds_total{{{RELAY}}}[5m])", "CPU cores"), ("sum by (kind) (arbos_relay_sockets)", "{{kind}} websockets"),
                                 ("sum(arbos_relay_rooms_open)", "rooms open"), (f"container_sockets{{{RELAY}}}", "sockets (all)")]),
    ts("Relay database", [("sum by (file) (arbos_relay_db_bytes)", "{{file}}")], "bytes",
       desc="Size on disk of relay.db and its write-ahead log. The log sits near 4 MB; one that stays far above 64 MB means a read is held open across many commits, and writes get slower."),
    ts("Relay HTTP requests per second", [("sum by (class) (rate(arbos_relay_http_requests_total[5m]))", "{{class}}")], "reqps"),
    ts("Service restarts per hour", [(f"changes(container_start_time_seconds{{{SERVICE}}}[1h])", "{{id}}")],
       desc="How often each systemd service's cgroup was re-created."),
    ts("Network", [('8 * sum(rate(node_network_receive_bytes_total[5m]))', "in"), ('8 * sum(rate(node_network_transmit_bytes_total[5m]))', "out")], "bps"),
    ts("Probes and certificates", [("probe_duration_seconds", "{{instance}} (s)"), ("(probe_ssl_earliest_cert_expiry - time()) / 86400", "{{instance}} certificate days left")],
       desc="How long each health probe took, and how many days each certificate has left."),
])


MEM_OF_LIMIT = f"container_memory_working_set_bytes{{{BOX}}} / (container_spec_memory_limit_bytes{{{BOX}}} > 0)"
PIDS_OF_LIMIT = f"container_threads{{{BOX}}} / (container_threads_max{{{BOX}}} > 0)"
SEL = 'name=~"$box"'
TOP = "topk($top, {})"

boxes = dashboard("arbos-boxes", "Boxes", [
    stat("Boxes running", f"count(container_last_seen{{{BOX}}}) or vector(0)"),
    stat("Box CPU cores", f"sum(rate(container_cpu_usage_seconds_total{{{BOX}}}[5m]))", decimals=1),
    stat("Box memory", f"sum(container_memory_working_set_bytes{{{BOX}}})", "bytes", decimals=1),
    stat("Over 80% memory", f"count({MEM_OF_LIMIT} > 0.8) or vector(0)", thresholds=steps((None, GREEN), (1, ORANGE))),
    stat("Over 80% pids", f"count({PIDS_OF_LIMIT} > 0.8) or vector(0)", thresholds=steps((None, GREEN), (1, ORANGE)),
         desc="Host threads of the sandbox against its pids limit (see README: under gVisor this is not the process count inside the box)."),
    stat("OOM kills (24h)", "increase(node_vmstat_oom_kill[24h])", thresholds=ANY_IS_BAD, decimals=0, desc="Every OOM kill on the host."),
    stat("Box restarts (1h)", f"sum(changes(container_start_time_seconds{{{BOX}}}[1h])) or vector(0)", thresholds=steps((None, GREEN), (1, ORANGE))),
    stat("Other containers", f'count(container_last_seen{{name!="", name!~"arbos-box-.+"}})'),

    ts("Top boxes by CPU (cores)", [(TOP.format(f"rate(container_cpu_usage_seconds_total{{{BOX}}}[5m])"), "{{name}}")]),
    ts("Top boxes by memory", [(TOP.format(f"container_memory_working_set_bytes{{{BOX}}}"), "{{name}}")], "bytes"),
    ts("Top boxes by pids", [(TOP.format(f"container_threads{{{BOX}}}"), "{{name}}")]),
    ts("Top boxes by disk IO", [(TOP.format(f"sum by (name) (rate(container_fs_reads_bytes_total{{{BOX}}}[5m]) + rate(container_fs_writes_bytes_total{{{BOX}}}[5m]))"), "{{name}}")], "Bps"),
    ts("Top boxes by network", [(TOP.format(f"8 * sum by (name) (rate(container_network_receive_bytes_total{{{BOX}}}[5m]) + rate(container_network_transmit_bytes_total{{{BOX}}}[5m]))"), "{{name}}")], "bps"),
    ts("OOM kills and restarts", [("increase(node_vmstat_oom_kill[10m])", "OOM kills on the host (10m)"),
                                  (f"sum by (name) (increase(container_oom_events_total{{{BOX}}}[10m])) > 0", "OOM {{name}}"),
                                  (f"changes(container_start_time_seconds{{{BOX}}}[10m]) > 0", "restart {{name}}")]),
    table("Boxes against their limits", [(f"max by (name) ({q})", "") for q in (
        MEM_OF_LIMIT, PIDS_OF_LIMIT, f"container_memory_working_set_bytes{{{BOX}}}",
        f"rate(container_cpu_usage_seconds_total{{{BOX}}}[5m])", f"time() - container_start_time_seconds{{{BOX}}}")],
          {"A": ("Memory of limit", "percentunit"), "B": ("Pids of limit", "percentunit"), "C": ("Memory", "bytes"),
           "D": ("CPU cores", "short"), "E": ("Up for", "s")}),

    row("Selected boxes ($box)"),
    ts("CPU (cores)", [(f"rate(container_cpu_usage_seconds_total{{{SEL}}}[5m])", "{{name}}"),
                       (f"rate(container_cpu_cfs_throttled_seconds_total{{{SEL}}}[5m])", "throttled {{name}}")], w=8),
    ts("Memory", [(f"container_memory_working_set_bytes{{{SEL}}}", "{{name}}"), (f"container_spec_memory_limit_bytes{{{SEL}}} > 0", "limit {{name}}")], "bytes", w=8),
    ts("Pids", [(f"container_threads{{{SEL}}}", "{{name}}"), (f"container_threads_max{{{SEL}}} > 0", "limit {{name}}")], w=8),
    ts("Disk IO", [(f"sum by (name) (rate(container_fs_reads_bytes_total{{{SEL}}}[5m]))", "read {{name}}"),
                   (f"sum by (name) (rate(container_fs_writes_bytes_total{{{SEL}}}[5m]))", "write {{name}}")], "Bps", w=8),
    ts("Network", [(f"8 * sum by (name) (rate(container_network_receive_bytes_total{{{SEL}}}[5m]))", "in {{name}}"),
                   (f"8 * sum by (name) (rate(container_network_transmit_bytes_total{{{SEL}}}[5m]))", "out {{name}}")], "bps", w=8),
    ts("Other containers by memory", [(TOP.format('container_memory_working_set_bytes{name!="", name!~"arbos-box-.+"}'), "{{name}}")], "bytes", w=8),
], variables=[
    {"name": "box", "label": "Box", "type": "query", "datasource": {"type": "prometheus", "uid": "prometheus"},
     "query": {"query": f"label_values(container_last_seen{{{BOX}}}, name)", "refId": "box"}, "refresh": 2, "sort": 1,
     "multi": True, "includeAll": True, "allValue": "arbos-box-.+", "current": {"text": "All", "value": "$__all"}},
    {"name": "top", "label": "Top", "type": "custom", "query": "5,10,20", "current": {"text": "10", "value": "10"},
     "options": [{"text": v, "value": v, "selected": v == "10"} for v in ("5", "10", "20")]},
])


FAILED_PCT = f'100 * (sum(rate(arbos_agentd_turns_finished_total{{result="error"}}[5m])) or {A0}) / (sum(rate(arbos_agentd_turns_finished_total[5m])) > 0)'
BOX_MODEL_ERR_PCT = f'100 * (sum(rate(arbos_agentd_model_requests_total{{class!="2xx"}}[5m])) or {A0}) / (sum(rate(arbos_agentd_model_requests_total[5m])) > 0)'

agents = dashboard("arbos-agents", "Agents", [
    stat("Boxes reporting", 'sum(arbos_relay_box_scrapes{result="ok"})',
         desc="Awake boxes whose agentd answers the scrape. Every number on this page covers these boxes only."),
    stat("On old image", 'sum(arbos_relay_box_scrapes{result="old_image"})', thresholds=steps((None, GREEN), (1, ORANGE)),
         desc="Awake boxes on an image from before agentd had /metrics. They work; their turns are not counted here until they are rolled."),
    stat("Unreachable", 'sum(arbos_relay_box_scrapes{result=~"unreachable|rejected"})', thresholds=ANY_IS_BAD,
         desc="Awake boxes whose agentd did not answer the relay (timeout, refused, 401 or 5xx), or whose answer the relay rejected."),
    stat("Turns started / min", "60 * sum(rate(arbos_agentd_turns_started_total[5m]))", decimals=1),
    stat("Turns failed", FAILED_PCT, "percent", PCT_ERRORS, decimals=1),
    stat("Turn duration p95", quantile(0.95), "s", decimals=1),
    stat("Waiting", "sum(arbos_agentd_sends_waiting)", thresholds=steps((None, GREEN), (20, ORANGE), (100, RED)),
         desc="User messages waiting for a turn, over all boxes."),
    stat("Crashes (1h)", f"sum(increase(arbos_agentd_worker_crashes_total[1h])) or {A0}", thresholds=ANY_IS_BAD, decimals=0,
         desc="Main agents respawned and job workers replaced."),

    ts("Turns per minute", [("60 * sum(rate(arbos_agentd_turns_started_total[5m]))", "started"),
                            ("60 * sum by (result) (rate(arbos_agentd_turns_finished_total[5m]))", "finished {{result}}")]),
    ts("Turn duration", [(quantile(0.5), "p50"), (quantile(0.95), "p95"), (quantile(0.99), "p99")], "s"),
    ts("Queue and running work", [("sum(arbos_agentd_sends_waiting)", "messages waiting"), ("sum(arbos_agentd_turns_running)", "turns running"),
                                  ("sum(arbos_agentd_workers_running)", "worker turns running"), ("sum by (state) (arbos_agentd_jobs)", "jobs {{state}}")]),
    ts("Crashes per hour", [(f"sum(increase(arbos_agentd_worker_crashes_total[1h])) or {A0}", "all"),
                            ("sum by (kind) (increase(arbos_agentd_worker_crashes_total[1h]))", "{{kind}}")]),
    ts("Model calls per minute", [("60 * sum by (provider, class) (rate(arbos_relay_model_requests_total[5m]))", "{{provider}} {{class}}")],
       desc="Upstream calls as the relay made them."),
    ts("Model response time", [(quantile(0.5, MODEL), "p50"), (quantile(0.95, MODEL), "p95")], "s"),
    ts("Failure rates", [(FAILED_PCT, "turns failed"), (MODEL_ERR_PCT, "model errors (relay)"), (BOX_MODEL_ERR_PCT, "model errors (as boxes saw them)")], "percent"),
    ts("Box health", [("sum by (result) (arbos_relay_box_scrapes)", "scrape {{result}}"),
                      (f'sum(arbos_agentd_workspaces{{health="unhealthy"}}) or {A0}', "unhealthy workspaces"),
                      ("sum(arbos_agentd_rooms_open) - sum(arbos_agentd_rooms_connected)", "rooms not connected"),
                      ("sum(increase(arbos_agentd_relay_reconnects_total[1h]))", "relay reconnects (1h)")],
       desc="Awake boxes by how their last metrics scrape ended; a sleeping box is not counted."),
])


# Swarm: every box at once. The relay's per-box series (`/metrics/boxes`, job relay-boxes) carry an
# opaque `box` id; `arbos_relay_box_info` maps it to the container name, which is the `name` cAdvisor
# uses, so both sources meet in one row per box. Admin organisation only: `name` holds a user id.
#
# A relay from before `/metrics/boxes` has none of those series. The dashboard then still plots
# every running box from cAdvisor (state unknown, no tokens), the counts come from the relay's
# fleet gauges, and each token figure says when it arrives instead of "No data".
AFTER_DEPLOY = "after the next relay update"
WAITING = [{"type": "value", "options": {"-1": {"text": AFTER_DEPLOY, "color": "#6e7681"},
                                         "-2": {"text": "relay just restarted", "color": "#6e7681"}}}]
# Colour is the box's state. Three hues that stay apart for colour-blind readers on the dark panel
# (checked pairwise), and greys for the states in which a box is not doing anything.
STATES = [(0, "unknown", "#8b949e"), (1, "asleep", "#57606a"), (2, "awake, idle", "#3987e5"),
          (3, "waking", "#c9d1d9"), (4, "working", "#c98500"), (5, "error", "#d55181")]
STATE_NAMES = [{"type": "value", "options": {str(n): {"text": text, "color": color} for n, text, color in STATES}}]
STATE_STEPS = steps(*[(n or None, color) for n, _, color in STATES])
INFO = 'label_replace(arbos_relay_box_info, "name", "$1", "container", "(.+)")'
# Tokens first: a box that used tokens in the last minute, the minute the swarm's height is measured
# over, is working, whatever the relay believes about it. The relay's view can be stale (it has
# just restarted and has not heard from the box, or lost its heartbeat) and its turn flag drops
# before the turn's tokens leave the rate; a box high in the swarm must not be coloured idle.
# Otherwise the relay's state, with awake split by its turn flag.
USED_TOKENS = "(sum by (box) (rate(arbos_relay_box_tokens_total[1m])) > bool 0)"
STATE_OF_BOX = (f'max by (box) (4 * ({USED_TOKENS} == 1) or on (box) (0 * arbos_relay_box_state{{state="unknown"}}'
                ' or 1 * arbos_relay_box_state{state="asleep"} or 3 * arbos_relay_box_state{state="waking"}'
                ' or 5 * arbos_relay_box_state{state="error"}'
                ' or (2 * arbos_relay_box_state{state="awake"} + on (box) 2 * arbos_relay_box_turn_running)))')
RUNNING0 = f"max by (name) (0 * container_last_seen{{{BOX}}})"  # 0 for every running box
KNOWN0 = f"max by (name) (0 * {INFO})"  # 0 for every box the relay lists, asleep ones included


def named(per_box, info=INFO):
    """A per-box relay expression, relabelled from `box` to the container `name`."""
    return f"max by (name) (({per_box}) * on (box) group_left (name) {info})"


def box_rate(metric, window="1m", by="box"):
    return f"sum by ({by}) (rate({metric}[{window}]))"


def pending(expr):
    return f"max({expr}) or vector(-1)"


def token_tile(title, expr, desc, metric, decimals=0):
    """-2 while `metric` is there but has no rate yet (the relay just restarted), -1 while it is not there at all."""
    return stat(title, pending(f"{expr} or (0 * count({metric}) - 2)"), "short", OK, WAITING, decimals, desc=desc)


def per_box(title, fields, w, desc):
    """A panel with one row per box: instant queries joined on `name`. `fields` is a list of
    (column name, unit, query)."""
    renames = {f"Value #{chr(65 + i)}": head for i, (head, _, _) in enumerate(fields)}
    return {"title": title, "description": desc, "w": w, "instant": True, "exprs": [(q, "") for _, _, q in fields],
            "transformations": [{"id": "joinByField", "options": {"byField": "name", "mode": "outer"}},
                                {"id": "organize", "options": {"renameByName": {**renames, "name": "Box"},
                                                               "excludeByName": {"Time": True, **{
                                                                   f"{label} {i}": True for i in range(1, 12) for label in ("Time", "__name__", "job", "instance")}}}}],
            "fieldConfig": {"defaults": {}, "overrides": [
                {"matcher": {"id": "byName", "options": head}, "properties": [{"id": "unit", "value": unit}]} for head, unit, _ in fields]}}


def override(name, **props):
    return {"matcher": {"id": "byName", "options": name}, "properties": [{"id": k, "value": v} for k, v in props.items()]}


def swarm_chart(title, fields, x, y, size, w, h, desc):
    """One bubble per box: position from two measures, area from a third, colour from its state."""
    p = per_box(title, fields, w, desc)
    by = lambda name: {"matcher": {"id": "byName", "options": name}}
    # pluginVersion: without one Grafana takes the options for the pre-11.1 format and rewrites them
    p.update(type="xychart", pluginVersion="13.2.6", h=h, options={
        "mapping": "manual", "series": [{"frame": {"matcher": {"id": "byIndex", "options": 0}}, "x": by(x), "y": by(y),
                                         "size": by(size), "color": by("State"), "name": {"fixed": "box"}}],
        "legend": {"showLegend": False}, "tooltip": {"mode": "single", "sort": "none"}})
    p["fieldConfig"]["defaults"] = {"min": 0, "custom": {
        "show": "points", "pointSize": {"fixed": 8, "min": 6, "max": 44}, "pointShape": "circle", "pointStrokeWidth": 1,
        "fillOpacity": 55, "axisPlacement": "auto", "scaleDistribution": {"type": "linear"}}}
    p["fieldConfig"]["overrides"] += [
        override("State", mappings=STATE_NAMES, thresholds={"mode": "absolute", "steps": STATE_STEPS}, color={"mode": "thresholds"}),
        override(x, **{"custom.axisLabel": x}), override(y, **{"custom.axisLabel": y})]
    return p


def box_table(title, fields, desc):
    p = per_box(title, fields, 24, desc)
    p.update(type="table", h=10, options={"cellHeight": "sm", "sortBy": [{"displayName": fields[0][0], "desc": True}]})
    p["fieldConfig"]["defaults"] = {"decimals": 1}
    p["fieldConfig"]["overrides"] += [override("State", mappings=STATE_NAMES, **{"custom.cellOptions": {"type": "color-text"}}),
                                      override("Pids", decimals=0), override(fields[0][0], decimals=0)]
    return p


def by_state(title, w, h, desc):
    """How many boxes are in each state, in the swarm's colours: the count and the colour key in one."""
    working, awake = "sum(arbos_relay_boxes_turn_running)", 'sum(arbos_relay_boxes{state="awake"})'
    rows = [("working", working), ("awake, idle", f"clamp_min({awake} - {working}, 0)"),
            *[(name, f'sum(arbos_relay_boxes{{state="{name}"}})') for name in ("waking", "asleep", "error")]]
    colors = {text: color for _, text, color in STATES}
    return {"type": "bargauge", "title": title, "description": desc, "w": w, "h": h, "exprs": [(q, name) for name, q in rows], "instant": True,
            "fieldConfig": {"defaults": {"unit": "short", "min": 0, "noValue": "–", "color": {"mode": "fixed"}},
                            "overrides": [override(name, color={"mode": "fixed", "fixedColor": colors[name]}) for name, _ in rows]},
            "options": {"orientation": "horizontal", "displayMode": "basic", "valueMode": "color", "showUnfilled": True,
                        "namePlacement": "left", "minVizHeight": 16, "reduceOptions": {"calcs": ["lastNotNull"]}}}


def later(panel):
    """A token panel on a relay that does not count tokens yet says when it will."""
    panel["fieldConfig"]["defaults"]["noValue"] = AFTER_DEPLOY
    return panel


BOX_TOKENS, FLEET_TOKENS = "arbos_relay_box_tokens_total", "arbos_relay_tokens_total"
CPU = f"max by (name) (rate(container_cpu_usage_seconds_total{{{BOX}}}[2m]))"
MEMORY = f"max by (name) (container_memory_working_set_bytes{{{BOX}}})"
NETWORK = f"8 * sum by (name) (rate(container_network_receive_bytes_total{{{BOX}}}[2m]) + rate(container_network_transmit_bytes_total{{{BOX}}}[2m]))"
TOKENS_MIN = named(f"60 * {box_rate(BOX_TOKENS)}")
STATE = named(STATE_OF_BOX)
# In the swarm a sleeping box sits at the origin (no container, so no CPU or memory from cAdvisor),
# and a box the relay does not describe yet has state 0 and no tokens.
POINT = [("CPU cores", "short", f"{CPU} or {KNOWN0}"), ("Memory", "bytes", f"{MEMORY} or {KNOWN0}"),
         ("State", "short", f"{STATE} or {RUNNING0}")]
SEL_INFO = INFO.replace("arbos_relay_box_info", 'arbos_relay_box_info{container=~"$box"}')

swarm = dashboard("arbos-swarm", "Swarm", [
    stat("Working", "sum(arbos_relay_boxes_turn_running)", desc="Boxes with an agent turn running in the last 3 minutes."),
    stat("Running", f"count(container_last_seen{{{BOX}}}) or vector(0)", desc="arbos-box-* containers Docker is running."),
    token_tile("Tokens / s", f"sum(rate({BOX_TOKENS}[20s]))",
               "Tokens the boxes' model calls used, per second over the last 20 s. A call is counted when it ends, so this moves in steps.", BOX_TOKENS),
    token_tile("Tokens / min", f"sum(increase({BOX_TOKENS}[1m]))", "Tokens used in the last minute, all four kinds together: input, output, cache read, cache write.", BOX_TOKENS),
    token_tile("Tokens / 24 h", f"sum(increase({FLEET_TOKENS}[24h]))",
               "Every token the proxy metered in the last 24 hours, or since the relay started counting if that is more recent.", FLEET_TOKENS),
    stat("Calls / min", f"60 * sum(rate(arbos_relay_model_requests_total[1m])) or {R0}", decimals=0, desc="Upstream model calls the relay made."),
    stat("Spend today", "max(arbos_relay_spend_today_usd)", "currencyUSD", decimals=0, desc="Relay-wide spend since 00:00 UTC."),
    stat("Spend of cap", SPEND_PCT, "percent", steps((None, GREEN), (60, ORANGE), (85, RED)), decimals=0,
         desc="Today's spend against the daily cap that trips the kill switch and stops every agent."),

    swarm_chart("Swarm", [*POINT, ("Tokens / min", "short", f"{TOKENS_MIN} or {RUNNING0}")], "CPU cores", "Tokens / min", "Memory", 17, 14,
                "Every box is one bubble. Up: tokens per minute its agents are using. Right: CPU cores it is using. Area: memory. "
                "Colour: state (key on the right). A busy fleet rises off the floor; an idle one sits along it; sleeping boxes are at the origin. "
                "Until the relay reports per box, every bubble is on the floor and grey: tokens and state arrive with the next relay deploy."),
    by_state("Boxes by state", 7, 6, "As the relay counts them. Working: a turn ran in the last 3 minutes. The colours are the swarm's."),
    {**swarm_chart("Network against CPU", [*POINT, ("Network", "bps", f"{NETWORK} or {KNOWN0}")], "CPU cores", "Network", "Memory", 7, 8,
                   "The same boxes by what they cost the host: network throughput against CPU, area = memory. Needs nothing from the relay."),
     "under": True},

    later(ts("Tokens per minute by kind", [(f"60 * sum by (kind) (rate({FLEET_TOKENS}[1m]))", "{{kind}}")], stack=True,
             desc="Every metered model call. Cache reads are tokens served from the provider's prompt cache.")),
    later(ts("Busiest boxes, tokens per minute", [(f"topk($top, {TOKENS_MIN})", "{{name}}")],
             desc="The $top boxes using the most tokens right now.")),

    box_table("Boxes", [("Tokens / min", "short", TOKENS_MIN), ("State", "short", f"{STATE} or {RUNNING0}"),
                        ("Calls / min", "short", named(f"60 * {box_rate('arbos_relay_box_model_calls_total')}")),
                        ("Spend / h", "currencyUSD", named(f"3600 * {box_rate('arbos_relay_box_spend_usd_total', '5m')}")),
                        ("CPU cores", "short", CPU), ("Memory", "bytes", MEMORY), ("Pids", "short", f"max by (name) (container_threads{{{BOX}}})"),
                        ("Network", "bps", NETWORK)],
              "Every box, busiest first. The token, call and spend columns arrive with the next relay deploy; a sleeping box has no container figures."),

    row("Selected boxes ($box)"),
    later(ts("Tokens per minute by kind", [(f"60 * sum by (kind) ({box_rate(BOX_TOKENS, by='box, kind')} * on (box) group_left () {SEL_INFO})", "{{kind}}")],
             stack=True, w=8)),
    later(ts("Model calls per minute", [(named(f"60 * {box_rate('arbos_relay_box_model_calls_total')}", SEL_INFO), "{{name}}")], w=8)),
    later(ts("Spend per hour", [(named(f"3600 * {box_rate('arbos_relay_box_spend_usd_total', '5m')}", SEL_INFO), "{{name}}")], "currencyUSD", w=8,
             desc="Metered cost of the selected boxes' model calls.")),
    ts("CPU (cores)", [(f'rate(container_cpu_usage_seconds_total{{{SEL}}}[2m])', "{{name}}")]),
    ts("Memory", [(f"container_memory_working_set_bytes{{{SEL}}}", "{{name}}")], "bytes"),
    later({"type": "state-timeline", "title": "State", "w": 24, "h": 14,
           "description": "What each selected box was doing, one row per box, in the swarm's colours. With every box selected it reads as the fleet's history.",
           "exprs": [(named(STATE_OF_BOX, SEL_INFO), "{{name}}")],
           "fieldConfig": {"defaults": {"mappings": STATE_NAMES, "thresholds": {"mode": "absolute", "steps": STATE_STEPS},
                                        "color": {"mode": "thresholds"}, "custom": {"fillOpacity": 80, "lineWidth": 0}}, "overrides": []},
           "options": {"showValue": "never", "mergeValues": True, "rowHeight": 0.85, "legend": {"showLegend": False},
                       "tooltip": {"mode": "single"}}}),
], variables=[
    {"name": "box", "label": "Box", "type": "query", "datasource": {"type": "prometheus", "uid": "prometheus"},
     "query": {"query": f"label_values(container_last_seen{{{BOX}}}, name)", "refId": "box"}, "refresh": 2, "sort": 1,
     "multi": True, "includeAll": True, "allValue": "arbos-box-.+", "current": {"text": "All", "value": "$__all"}},
    {"name": "top", "label": "Top", "type": "custom", "query": "5,10,20", "current": {"text": "10", "value": "10"},
     "options": [{"text": v, "value": v, "selected": v == "10"} for v in ("5", "10", "20")]},
], refresh="5s", span="now-30m")


# Arbos: the front page, one screen, for the team and for the partner alike. It is written twice
# from one definition: `arbos.json` for the main organisation and `subnet.json` for the Subnet one.
# The team's reads exact figures (`arbos:*`), the partner's the same figures made coarse
# (`subnet:*`, see prometheus/subnet-rules.yml). They differ in the centre: the team sees the swarm, one dot per box with
# its name on hover and a link to the Swarm dashboard; the partner sees the same fleet as counts
# of machines by token rate, because a dot per machine can be followed by its values whatever it
# is labelled with. The team also sees tokens by kind; the partner sees their total.
#
# Calm on purpose: numbers in the text colour, colour only for a crossed threshold or a box's state.
# A figure whose source is not reporting says so in grey (-1), never a blank or a NaN; a percentage
# with nothing to divide by (no model call in five minutes) reads 100: nothing failed.
QUIET_GREY = "#6e7681"
FEW = {"-5": {"text": "<5"}}  # a count of one to four machines, as the partner's rules pass it on
PENDING = [{"type": "value", "options": {"-1": {"text": "no data yet", "color": QUIET_GREY}, **FEW}}]
NEUTRAL = steps((None, "text"))
RELAY_REPORTS = "0 * subnet:box_start_failures:rate5m"  # present exactly while the relay reports
# A relay that counts tokens but started under a minute ago has no rate yet: that is -2, "relay just
# restarted", and not -1, which is a relay from before it counted them at all.
JUST_STARTED = "(0 * subnet:relay_counts_tokens - 2)"
MODEL_OK = f"100 * (1 - subnet:model_error_ratio) or 100 + {RELAY_REPORTS}"
BLUE, AMBER, MAGENTA, AQUA = "#3987e5", "#c98500", "#d55181", "#199e70"


def headline(title, expr, unit="short", thresholds=NEUTRAL, mappings=PENDING, decimals=0, desc=""):
    p = stat(title, pending(expr), unit, thresholds, mappings, decimals, w=4, desc=desc)
    p.update(h=3, transparent=True)
    p["options"].update(colorMode="value", justifyMode="center")
    return p


def bars(title, rows, unit, h, max_expr=None, thresholds=None, desc=""):
    """A short list of labelled bars. `rows` is (name, query, colour); with `thresholds` the colour is the value's."""
    p = {"type": "bargauge", "title": title, "description": desc, "w": 8, "h": h, "instant": True, "transparent": True,
         "exprs": [(q, name) for name, q, _ in rows],
         "fieldConfig": {"defaults": {"unit": unit, "min": 0, "decimals": 0, "noValue": "–", "mappings": [{"type": "value", "options": FEW}],
                                      "color": {"mode": "thresholds" if thresholds else "fixed"},
                                      "thresholds": {"mode": "absolute", "steps": thresholds or NEUTRAL}},
                         "overrides": [] if thresholds else [override(name, color={"mode": "fixed", "fixedColor": c}) for name, _, c in rows]},
         "options": {"orientation": "horizontal", "displayMode": "basic", "valueMode": "color", "showUnfilled": True,
                     "namePlacement": "left", "minVizHeight": 12, "maxVizHeight": 22, "text": {"titleSize": 12, "valueSize": 16},
                     "reduceOptions": {"calcs": ["lastNotNull"]}}}
    if max_expr:  # every bar against the same ceiling, read from a query
        p["exprs"].append((max_expr, "max"))
        p["transformations"] = [{"id": "configFromData", "options": {"configRefId": chr(65 + len(rows)), "mappings": [
            {"fieldName": "max", "handlerKey": "max"}, {"fieldName": "Time", "handlerKey": "__ignore"}]}}]
    else:
        p["fieldConfig"]["defaults"]["max"] = 100
    return p


def trend(title, lines, unit="short", stack=False, desc="", max_=None):
    """A time series in fixed colours. `lines` is (query, legend, colour)."""
    p = ts(title, [(q, name) for q, name, _ in lines], unit, w=8, stack=stack, desc=desc, max_=max_)
    p.update(h=6, transparent=True)
    p["fieldConfig"]["overrides"] = [override(name, color={"mode": "fixed", "fixedColor": c}) for _, name, c in lines]
    p["options"]["legend"] = {"displayMode": "list", "placement": "bottom", "calcs": []}
    return later(p)


def overview(partner):
    # The team reads its own exact figures; the partner reads the coarse ones made from them.
    ns = "subnet" if partner else "arbos"
    colors = {text: color for _, text, color in STATES}
    if partner:
        # No machine leaves the team's Prometheus as a series of its own, so the partner's centre
        # piece is the same fleet as counts: how many machines use how many tokens a minute.
        hero = bars("Machines by tokens per minute", [(name, f'{ns}:boxes_by_token_rate{{bucket="{name}"}}', color) for name, color in (
            ("idle", colors["awake, idle"]), ("under 10K", "#7a5a12"), ("10K to 100K", "#a06d0a"), ("100K to 1M", AMBER), ("over 1M", "#e59a0c"))],
            "short", 10, desc="Agent machines by how many tokens a minute their agents are using, averaged over five minutes. "
                              "Counts are rounded to the nearest five; one to four machines show as \"<5\".")
        hero.update(w=16)
        hero["options"].update(orientation="vertical", showUnfilled=False, text={"titleSize": 13, "valueSize": 28}, maxVizHeight=300,
                               minVizWidth=40, maxVizWidth=220)
        hero["fieldConfig"]["defaults"].update(min=0)
        del hero["fieldConfig"]["defaults"]["max"]
    else:
        hero = swarm_chart("Swarm", [*POINT, ("Tokens / min", "short", f"{TOKENS_MIN} or {RUNNING0}")], "CPU cores", "Tokens / min", "Memory", 16, 10,
                           "Every box is one dot. Up: tokens per minute its agents are using. Right: CPU cores. Area: memory. Colour: what it is doing, "
                           "as in the list on the right. A busy fleet rises off the floor; an idle one sits along it; sleeping boxes are at the origin. "
                           "Hover for the box; click to open it. Until the relay reports per box, every dot is grey and on the floor.")
        hero["fieldConfig"]["defaults"]["links"] = [{"title": "Open this box", "url": "/d/arbos-swarm/swarm?var-box=${__data.fields.Box}"}]
        hero.update(transparent=True)
    hero = later(hero)
    return dashboard("arbos-subnet" if partner else "arbos", "Arbos", [
        headline("Service", "subnet:relay_up * subnet:public_up", thresholds=steps((None, MAGENTA), (1, AQUA)),
                 mappings=[{"type": "value", "options": {"0": {"text": "Down"}, "1": {"text": "Up"}, "-1": {"text": "no data yet", "color": QUIET_GREY}}}],
                 desc="The relay answers, on the server and from outside through its public name."),
        headline("Working now", f"{ns}:boxes_working", desc="Agent machines with a turn running in the last 3 minutes."
                 + (" Rounded to the nearest five." if partner else "")),
        headline("Tokens / min", f"60 * {ns}:tokens:rate5m or {JUST_STARTED}", mappings=WAITING,
                 desc="Tokens the agents' model calls use: input, output and cached, averaged over five minutes."
                 + (" Two significant figures; one while fewer than five machines are using any." if partner else "")),
        headline("Tokens / 24 h", f"{ns}:tokens:increase24h or {JUST_STARTED}", mappings=WAITING),
        headline("Model calls OK", MODEL_OK, "percent", steps((None, MAGENTA), (90, AMBER), (98, "text")), decimals=1,
                 desc="Model calls answered successfully in the last five minutes."),
        headline("Spend of cap", "100 * subnet:spend_cap_ratio", "percent", steps((None, "text"), (60, AMBER), (85, MAGENTA)),
                 desc="Today's model spend against the daily cap. At 100% every agent stops until 00:00 UTC."),

        hero,
        bars("Machines", [("working", f"{ns}:boxes_working", colors["working"]), ("awake, idle", f"{ns}:boxes_idle", colors["awake, idle"]),
                          ("waking", f"{ns}:boxes_waking", colors["waking"]), ("asleep", f"{ns}:boxes_asleep", colors["asleep"]),
                          ("error", f"{ns}:boxes_error", colors["error"]), ("waiting for one", f"{ns}:boxes_waitlisted", MAGENTA)],
             "short", 6, max_expr="subnet:boxes_max", desc="Agent machines by what they are doing, each bar against the limit of machines this host admits."
             + (" Rounded to the nearest five; one to four show as \"<5\"." if partner else "")),
        {**bars("Host", [("CPU", "100 * subnet:host_cpu_used_ratio", None), ("memory", "100 * subnet:host_memory_used_ratio", None),
                         ("disk", "100 * (1 - subnet:host_disk_free_ratio)", None)], "percent", 4,
                thresholds=steps((None, QUIET_GREY), (75, AMBER), (90, MAGENTA)), desc="How much of the server is in use."), "under": True},

        # By kind for the team only: with list prices the split would price the partner's view in dollars.
        trend("Tokens per minute", [(f"60 * {ns}:tokens:rate5m", "tokens", BLUE)]) if partner else
        trend("Tokens per minute", [(f"60 * sum(rate({FLEET_TOKENS}{{kind=\"{k}\"}}[5m]))", name, c) for k, name, c in (
            ("cache_read", "cache read", BLUE), ("cache_write", "cache write", AQUA), ("input", "input", AMBER), ("output", "output", MAGENTA))], stack=True),
        trend("Model calls per minute", [(f"60 * {ns}:model_requests:rate5m", "calls", BLUE),
                                         (f"60 * {ns}:model_requests:rate5m * subnet:model_error_ratio", "failed", MAGENTA)]),
        trend("Machine starts per minute", [("60 * subnet:box_starts:rate5m", "started", BLUE), ("60 * subnet:box_start_failures:rate5m", "failed", MAGENTA)]),

        trend("Spend of cap", [("100 * subnet:spend_cap_ratio", "today", BLUE)], "percent", max_=100,
              desc="Climbs through the day and returns to 0 at 00:00 UTC."),
        trend("Host in use", [("100 * subnet:host_cpu_used_ratio", "CPU", BLUE), ("100 * subnet:host_memory_used_ratio", "memory", AQUA),
                              ("100 * (1 - subnet:host_disk_free_ratio)", "disk", AMBER)], "percent", max_=100),
        trend("Machines", [(f"{ns}:boxes_running", "running", BLUE), (f"{ns}:boxes_working", "working", AMBER),
                           (f"{ns}:boxes_asleep", "asleep", QUIET_GREY), ("subnet:boxes_max", "limit", "#c9d1d9")]),
    ], ds="prometheus-subnet" if partner else "prometheus", nav=not partner, refresh="10s", span="now-3h")


FILES = {"grafana/dashboards/arbos.json": overview(partner=False), "grafana/dashboards/fleet.json": fleet,
         "grafana/dashboards/boxes.json": boxes, "grafana/dashboards/swarm.json": swarm,
         "grafana/dashboards/agents.json": agents, "grafana/dashboards-subnet/subnet.json": overview(partner=True)}

if __name__ == "__main__":
    stale = []
    for path, dash in FILES.items():
        text = json.dumps(dash, indent=1) + "\n"
        if "--check" in sys.argv:
            if (HERE / path).read_text() != text:
                stale.append(path)
        else:
            (HERE / path).write_text(text)
    if stale:
        sys.exit(f"out of date, run python3 dashboards.py: {', '.join(stale)}")
