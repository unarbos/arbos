#!/usr/bin/env python3
"""Against the running local stack: runs every query of every dashboard through Grafana (so the
datasource reference is exercised too) and fails on an error or an empty result, then checks that
nothing the Subnet datasource holds carries an identifying label.

    python3 local/check-dashboards.py        # after docker compose ... up -d and grafana-access.sh
    python3 local/check-dashboards.py --relay-only   # stack started with FAKE_BOXES=0: every panel of the
                                                     # Subnet view has real data from relay metrics alone
"""
import base64, json, pathlib, sys, time, urllib.request

HERE = pathlib.Path(__file__).parent.parent
ENV = dict(line.split("=", 1) for line in (HERE / "local/local.env").read_text().splitlines() if "=" in line and not line.startswith("#"))
GRAFANA = "http://127.0.0.1:3300"
AUTH = "Basic " + base64.b64encode(f"admin:{ENV['GF_SECURITY_ADMIN_PASSWORD']}".encode()).decode()
# Per-series breakdowns of events a healthy system has not had yet. Anything else that comes back
# empty is a blank panel on a healthy system, and fails.
EMPTY_IS_FINE = ("container_oom_events_total", "changes(container_start_time_seconds{name=~", "cfs_throttled",
                 "sum by (kind) (increase(arbos_agentd_worker_crashes_total")


RELAY_ONLY = "--relay-only" in sys.argv


def post(path, body, org):
    req = urllib.request.Request(GRAFANA + path, json.dumps(body).encode(),
                                 {"Authorization": AUTH, "Content-Type": "application/json", "X-Grafana-Org-Id": str(org)})
    return json.load(urllib.request.urlopen(req))


def query(expr, uid, org):
    now = int(time.time() * 1000)
    expr = expr.replace("$box", "arbos-box-.+").replace("$top", "10")
    out = post("/api/ds/query", {"from": str(now - 900_000), "to": str(now), "queries": [
        {"refId": "A", "datasource": {"type": "prometheus", "uid": uid}, "expr": expr, "range": True,
         "intervalMs": 30_000, "maxDataPoints": 100}]}, org)["results"]["A"]
    if out.get("error"):
        return out["error"]
    values = [v for f in out.get("frames", []) if len(f["data"]["values"]) > 1 for v in f["data"]["values"][1]]
    if not values:
        return "no data"
    if any(v is None or v != v for v in values[-3:]):
        return "NaN"
    if RELAY_ONLY and values[-1] == -1:
        return "placeholder (no data yet)"


failed = 0
for path, org in (("grafana/dashboards/fleet.json", 1), ("grafana/dashboards/boxes.json", 1),
                  ("grafana/dashboards/agents.json", 1), ("grafana/dashboards-subnet/subnet.json", 2)):
    dash = json.loads((HERE / path).read_text())
    if RELAY_ONLY:
        if org != 2:
            continue
    for panel in dash["panels"]:
        for t in panel.get("targets", []):
            problem = query(t["expr"], t["datasource"]["uid"], org)
            if problem and not (problem == "no data" and any(s in t["expr"] for s in EMPTY_IS_FINE)):
                failed += 1
                print(f"FAIL {dash['title']} / {panel['title']} [{t['refId']}]: {problem}\n     {t['expr']}")
    print(f"checked {dash['title']}: {sum(len(p.get('targets', [])) for p in dash['panels'])} queries")

# The Subnet datasource, asked for everything it has: only these labels may exist.
now = int(time.time())
series = post("/api/ds/query", {"from": str((now - 300) * 1000), "to": str(now * 1000), "queries": [
    {"refId": "A", "datasource": {"type": "prometheus", "uid": "prometheus-subnet"}, "expr": '{__name__=~".+"}', "instant": True}]}, 2)
labels = {k for f in series["results"]["A"]["frames"] for field in f["schema"]["fields"] for k in (field.get("labels") or {})}
print("labels in the Subnet datasource:", sorted(labels))
if labels - {"__name__", "instance", "job"}:
    failed += 1
    print("FAIL: the Subnet datasource holds labels beyond __name__/instance/job")
sys.exit(1 if failed else 0)
