#!/usr/bin/env python3
"""Read-out of the anonymous performance diagnostics the iPhone app forwards from MetricKit
(iphone-scroll-perf, 2026-10-27): one markdown table per app build from the relay's
`GET /admin/perf/summary` — scroll hitch time ratio (ms of hitch per second of scrolling; Apple
calls < 5 good, > 10 poor), hang seconds and hang count, mean time to first draw, mean resume
time, peak memory — one row per payload (one device-day), plus the diagnostic payload counts.

    perf-readout.py [--since 2026-10-27T00:00:00Z] [--relay https://relay.arbos.life] [--json]

Token: ARBOS_RELAY_ADMIN_TOKEN (or RELAY_ADMIN_TOKEN) in the environment — the loop's
/etc/arbos/feedback.env has it. Nothing here identifies a person or a device: the relay never
stored that (see crates/arbos-relay/src/perf.rs).
"""
import argparse
import datetime
import json
import os
import sys
import urllib.parse
import urllib.request


def fetch(relay: str, token: str, since_ms: int) -> dict:
    url = f"{relay.rstrip('/')}/admin/perf/summary?{urllib.parse.urlencode({'since': since_ms})}"
    req = urllib.request.Request(url, headers={"Authorization": f"Bearer {token}"})
    with urllib.request.urlopen(req, timeout=30) as resp:
        return json.load(resp)


def fmt(v, digits=1, suffix=""):
    if v is None:
        return "—"
    return f"{v:.{digits}f}{suffix}"


def render(summary: dict) -> str:
    out = []
    builds = summary.get("builds", {})
    if not builds:
        return "No performance diagnostics received yet."
    for build, data in builds.items():
        days = data.get("metric_days", [])
        diag = data.get("diagnostics", {})
        out.append(f"### {build} — {len(days)} metric payload(s), hangs diagnosed {diag.get('hangs', 0)}, crashes {diag.get('crashes', 0)}")
        if days:
            out.append("")
            out.append("| received (UTC) | device | iOS | hitch ms/s | hangs | hang s | first draw ms | resume ms | peak MB |")
            out.append("| --- | --- | --- | --- | --- | --- | --- | --- | --- |")
            for d in days:
                when = datetime.datetime.fromtimestamp(d["received_at"] / 1000, tz=datetime.timezone.utc).strftime("%m-%d %H:%M")
                out.append(
                    f"| {when} | {d.get('device_model') or '—'} | {d.get('os') or '—'} | {fmt(d.get('scroll_hitch_ms_per_s'))} | "
                    f"{d.get('hangs') if d.get('hangs') is not None else '—'} | {fmt(d.get('hang_seconds'), 2)} | "
                    f"{fmt(d.get('launch_first_draw_ms'), 0)} | {fmt(d.get('resume_ms'), 0)} | {fmt(d.get('peak_memory_mb'), 0)} |"
                )
        out.append("")
    return "\n".join(out)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--since", default=None, help="ISO-8601 UTC; default: 7 days ago")
    ap.add_argument("--relay", default=os.environ.get("ARBOS_RELAY_URL", "https://relay.arbos.life"))
    ap.add_argument("--json", action="store_true", help="print the raw summary")
    args = ap.parse_args()
    token = os.environ.get("ARBOS_RELAY_ADMIN_TOKEN") or os.environ.get("RELAY_ADMIN_TOKEN")
    if not token:
        print("ARBOS_RELAY_ADMIN_TOKEN is not set", file=sys.stderr)
        return 2
    if args.since:
        since = datetime.datetime.fromisoformat(args.since.replace("Z", "+00:00"))
    else:
        since = datetime.datetime.now(datetime.timezone.utc) - datetime.timedelta(days=7)
    summary = fetch(args.relay, token, int(since.timestamp() * 1000))
    if args.json:
        json.dump(summary, sys.stdout, indent=2)
        print()
    else:
        print(render(summary))
    return 0


if __name__ == "__main__":
    sys.exit(main())
