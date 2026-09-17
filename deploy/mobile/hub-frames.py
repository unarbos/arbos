#!/usr/bin/env python3
"""hub-frames.py <machine> <project> <seconds> — attach and count every frame kind for N seconds; print notify/seen frames."""
import json, os, ssl, subprocess, sys, time
from websocket import create_connection
def plist(k): return subprocess.check_output(["plutil","-extract",k,"raw","-o","-",os.path.expanduser("~/arbos/ios/Arbos/Secrets.plist")]).decode().strip()
m,p,secs=sys.argv[1],sys.argv[2],int(sys.argv[3])
ws=create_connection(plist("hubURL").rstrip("/")+f"/attach/{m}/{p}", header=["Authorization: Bearer "+plist("hubToken")], sslopt={"cert_reqs": ssl.CERT_NONE}, timeout=5)
kinds={}; notes=[]; t0=time.time()
while time.time()-t0<secs:
    try: raw=ws.recv()
    except Exception: continue
    if not isinstance(raw,str): continue
    f=json.loads(raw); t=f.get("type")
    key=("replayed:"+str(f.get("event",{}).get("kind"))) if t=="replayed" else "frame:"+str(t)
    kinds[key]=kinds.get(key,0)+1
    if "notify" in key or "seen" in key: notes.append(raw[:240])
print(json.dumps(kinds, indent=0)); print("\n".join(notes[-6:]))
