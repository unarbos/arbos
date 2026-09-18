#!/usr/bin/env python3
"""kernel.py <target> history [n] | read <path> | frames <secs> | hello
         | feedback <agent> [seq=N|call=ID] [tail=N] [note...]
target: "pod" (the direct kernel in Secrets.plist) or "<machine>/<project>" through the hub.
hello: the kernel's own `--version` line, read off this very socket. Use it,
never the hub's `/list`: the roster keeps one row per machine and takes
git_sha from whichever process registered last, so it can name a build the
process you are talking to is not running. The hub relays a kernel's hello
frame verbatim, so this is the kernel's own word.
feedback: asks the kernel for the bundle of the exchange holding seq/call
(or the last one); the root agent is named "root" here (history aliases
"main", feedback does not). Writes it to ~/mobile-bundles/<utc>-<target>-<agent>.json
and prints where it landed with the counts. F15 runbook: see the symptom,
run this against that kernel, say where it landed.
Never prints a token."""
import json, os, ssl, subprocess, sys, time
from websocket import create_connection
def plist(k): return subprocess.check_output(["plutil","-extract",k,"raw","-o","-",os.path.expanduser("~/arbos/ios/Arbos/Secrets.plist")]).decode().strip()
target, cmd = sys.argv[1], sys.argv[2]
if target == "pod":
    url, tok = plist("kernelURL"), plist("kernelToken")
    # the app may have followed a redirect; a local override file wins
    ov = os.path.expanduser("~/.kernel-url-override")
    if os.path.exists(ov): url = open(ov).read().strip()
else:
    m, p = target.split("/", 1)
    hub = os.environ.get("KERNEL_HUB") or plist("hubURL"); tok = os.environ.get("KERNEL_TOKEN") or plist("hubToken")
    url = hub.rstrip("/") + f"/attach/{m}/{p}"
ws = create_connection(url, header=["Authorization: Bearer " + tok], sslopt={"cert_reqs": ssl.CERT_NONE}, timeout=8)
def frames(secs):
    t0 = time.time()
    while time.time() - t0 < secs:
        try: raw = ws.recv()
        except Exception: continue
        if isinstance(raw, str): yield json.loads(raw)
if cmd == "history":
    n = int(sys.argv[3]) if len(sys.argv) > 3 else 60
    # `since 0` answers with the newest 200 lines (history_end says from/to/total).
    ws.send(json.dumps({"type": "history", "agent": "main", "since": 0, "limit": 200}))
    lines = []
    for f in frames(25):
        t = f.get("type")
        if t == "history_end": break
        if t != "replayed": continue
        e = f.get("event", {}); k = e.get("kind")
        if k in ("user", "assistant", "say", "notice", "ask", "answer", "notify", "interrupted", "turn_complete"):
            txt = (e.get("text") or e.get("goal") or "").replace("\n", " ")
            if k == "assistant" and not txt: continue
            lines.append(f"{e.get('seq',0):5} {k:13} {txt[:8000]}")
        elif k == "tool":
            a = e.get("args", {}) or {}
            err = f"  ERROR: {e['error']}" if e.get("error") else ""
            lines.append(f"{e.get('seq',0):5} tool          {e.get('name','')} {str(a.get('command') or a.get('path') or a.get('goal') or a.get('name') or '')[:200]}{err}")
    print("\n".join(lines[-n:]))
elif cmd == "read":
    path = sys.argv[3]
    ws.send(json.dumps({"type": "read", "path": path}))
    for f in frames(15):
        if f.get("type") == "file":
            if f.get("error"): print("ERROR", f["error"]); sys.exit(2)
            print(f.get("text", "")); break
        if f.get("type") == "error": print("ERROR", f.get("detail")); sys.exit(2)
elif cmd == "feedback":
    agent = sys.argv[3]
    req = {"type": "feedback", "agent": agent, "tail": 40}
    words = []
    for a in sys.argv[4:]:
        if a.startswith("seq="): req["seq"] = int(a[4:])
        elif a.startswith("call="): req["call_id"] = a[5:]
        elif a.startswith("tail="): req["tail"] = int(a[5:])
        else: words.append(a)
    if words: req["note"] = " ".join(words)
    ws.send(json.dumps(req))
    for f in frames(30):
        t = f.get("type")
        if t == "feedback_bundle":
            out = os.path.expanduser("~/mobile-bundles"); os.makedirs(out, exist_ok=True)
            name = time.strftime("%Y%m%d-%H%M%S") + "-" + target.replace("/", "_") + "-" + agent + ".json"
            path = os.path.join(out, name)
            with open(path, "w") as fh: json.dump(f, fh, indent=1)
            k = f.get("kernel", {}) or {}
            print(f"landed {path}")
            print(f"kernel {k.get('version')} {k.get('git_sha') or 'no-sha'} built {k.get('built_at') or '?'} model {k.get('model')}  bytes {f.get('bytes')}  truncated {f.get('truncated')}")
            print(f"events {len(f.get('events', []))}  tail {len(f.get('tail', []))}  children {len(f.get('children', []))}  log {len(f.get('log', []))}  redacted {f.get('redacted')}")
            turn = f.get("turn", {}) or {}
            print("turn", json.dumps(turn)[:300])
            break
        if t == "error": print("ERROR", f.get("detail")); sys.exit(2)
    else:
        print("no feedback_bundle within 30 s"); sys.exit(3)
elif cmd == "total":
    # The kernel's own count of the transcript, from `history_end`. Counting
    # the lines `history` prints instead measures a capped tail through a
    # filter, which moves for reasons of its own: cycle 43's first attempt
    # read 150 then 149 across a minute in which nothing was sent, and the
    # comparison it was for was worthless.
    # `total [agent]`: the root by default, or a named worker — which is how
    # to tell "the app drew an empty worker chat" from "the kernel has
    # nothing for that worker" without reading either off the screen.
    agent = sys.argv[3] if len(sys.argv) > 3 else "main"
    ws.send(json.dumps({"type": "history", "agent": agent, "since": 0, "limit": 1}))
    # The agent must match. Attaching starts the root's own replay, so its
    # `history_end` usually arrives first, and taking whichever came first
    # printed the root's count under a worker's name — the same number for
    # every worker asked about. Cycle 46's "8 of 8 workers answer total: 0"
    # was read this way and has to be taken again (M-224).
    # `main` and `root` are the same agent: the request aliases one to the
    # other and the answer comes back under `root`. Filtering on the name as
    # asked reported 0 for the root, which is the opposite mistake.
    want = {agent, "root"} if agent == "main" else {agent}
    for f in frames(25):
        # The frame must *say* which agent it is for. Defaulting a missing
        # field to the one asked about made any unlabelled `history_end`
        # match, so the root's frame could win the race and print the root's
        # count under a worker's name. It did: `count-slowly-one-to-forty`
        # read 2350, the root's total, where its own is 11.
        if f.get("type") == "history_end" and f.get("agent") in want:
            print(f.get("total", 0)); break
    else:
        print("-1"); sys.exit(3)
elif cmd == "hello":
    # The kernel sends `hello` unprompted on connect. Print the same shape
    # `arbos-kernel --version` prints, so a verdict can be compared with a
    # commit, plus built_at because semver moves rarely.
    for f in frames(15):
        if f.get("type") != "hello": continue
        line = f"arbos-kernel {f.get('kernel','?')} {f.get('git_sha') or 'unknown'} protocol {f.get('protocol','?')}"
        if f.get("binary_gone"): line += " BINARY-GONE"
        print(line)
        print(json.dumps({
            "version_line": line,
            "kernel": f.get("kernel"), "git_sha": f.get("git_sha") or None,
            "built_at": f.get("built_at") or None, "protocol": f.get("protocol"),
            # #385: the kernel's own word that the file it started from is
            # gone. It serves happily in that state and refuses every spawn
            # (JB-6), so a run scored against it is measuring a ghost.
            "binary_gone": bool(f.get("binary_gone")),
            "target": target, "asked": "attach socket (not the hub roster)",
            "at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        }))
        break
    else:
        print("no hello frame within 15 s"); sys.exit(3)
elif cmd == "frames":
    kinds = {}
    for f in frames(int(sys.argv[3])):
        t = f.get("type"); key = ("replayed:" + str(f.get("event", {}).get("kind"))) if t == "replayed" else "frame:" + str(t)
        kinds[key] = kinds.get(key, 0) + 1
    print(json.dumps(kinds))
ws.close()
