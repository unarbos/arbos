#!/usr/bin/env python3
"""Writes the Grafana dashboards under grafana/. The JSON files are what Grafana loads and are
committed; edit a query here, run `python3 dashboards.py`, commit both. `--check` exits 1 when the
committed JSON is not what this file produces (check.sh runs it)."""
import json, pathlib, sys

HERE = pathlib.Path(__file__).parent
FS = '{fstype=~"ext4|xfs|btrfs"}'
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


def dashboard(uid, title, panels, ds="prometheus", variables=(), nav=True):
    ds = {"type": "prometheus", "uid": ds}
    out, x, y, line_h = [], 0, 0, 0
    for i, p in enumerate(panels, 1):
        p = dict(p)
        w, h = p.pop("w"), p.pop("h")
        if x + w > 24:
            x, y, line_h = 0, y + line_h, 0
        p.update(id=i, gridPos={"x": x, "y": y, "w": w, "h": h})
        if p["type"] != "row":
            p.update(datasource=ds, targets=targets(ds, p.pop("exprs"), p.pop("instant", False)))
        out.append(p)
        x, line_h = x + w, max(line_h, h)
    return {"uid": uid, "title": title, "tags": ["arbos"], "editable": False, "schemaVersion": 39,
            "timezone": "browser", "refresh": "30s", "time": {"from": "now-3h", "to": "now"},
            "graphTooltip": 1, "templating": {"list": list(variables)}, "panels": out,
            "links": [{"type": "dashboards", "tags": ["arbos"], "title": "Arbos", "asDropdown": False,
                       "includeVars": False, "keepTime": True}] if nav else []}


DAYS_TO_FULL = (f"clamp_max(min(node_filesystem_avail_bytes{FS} / "
                f"clamp_min(-deriv(node_filesystem_avail_bytes{FS}[6h]), 1)) / 86400, 999)")
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
MODEL_ERR_PCT = f'100 * (sum(rate(arbos_relay_model_requests_total{{class!="2xx"}}[5m])) or {R0}) / sum(rate(arbos_relay_model_requests_total[5m]))'
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
         steps((None, RED), (15, ORANGE), (25, GREEN)), decimals=0, desc="The fullest filesystem."),
    stat("Days to disk full", DAYS_TO_FULL, "d", steps((None, RED), (2, ORANGE), (14, GREEN)), decimals=0,
         mappings=[{"type": "value", "options": {"999": {"text": "not filling"}}}],
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


FAILED_PCT = f'100 * (sum(rate(arbos_agentd_turns_finished_total{{result="error"}}[5m])) or {A0}) / sum(rate(arbos_agentd_turns_finished_total[5m]))'
BOX_MODEL_ERR_PCT = f'100 * (sum(rate(arbos_agentd_model_requests_total{{class!="2xx"}}[5m])) or {A0}) / sum(rate(arbos_agentd_model_requests_total[5m]))'

agents = dashboard("arbos-agents", "Agents", [
    stat("Boxes reporting", 'sum(arbos_relay_box_scrapes{result="ok"})',
         desc="Awake boxes whose agentd answers the scrape. Every number on this page covers these boxes only."),
    stat("On old image", 'sum(arbos_relay_box_scrapes{result="old_image"})', thresholds=steps((None, GREEN), (1, ORANGE)),
         desc="Awake boxes on an image from before agentd had /metrics. They work; their turns are not counted here until they are rolled."),
    stat("Unreachable", 'sum(arbos_relay_box_scrapes{result="unreachable"})', thresholds=ANY_IS_BAD,
         desc="Awake boxes whose agentd did not answer the relay: timeout, refused, 401 or 5xx."),
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


# The Subnet view lives in its own Grafana organisation whose only datasource holds `subnet:*`.
# 100% while agents report and no turn has finished in the window, rather than a blank headline.
SUCCESS = "100 * (1 - subnet:turns_failed:rate5m / (subnet:turns_finished:rate5m > 0)) or 100 + 0 * subnet:turns_failed:rate5m"
subnet = dashboard("arbos-subnet", "Subnet view", [
    stat("Service", "subnet:relay_up", mappings=UPDOWN, thresholds=steps((None, RED), (1, GREEN)), w=4),
    stat("Reachable from outside", "subnet:public_up", mappings=UPDOWN, thresholds=steps((None, RED), (1, GREEN)), w=4),
    stat("Agent machines running", "subnet:boxes_running", w=4),
    stat("Turns succeeding", SUCCESS, "percent", steps((None, RED), (90, ORANGE), (98, GREEN)), decimals=1, w=4),
    stat("Turn duration p95", "subnet:turn_duration_seconds:p95", "s", decimals=1, w=4),
    stat("Waiting for a machine", "subnet:boxes_waitlisted", thresholds=ANY_IS_BAD, w=4),

    ts("Agent turns per minute", [("60 * subnet:turns_started:rate5m", "started"), ("60 * subnet:turns_finished:rate5m", "finished"),
                                  ("60 * subnet:turns_failed:rate5m", "failed")]),
    ts("Turn duration", [("subnet:turn_duration_seconds:p50", "p50"), ("subnet:turn_duration_seconds:p95", "p95")], "s"),
    ts("Agent machines", [("subnet:boxes_running", "running"), ("subnet:boxes_asleep", "asleep"), ("subnet:boxes_waitlisted", "waiting"),
                          ("subnet:boxes_max", "limit"), ("subnet:boxes_reporting", "reporting turn numbers")],
       desc="Turn numbers come only from the machines that report them; the rest are being upgraded."),
    ts("Work in progress", [("subnet:turns_active", "turns running"), ("subnet:queue_depth", "queued")]),
    ts("Success rates", [(SUCCESS, "turns"), ("100 * (1 - subnet:model_error_ratio)", "model calls"),
                         ("100 * (1 - subnet:box_start_failures:rate5m / (subnet:box_starts:rate5m > 0))", "machine starts")], "percent", max_=100),
    ts("Host headroom", [("100 * subnet:host_cpu_used_ratio", "CPU used"), ("100 * subnet:host_memory_used_ratio", "memory used"),
                         ("100 * (1 - subnet:host_disk_free_ratio)", "disk used")], "percent", max_=100),
], ds="prometheus-subnet", nav=False)

FILES = {"grafana/dashboards/fleet.json": fleet, "grafana/dashboards/boxes.json": boxes,
         "grafana/dashboards/agents.json": agents, "grafana/dashboards-subnet/subnet.json": subnet}

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
