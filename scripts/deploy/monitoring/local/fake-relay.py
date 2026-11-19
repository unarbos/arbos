#!/usr/bin/env python3
"""Local stand-in for the relay (docker-compose.local.yml). Not used in production.

  127.0.0.1:7700  /healthz
  127.0.0.1:9108  /metrics            relay series, names from docs/metrics.md
                  /metrics/targets    http_sd: three awake "boxes"
                  /metrics/box/<id>   agentd series of one box

Counters are computed from the clock, so rates and percentiles have something to show. For its first
five minutes the fake is a healthy system: like the real relay it has no "failed", "error" or "5xx"
series until the first such event, so a panel that goes blank without them shows up here. A fourth
box is listed but answers 502, as a box on an image from before agentd had /metrics does. /metrics also
carries stand-ins for what cAdvisor reports about the relay's systemd service on the real host,
which a laptop has no equivalent of.
"""
import json, os, threading, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

START = time.time()
# FAKE_BOXES=0: the relay as it is before box metrics ship: its own series only, no
# arbos_relay_box_scrapes, no box discovery (404). The Subnet view has to be complete like that.
BOXES = ["0a1b2c3d4e5f", "1b2c3d4e5f60", "2c3d4e5f6071"] if os.environ.get("FAKE_BOXES", "1") != "0" else []
OLD_IMAGE_BOX = "3d4e5f607182"


def failing(lines):
    """Failure series appear with the first failure: none in the first five minutes."""
    return lines if time.time() - START > 300 else []

RELAY_BUCKETS = (0.001, 0.005, 0.025, 0.1, 0.25, 0.5, 1, 2.5, 5, 10, 30, 60, 120, 300)
TURN_BUCKETS = (1, 2.5, 5, 10, 30, 60, 120, 300, 600, 1800)


def histogram(name, buckets, count, typical, labels=""):
    """A histogram whose observations sit around `typical` seconds, with a thin slow tail."""
    lines, sep = [], "," if labels else ""
    for le in buckets:
        share = 0.0 if le < typical / 4 else 0.5 if le < typical else 0.9 if le < typical * 4 else 0.99
        lines.append(f'{name}_bucket{{{labels}{sep}le="{le}"}} {int(count * share)}')
    lines += [f'{name}_bucket{{{labels}{sep}le="+Inf"}} {int(count)}', f"{name}_count{{{labels}}} {int(count)}",
              f"{name}_sum{{{labels}}} {count * typical}"]
    return lines


def relay_metrics():
    t = int(time.time()) - 1_700_000_000
    wave = t // 10 % 12
    svc = 'id="/system.slice/arbos-relay.service"'
    return [
        f"process_resident_memory_bytes {22_000_000_000 + wave * 50_000_000}",
        f"process_open_fds {900 + wave * 7}",
        f"process_start_time_seconds {START}",
        f"process_cpu_seconds_total {t * 3}",
        'arbos_relay_build_info{version="0.6.422"} 1',
        f"arbos_relay_uptime_seconds {time.time() - START}",
        f"arbos_relay_rooms_open {300 + wave * 4}",
        f'arbos_relay_sockets{{kind="client"}} {240 + wave * 5}',
        'arbos_relay_sockets{kind="box"} 180',
        f'arbos_relay_boxes{{state="awake"}} {58 + wave // 4}',
        f'arbos_relay_boxes{{state="waking"}} {wave // 6}',
        f'arbos_relay_boxes{{state="asleep"}} {14 - wave // 4}',
        'arbos_relay_boxes_registered{provider="gvisor"} 72',
        "arbos_relay_boxes_max 80",
        f"arbos_relay_boxes_turn_running {20 + wave}",
        "arbos_relay_boxes_evicted 1",
        *([f'arbos_relay_box_scrapes{{result="ok"}} {len(BOXES)}', 'arbos_relay_box_scrapes{result="old_image"} 1',
           'arbos_relay_box_scrapes{result="unreachable"} 0', 'arbos_relay_box_scrapes{result="rejected"} 0',
           'arbos_relay_box_scrapes{result="pending"} 0', "arbos_relay_box_scrape_dropped_lines_total 0"] if BOXES else []),
        "arbos_relay_boxes_upgrading 0",
        "arbos_relay_boxes_creating 0",
        "arbos_relay_waitlist_length 0",
        f"arbos_relay_wake_queue_depth {wave // 5}",
        f'arbos_relay_box_starts_total{{result="ok"}} {t // 30}',
        f'arbos_relay_box_starts_total{{result="fetching"}} {t // 400}',
        *failing([f'arbos_relay_box_starts_total{{result="failed"}} {t // 3000}']),
        *histogram("arbos_relay_box_start_duration_seconds", RELAY_BUCKETS, t // 30, 2.5),
        *histogram("arbos_relay_box_wake_duration_seconds", RELAY_BUCKETS, t // 30, 10),
        f'arbos_relay_model_requests_total{{provider="openrouter",class="2xx"}} {t * 6}',
        *failing([f'arbos_relay_model_requests_total{{provider="openrouter",class="5xx"}} {t // 9}',
                  f'arbos_relay_model_requests_total{{provider="anthropic",class="unreachable"}} {t // 40}']),
        f'arbos_relay_model_requests_total{{provider="anthropic",class="2xx"}} {t * 2}',
        *histogram("arbos_relay_model_response_seconds", RELAY_BUCKETS, t * 8, 2.5),
        f"arbos_relay_spend_today_usd {int(time.time()) % 86400 / 86400 * 310:.2f}",
        "arbos_relay_spend_daily_cap_usd 500",
        "arbos_relay_kill_switch 0",
        f'arbos_relay_http_requests_total{{route="/chats/{{id}}/messages",class="2xx"}} {t * 40}',
        f'arbos_relay_http_requests_total{{route="/proxy/v1/{{*tail}}",class="2xx"}} {t * 8}',
        f'arbos_relay_http_requests_total{{route="unmatched",class="4xx"}} {t * 2}',
        f'arbos_relay_http_requests_total{{route="/proxy/v1/{{*tail}}",class="5xx"}} {t // 20}',
        *histogram("arbos_relay_db_writer_wait_seconds", RELAY_BUCKETS, t // 10, 0.005),
        f'arbos_relay_db_bytes{{file="db"}} {9_000_000_000 + t % 1000 * 4096}',
        f'arbos_relay_db_bytes{{file="wal"}} {4_000_000 + wave * 300_000}',
        f"container_memory_rss{{{svc}}} {22_500_000_000 + wave * 50_000_000}",
        f"container_memory_working_set_bytes{{{svc}}} {23_000_000_000 + wave * 50_000_000}",
        f"container_cpu_usage_seconds_total{{{svc}}} {t * 3}",
        f"container_sockets{{{svc}}} {1100 + wave * 9}",
        f"container_start_time_seconds{{{svc}}} {START}",
        f'container_start_time_seconds{{id="/system.slice/caddy.service"}} {START - 86400}',
    ]


def box_metrics(n):
    t = int(time.time()) - 1_700_000_000
    wave = t // 10 % 12
    return [
        'arbos_agentd_build_info{version="0.6.422"} 1',
        f"arbos_agentd_uptime_seconds {time.time() - START}",
        f'arbos_agentd_workspaces{{health="healthy"}} {n + 1}',
        f"arbos_agentd_turns_running {n + wave // 5}",
        f"arbos_agentd_sends_waiting {wave // (4 - n)}",
        f"arbos_agentd_workers_running {n + 2}",
        f'arbos_agentd_jobs{{state="running"}} {n}',
        f'arbos_agentd_jobs{{state="queued"}} {wave // 6}',
        f"arbos_agentd_rooms_open {4 + n}",
        f"arbos_agentd_rooms_connected {4 + n}",
        f"arbos_agentd_turns_started_total {t * (n + 1) // 4}",
        f'arbos_agentd_turns_finished_total{{result="done"}} {t * (n + 1) // 4 - t * (n + 1) // 90 - n}',
        *failing([f'arbos_agentd_turns_finished_total{{result="error"}} {t * (n + 1) // 100}',
                  f'arbos_agentd_turns_finished_total{{result="cancelled"}} {t * (n + 1) // 900}']),
        *histogram("arbos_agentd_turn_duration_seconds", TURN_BUCKETS, t * (n + 1) // 4, 30),
        *failing([f'arbos_agentd_worker_crashes_total{{kind="main"}} {t * (n + 1) // 5000}',
                  f'arbos_agentd_worker_crashes_total{{kind="job"}} {t * (n + 1) // 9000}']),
        f'arbos_agentd_model_requests_total{{class="2xx"}} {t * (n + 1)}',
        *failing([f'arbos_agentd_model_requests_total{{class="5xx"}} {t * (n + 1) // 60}']),
        f"arbos_agentd_relay_reconnects_total {t * (n + 1) // 7000}",
    ]


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        path, kind = self.path.split("?")[0], "text/plain; version=0.0.4"
        if path == "/healthz":
            body, kind = '{"ok":true}', "application/json"
        elif path == "/metrics":
            body = "\n".join(relay_metrics()) + "\n"
        elif path == "/metrics/targets" and BOXES:
            body, kind = json.dumps([{"targets": ["127.0.0.1:9108"], "labels": {"__metrics_path__": f"/metrics/box/{b}", "box": b}}
                                     for b in BOXES + [OLD_IMAGE_BOX]]), "application/json"
        elif path.startswith("/metrics/box/") and path.rsplit("/", 1)[1] in BOXES:
            body = "\n".join(box_metrics(BOXES.index(path.rsplit("/", 1)[1]))) + "\n"
        elif path == f"/metrics/box/{OLD_IMAGE_BOX}":
            return self.send_error(502)
        else:
            return self.send_error(404)
        self.send_response(200)
        self.send_header("Content-Type", kind)
        self.end_headers()
        self.wfile.write(body.encode())

    def log_message(self, *args):
        pass


for port in (7700, 9108):
    threading.Thread(target=ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever).start()
