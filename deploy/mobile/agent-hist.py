import json, os, ssl, subprocess, sys, time
from websocket import create_connection
def plist(k): return subprocess.check_output(["plutil","-extract",k,"raw","-o","-",os.path.expanduser("~/arbos/ios/Arbos/Secrets.plist")]).decode().strip()
agent=sys.argv[1]
ws=create_connection(plist("kernelURL"), header=["Authorization: Bearer "+plist("kernelToken")], sslopt={"cert_reqs": ssl.CERT_NONE}, timeout=6)
t0=time.time(); seen_end=0
while time.time()-t0<8:
    try: raw=ws.recv()
    except Exception: break
    f=json.loads(raw) if isinstance(raw,str) else {}
    if f.get("type")=="history_end": seen_end+=1; break
ws.send(json.dumps({"type":"history","agent":agent,"since":0,"limit":50}))
kinds={}; n=0; t0=time.time()
while time.time()-t0<8:
    try: raw=ws.recv()
    except Exception: break
    f=json.loads(raw) if isinstance(raw,str) else {}
    t=f.get("type")
    if t=="history_end": print("history_end", {k:f.get(k) for k in ("agent","total","from","to")}); break
    if t=="error": print("error", f.get("detail")); break
    if t=="replayed" and f.get("agent")==agent: n+=1; k=f["event"].get("kind"); kinds[k]=kinds.get(k,0)+1
print(agent, "replayed", n, kinds)
