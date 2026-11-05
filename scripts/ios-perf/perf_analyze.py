#!/usr/bin/env python3
"""Summarise perf-baseline logs from perf-rig.sh.

Input: <tag>-<test>.app.log (log show --style compact) and <tag>-<test>.xcb.log (xcodebuild).
Output: JSON on stdout with, per test and tag:
  intervals   name -> list of ms (in order; the first open-chat after each launch is cold)
  rows_build  list of (path, messages, rows, window, ms)
  phases      phase -> frames, late, dropped, stalls>=100, hangs>=250, over_ms, secs,
              probe hitch ratio (ms/s), mean hz, counter sums
  xct         XCTest "measured [...]" lines: metric -> values
  launch      first-content ms per launch
  rss         max MB seen
"""
import json, re, sys, glob, os, statistics
from datetime import datetime

DECEL = 1.5
TS = re.compile(r'^(\d{4}-\d\d-\d\d \d\d:\d\d:\d\d\.\d+)')


def ts(line):
    m = TS.match(line)
    return datetime.strptime(m.group(1)[:26], "%Y-%m-%d %H:%M:%S.%f").timestamp() if m else None


def kv(s):
    return {k: v for k, v in re.findall(r'(\w+)=([\w.\-]+)', s)}


def runner_gestures(path):
    """`perfgesture end` lines from the runner, as printed into the xcodebuild log (UTC)."""
    out = []
    if not os.path.exists(path):
        return out
    for line in open(path, errors="replace"):
        if "perfgesture end" in line:
            t = ts(line)
            if t is not None:
                out.append(t)
    return out


def parse_app(path):
    out = {"intervals": {}, "rows_build": [], "phases": {}, "launch": [], "rss_max": 0, "launches": 0, "hz": []}
    out["gestures"] = runner_gestures(path[:-len(".app.log")] + ".xcb.log")
    events = []
    open_phase = {}
    for line in open(path, errors="replace"):
        t = ts(line)
        if t is None:
            continue
        if "perfphase " in line:
            m = re.search(r'perfphase (\S+) (begin|end)', line)
            if m:
                name, edge = m.groups()
                if edge == "begin":
                    open_phase[name] = t
                elif name in open_phase:
                    out["phases"].setdefault(name, []).append([open_phase.pop(name), t])
            continue
        if "[com.unarbos.arbos.ios:launch]" in line:
            m = re.search(r':launch\] (\S+) (\d+) ms', line)
            if m:
                out.setdefault("milestones", {}).setdefault(m.group(1), []).append(int(m.group(2)))
            continue
        if ":perf-frames]" not in line:
            continue
        body = line.split(":perf-frames] ", 1)[1].strip()
        if body.startswith("probe on"):
            out["launches"] += 1
            m = re.search(r't=(\d+)ms', body)
            if m:
                out.setdefault("milestones", {}).setdefault("root-onAppear", []).append(int(m.group(1)))
            events.append((t, "launch", {}))
        elif body.startswith("interval "):
            name = body.split()[1]
            ms = float(kv(body)["ms"])
            events.append((t, "interval", {"name": name, "ms": ms}))
        elif body.startswith("rows-build"):
            d = kv(body)
            out["rows_build"].append([d["path"], int(d["messages"]), int(d["rows"]), int(d["window"]), float(d["ms"]), t])
        elif body.startswith("late "):
            d = kv(body)
            events.append((t, "late", {"ms": float(d["ms"]), "missed": int(d["missed"])}))
        elif body.startswith("second "):
            d = kv(body)
            rss = int(d["rss"].rstrip("MB")) if "rss" in d else 0
            out["rss_max"] = max(out["rss_max"], rss)
            events.append((t, "second", d))
    # intervals, tagging the first open-chat after each launch as cold
    seen_open = True
    for t, kind, d in events:
        if kind == "launch":
            seen_open = False
        elif kind == "interval":
            name = d["name"]
            if name == "open-chat":
                name = "open-chat (first after launch)" if not seen_open else "open-chat (repeat)"
                seen_open = True
            out["intervals"].setdefault(name, []).append(d["ms"])
    # phases
    summary = {}
    for name, spans in out["phases"].items():
        acc = {"secs": 0.0, "late": 0, "dropped": 0, "stall100": 0, "hang250": 0, "over_ms": 0.0, "frames": 0,
               "hz": [], "counts": {}}
        for a, b in spans:
            acc["secs"] += b - a
            hz = 60.0
            for t, kind, d in events:
                if kind == "second" and a <= t <= b + 1.0:
                    hz = float(d.get("hz", 60)) or 60.0
                    acc["hz"].append(hz)
                    acc["frames"] += int(d.get("frames", 0))
                    for k in ("publishes", "home_body", "chat_body", "row_body", "block_body", "tokens"):
                        if k in d:
                            acc["counts"][k] = acc["counts"].get(k, 0) + int(d[k])
                if kind == "late" and a <= t <= b:
                    nominal = 1000.0 / hz
                    acc["late"] += 1
                    acc["dropped"] += max(0, d["missed"])
                    acc["over_ms"] += d["ms"] - nominal
                    acc["stall100"] += d["ms"] >= 100
                    acc["hang250"] += d["ms"] >= 250
        acc["hitch_ms_per_s"] = round(acc["over_ms"] / acc["secs"], 2) if acc["secs"] else None
        # Deceleration windows: DECEL s after each gesture's touches end, no XCUITest query inside.
        gs = [g for g in out.get("gestures", []) if any(a <= g <= b for a, b in spans)]
        if gs:
            dec = {"windows": len(gs), "secs": round(len(gs) * DECEL, 2), "late": 0, "dropped": 0, "over_ms": 0.0, "stall100": 0}
            for g in gs:
                for t, kind, d in events:
                    if kind == "late" and g < t <= g + DECEL:
                        dec["late"] += 1
                        dec["dropped"] += max(0, d["missed"])
                        dec["over_ms"] += d["ms"] - 1000.0 / (acc["hz"][-1] if acc["hz"] else 60.0)
                        dec["stall100"] += d["ms"] >= 100
            dec["over_ms"] = round(dec["over_ms"], 1)
            dec["hitch_ms_per_s"] = round(dec["over_ms"] / dec["secs"], 2)
            acc["decel"] = dec
        acc["hz"] = round(statistics.mean(acc["hz"]), 1) if acc["hz"] else None
        acc["spans"] = len(spans)
        acc["secs"] = round(acc["secs"], 2)
        acc["over_ms"] = round(acc["over_ms"], 1)
        summary[name] = acc
    out["phases"] = summary
    return out


MEAS = re.compile(r"measured \[(.+?)\] average: ([\d.]+), relative standard deviation: ([\d.]+)%, values: \[(.*?)\]")


def parse_xcb(path):
    res, status = {}, None
    for line in open(path, errors="replace"):
        m = MEAS.search(line)
        if m:
            vals = [float(v) for v in m.group(4).split(",") if v.strip()]
            res[m.group(1)] = {"avg": float(m.group(2)), "rsd_pct": float(m.group(3)), "values": vals}
        m = re.search(r"Test Case .*?\]' (passed|failed|skipped)", line)
        if m:
            status = m.group(1)
    return res, status


def stats(v):
    if not v:
        return None
    s = sorted(v)
    p = lambda q: s[min(len(s) - 1, int(round(q * (len(s) - 1))))]
    return {"n": len(s), "median": round(statistics.median(s), 1), "p90": round(p(0.9), 1),
            "min": round(s[0], 1), "max": round(s[-1], 1),
            "iqr": round(p(0.75) - p(0.25), 1)}


def main(d):
    out = {}
    for app in sorted(glob.glob(os.path.join(d, "*.app.log"))):
        base = os.path.basename(app)[:-len(".app.log")]
        tag, test = base.split("-", 1)
        a = parse_app(app)
        x, status = parse_xcb(os.path.join(d, base + ".xcb.log")) if os.path.exists(os.path.join(d, base + ".xcb.log")) else ({}, None)
        a["interval_stats"] = {k: stats(v) for k, v in a["intervals"].items()}
        a["milestone_stats"] = {k: stats(v) for k, v in a.get("milestones", {}).items()}
        a["xct"] = x
        a["status"] = status
        out.setdefault(test, {})[tag] = a
    json.dump(out, sys.stdout, indent=1, default=str)


if __name__ == "__main__":
    main(sys.argv[1])
