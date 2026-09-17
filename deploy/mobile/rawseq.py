import json, os, ssl, subprocess, sys, time
from websocket import create_connection
def plist(k): return subprocess.check_output(["plutil","-extract",k,"raw","-o","-",os.path.expanduser("~/arbos/ios/Arbos/Secrets.plist")]).decode().strip()
target=sys.argv[1]; want=set(int(x) for x in sys.argv[2:])
m,p=target.split("/",1)
ws=create_connection(plist("hubURL").rstrip("/")+f"/attach/{m}/{p}", header=["Authorization: Bearer "+plist("hubToken")], sslopt={"cert_reqs": ssl.CERT_NONE}, timeout=8)
ws.send(json.dumps({"type":"history","agent":"main","since":0,"limit":200}))
t0=time.time()
while time.time()-t0<25:
    try: raw=ws.recv()
    except Exception: break
    if not isinstance(raw,str): continue
    f=json.loads(raw)
    if f.get("type")=="history_end": break
    if f.get("type")=="replayed" and f["event"].get("seq") in want:
        e=f["event"]; e.pop("text",None) if e.get("kind")=="assistant" else None
        print(json.dumps(e)[:1500])
