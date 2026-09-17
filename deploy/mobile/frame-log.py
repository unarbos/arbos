#!/usr/bin/env python3
"""frame-log.py <target> <seconds> — print every non-replay frame about agents other than root (turn/status/working/say/tree), with a timestamp."""
import json, os, ssl, subprocess, sys, time
from websocket import create_connection
def plist(k): return subprocess.check_output(["plutil","-extract",k,"raw","-o","-",os.path.expanduser("~/arbos/ios/Arbos/Secrets.plist")]).decode().strip()
target, secs = sys.argv[1], int(sys.argv[2])
if target == "pod": url, tok = plist("kernelURL"), plist("kernelToken")
else:
    m, p = target.split("/", 1); url, tok = plist("hubURL").rstrip("/") + f"/attach/{m}/{p}", plist("hubToken")
ws = create_connection(url, header=["Authorization: Bearer " + tok], sslopt={"cert_reqs": ssl.CERT_NONE}, timeout=5)
t0 = time.time()
while time.time() - t0 < secs:
    try: raw = ws.recv()
    except Exception: continue
    if not isinstance(raw, str): continue
    f = json.loads(raw); t = f.get("type"); ts = f"{time.time()-t0:6.1f}"
    if t in ("turn", "status", "working"):
        if f.get("agent") not in (None, "root", "main"): print(ts, t, json.dumps({k: v for k, v in f.items() if k != "type"})[:200])
    elif t == "tree":
        print(ts, "tree", [(a.get("id"), a.get("step")) for a in f.get("tree", []) if a.get("parent")])
    elif t == "event":
        e = f.get("event", {}); k = e.get("kind")
        if k in ("say", "tool") and (f.get("agent") not in ("root", "main") or e.get("name") == "spawn" or k == "say"):
            print(ts, "event", f.get("agent"), k, (e.get("name") or "") , (e.get("from") or ""), (e.get("text") or "")[:80].replace("\n", " "))
