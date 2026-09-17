#!/usr/bin/env python3
"""hub-history.py <machine> <project> [n] — the last n lines of the project's root transcript, through the hub.
Reads hub URL + token from ~/arbos/ios/Arbos/Secrets.plist (never printed). Live frames after the replay
are ignored; `history_end` ends the read."""
import json, os, ssl, subprocess, sys
from websocket import create_connection
def plist(k): return subprocess.check_output(["plutil","-extract",k,"raw","-o","-",os.path.expanduser("~/arbos/ios/Arbos/Secrets.plist")]).decode().strip()
machine, project = sys.argv[1], sys.argv[2]; n=int(sys.argv[3]) if len(sys.argv)>3 else 40
ws=create_connection(plist("hubURL").rstrip("/")+f"/attach/{machine}/{project}", header=[f"Authorization: Bearer {plist('hubToken')}"], sslopt={"cert_reqs": ssl.CERT_NONE}, timeout=20)
ws.send(json.dumps({"type":"history","agent":"main","since":0,"limit":200}))
lines=[]
while True:
    try: raw=ws.recv()
    except Exception: break
    if not isinstance(raw,str): continue
    f=json.loads(raw); t=f.get("type")
    if t=="history_end": break
    if t!="replayed": continue
    e=f.get("event",{}); k=e.get("kind")
    if k in("user","assistant","say","notice","spawn","ask","answer"):
        txt=(e.get("text") or e.get("goal") or e.get("name") or "").replace("\n"," ")
        if k=="assistant" and not txt: continue
        lines.append(f"{e.get('seq',0):5} {k:9} {txt[:220]}")
    elif k=="tool":
        a=e.get("args",{}); lines.append(f"{e.get('seq',0):5} tool      {e.get('name','')} {str(a.get('command') or a.get('path') or a.get('goal') or '')[:160]}")
ws.close()
print("\n".join(lines[-n:]))
