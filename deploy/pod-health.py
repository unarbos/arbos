#!/usr/bin/env python3
"""Hourly probe of the voice pod's public endpoints, from the QA loop.

    pod-health.py <loop-dir>

Reads the three tunnel URLs over ssh (kernel-url.txt, public-url.txt, hub-url.txt on the pod),
then checks each the way its clients use it:
  kernel  — a WebSocket handshake; the kernel must answer with a frame (hello, or the
            auth-required error off loopback). A plain GET is *expected* to 502 (qa-036).
  voice   — GET /healthz on the gateway must be 200.
  hub     — GET / must answer (any HTTP status < 500).
Appends one line to <loop>/pod-health.jsonl and drafts bugs/pod-<name>.md when a probe is red.
Never prints tokens; the ssh key is the agents' key (~/.ssh/arbos_agents).
"""

import asyncio
import json
import os
import subprocess
import sys
import time
import urllib.request

POD = ["ssh", "-i", os.path.expanduser("~/.ssh/arbos_agents"), "-o", "BatchMode=yes", "-o", "ConnectTimeout=15", "-p", "40300", "root@216.243.220.25"]


def pod_urls():
    try:
        out = subprocess.run(POD + ["for f in kernel-url public-url hub-url; do echo \"$f=$(cat /root/arbos-voice/$f.txt 2>/dev/null)\"; done"], capture_output=True, text=True, timeout=40).stdout
    except Exception as e:  # noqa: BLE001
        return {}, f"ssh: {e}"
    urls = dict(l.split("=", 1) for l in out.splitlines() if "=" in l)
    return urls, ""


def http(url, timeout=15):
    try:
        req = urllib.request.Request(url, headers={"User-Agent": "arbos-qa pod-health"})
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return r.status, ""
    except urllib.error.HTTPError as e:
        return e.code, ""
    except Exception as e:  # noqa: BLE001
        return 0, f"{type(e).__name__}: {e}"[:160]


def ws_kernel(url):
    try:
        import websockets  # noqa: PLC0415  (optional dependency; the harness venv has it)
    except Exception:  # noqa: BLE001
        return False, "websockets module missing"

    async def go():
        async with websockets.connect(url, open_timeout=15) as ws:
            return await asyncio.wait_for(ws.recv(), 10)

    try:
        msg = asyncio.run(go())
        ok = '"type"' in str(msg)
        return ok, str(msg)[:120]
    except Exception as e:  # noqa: BLE001
        return False, f"{type(e).__name__}: {e}"[:160]


def main():
    loop = sys.argv[1]
    stamp = time.strftime("%Y%m%dT%H%M%SZ", time.gmtime())
    urls, err = pod_urls()
    line = {"ts": stamp, "probes": {}}
    if err:
        line["error"] = err
    k = urls.get("kernel-url", "").strip()
    if k:
        ok, detail = ws_kernel(k.replace("https://", "wss://").rstrip("/") + "/")
        code, _ = http(k)
        line["probes"]["kernel"] = {"ws_ok": ok, "ws": detail, "http_get": code}
    v = urls.get("public-url", "").strip()
    if v:
        code, e = http(v.rstrip("/") + "/healthz")
        line["probes"]["voice"] = {"healthz": code, "error": e}
    h = urls.get("hub-url", "").strip()
    if h:
        # hub-url.txt holds a wss:// URL; the hub answers HTTP on the same host.
        code, e = http(h.replace("wss://", "https://").replace("ws://", "http://"))
        line["probes"]["hub"] = {"http": code, "error": e}
    red = []
    p = line["probes"]
    if "kernel" in p and not p["kernel"]["ws_ok"]:
        red.append(("kernel", f"phone kernel tunnel: no frame on a WebSocket handshake ({p['kernel']['ws']}); plain GET {p['kernel']['http_get']}"))
    if "voice" in p and p["voice"]["healthz"] != 200:
        red.append(("voice", f"voice gateway /healthz -> {p['voice']['healthz']} {p['voice']['error']}"))
    if "hub" in p and not (0 < p["hub"]["http"] < 500):
        red.append(("hub", f"hub -> {p['hub']['http']} {p['hub']['error']}"))
    line["red"] = [r[0] for r in red]
    os.makedirs(loop, exist_ok=True)
    with open(os.path.join(loop, "pod-health.jsonl"), "a") as f:
        f.write(json.dumps(line) + "\n")
    print(f"-- pod-health: {json.dumps(p)}" + (f"; RED {line['red']}" if red else "") + (f"; {err}" if err else ""))
    bugs = os.path.join(loop, "bugs")
    os.makedirs(bugs, exist_ok=True)
    for name, detail in red:
        path = os.path.join(bugs, f"pod-{name}-down.md")
        if os.path.exists(path):
            with open(path, "a") as f:
                f.write(f"\n- seen again {stamp}: {detail}\n")
        else:
            with open(path, "w") as f:
                f.write(f"# pod-{name}-down: the voice pod's {name} endpoint failed its hourly probe\n\n- First seen: {stamp}\n- Probe: `deploy/pod-health.py`\n- Detail: {detail}\n\nCheck on the pod: `tmux ls`, `ss -ltnp`, `/root/arbos-voice/logs/supervisor.log`, `logs/kquick.log`.\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
