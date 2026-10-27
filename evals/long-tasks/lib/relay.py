"""Relay access for the long-task eval: HTTP with the test user's token, arbosctl wrapper,
usage, routines. Nothing here talks to a model; everything goes through the product path."""

import json
import os
import subprocess
import time
import urllib.error
import urllib.request
from pathlib import Path

RELAY = os.environ.get("ARBOS_RELAY_URL", "https://relay.arbos.life").rstrip("/")
STATE_DIR = Path(os.environ.get("LT_STATE_DIR", "/tmp/lt"))
TOKENS = STATE_DIR / "tokens.json"
CTL_CONFIG = STATE_DIR / "ctl" / "arbosctl.json"
ARBOSCTL = os.environ.get("ARBOSCTL", "arbosctl")


def http(method, url, token=None, body=None, timeout=60, headers=None):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method)
    req.add_header("content-type", "application/json")
    for k, v in (headers or {}).items():
        req.add_header(k, v)
    if token:
        req.add_header("authorization", f"Bearer {token}")
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            raw = r.read()
            try:
                return r.status, (json.loads(raw) if raw else None)
            except json.JSONDecodeError:
                return r.status, {"raw": raw.decode(errors="replace")}
    except urllib.error.HTTPError as e:
        raw = e.read()
        try:
            return e.code, json.loads(raw)
        except Exception:
            return e.code, {"error": raw.decode(errors="replace")[:500]}
    except Exception as e:  # network
        return 0, {"error": str(e)}


def admin_token():
    return os.environ["ARBOS_RELAY_ADMIN_TOKEN"]


def ensure_test_user(handle, name):
    """Create the test user once; afterwards reuse the saved token pair."""
    if TOKENS.exists():
        return json.loads(TOKENS.read_text())
    st, v = http("POST", f"{RELAY}/admin/users/test", token=admin_token(), body={"handle": handle, "name": name})
    if st != 201:
        raise SystemExit(f"create test user: {st} {v}")
    v["issued_at"] = time.time()
    STATE_DIR.mkdir(parents=True, exist_ok=True)
    TOKENS.write_text(json.dumps(v))
    write_ctl_config(v["access_token"], v.get("device_id", ""))
    return v


def write_ctl_config(access_token, device_id):
    CTL_CONFIG.parent.mkdir(parents=True, exist_ok=True)
    CTL_CONFIG.write_text(json.dumps({"relay_url": RELAY, "device_token": access_token, "device_id": device_id}, indent=2))


def user_token():
    """Access token, refreshed when < 10 min remain (the relay issues 1 h tokens)."""
    t = json.loads(TOKENS.read_text())
    issued = t.get("issued_at", TOKENS.stat().st_mtime)
    if time.time() > issued + t.get("expires_in", 3600) - 600:
        st, new = http("POST", f"{RELAY}/auth/refresh", body={"refresh_token": t["refresh_token"]})
        if st != 200:
            raise SystemExit(f"token refresh failed: {st} {new}")
        for k in ("user", "device_id", "test"):
            new.setdefault(k, t.get(k))
        new["issued_at"] = time.time()
        TOKENS.write_text(json.dumps(new))
        write_ctl_config(new["access_token"], new.get("device_id", ""))
        t = new
    return t["access_token"]


def user_id():
    return json.loads(TOKENS.read_text())["user"]["id"]


def relay(method, path, body=None, timeout=60):
    return http(method, f"{RELAY}{path}", token=user_token(), body=body, timeout=timeout)


def admin(method, path, body=None):
    return http(method, f"{RELAY}{path}", token=admin_token(), body=body)


def me():
    st, v = relay("GET", "/me")
    return v if st == 200 else {}


def spent_today():
    c = (me() or {}).get("credits") or {}
    return float(c.get("spent_today_usd") or 0.0), float(c.get("daily_cap_usd") or 0.0)


def machine():
    st, v = relay("GET", "/machine")
    return v if st == 200 else {}


def usage_all(project):
    st, v = relay("GET", f"/projects/{project}/usage")
    if st != 200:
        return {}
    return v.get("all") or {}


def diff_usage(a, b):
    return {k: round((b.get(k, 0) or 0) - (a.get(k, 0) or 0), 6) for k in ("calls", "input", "output", "cache_read", "cache_write", "usd")}


def routines(project):
    st, v = relay("GET", f"/projects/{project}/routines")
    if st != 200:
        return []
    if isinstance(v, list):
        return v
    return v.get("routines") or v.get("items") or []


def routine_runs(project, routine_id, limit=20):
    st, v = relay("GET", f"/projects/{project}/routines/{routine_id}/runs?limit={limit}")
    if st != 200:
        return []
    if isinstance(v, list):
        return v
    return v.get("runs") or v.get("items") or []


def delete_routine(project, routine_id):
    return relay("DELETE", f"/projects/{project}/routines/{routine_id}")


def projects():
    st, v = relay("GET", "/projects")
    if st != 200:
        return []
    return v if isinstance(v, list) else (v.get("projects") or v.get("items") or [])


# ---- arbosctl ----------------------------------------------------------------------------------


def ctl(*args, timeout=900, input_text=None):
    user_token()
    env = dict(os.environ, ARBOSCTL_CONFIG=str(CTL_CONFIG))
    p = subprocess.run([ARBOSCTL, *args], capture_output=True, text=True, timeout=timeout, env=env, input=input_text)
    return p.returncode, p.stdout, p.stderr


def ctl_popen(*args):
    user_token()
    env = dict(os.environ, ARBOSCTL_CONFIG=str(CTL_CONFIG))
    return subprocess.Popen([ARBOSCTL, *args], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, env=env, bufsize=1)


def ensure_project(name):
    for p in projects():
        if p.get("name") == name:
            return p["id"]
    code, out, err = ctl("project", "create", "--name", name)
    if code != 0:
        raise SystemExit(f"project create {name}: {err}")
    for p in projects():
        if p.get("name") == name:
            return p["id"]
    raise SystemExit(f"project {name} not listed after create")


def chat_open(project, title):
    code, out, err = ctl("chat", "open", project, "--title", title)
    if code != 0:
        raise RuntimeError(f"chat open: {err[-300:]}")
    chat_id = out.strip().splitlines()[-1].split()[-1]
    # A send in the first second of a brand-new chat lost its turn 4 times in 48 on an earlier
    # eval; give the box a moment to join the room.
    time.sleep(3)
    return chat_id


def chat_dump(project, chat_id, reset=False):
    # Keep the local replica between dumps. After `chat reset` the client asks the relay for rows
    # with `excludeOwn`, so its own user rows are gone and the box's rows that causally depend on
    # them stay pending until the relay checkpoints — the doc then looks empty for minutes. A
    # reset is only a repair step when a replica looks stuck (#1103 catch-up race).
    if reset:
        ctl("chat", "reset", project, chat_id, timeout=60)
    code, out, err = ctl("chat", "dump", project, chat_id, timeout=120)
    i = out.find("{")
    if i < 0:
        return {"messages": []}
    try:
        return json.loads(out[i:])
    except json.JSONDecodeError:
        return {"messages": []}


def pinned_chat(project):
    st, v = relay("GET", f"/projects/{project}/chats")
    chats = v if isinstance(v, list) else ((v or {}).get("chats") or [])
    for c in chats:
        if c.get("is_pinned") or c.get("pinned"):
            return c["id"]
    return None


def patch_project(project, body):
    return relay("PATCH", f"/projects/{project}", body)


def project_name(project):
    st, v = relay("GET", f"/projects/{project}")
    return ((v or {}).get("project") or v or {}).get("name") if st == 200 else None


def create_secret(name, value, note="", agents=(), inject_env=False):
    body = {"name": name, "value": value, "note": note, "agents": [{"project_id": p} for p in agents]}
    if inject_env:
        body["inject_env"] = True
    return relay("POST", "/secrets", body)


def delete_secret(secret_id):
    return relay("DELETE", f"/secrets/{secret_id}")


def project_secret_requests(project):
    st, v = relay("GET", f"/projects/{project}/secrets")
    return [r for r in ((v or {}).get("requests") or []) if r.get("status") == "pending"] if st == 200 else []


def decide_secret_request(request_id, decision="once"):
    return relay("POST", f"/secrets/requests/{request_id}/decide", {"decision": decision})


def group_create(host, with_names, title):
    code, out, err = ctl("group", "create", "--host", host, "--with", ",".join(with_names), "--title", title)
    if code != 0:
        raise RuntimeError(f"group create: {err[-300:]}")
    return out.strip().splitlines()[0].split()[0]


def group_send(host, chat_id, text, wait_s=120):
    code, out, err = ctl("group", "send", host, chat_id, text, "--stream", "--wait", str(wait_s), timeout=wait_s + 700)
    return out + "\n" + err


def people_dm_with_agent(peer_handle, project):
    """A DM between the eval user and a scratch peer (created once, reused), with `project`'s
    agent added. Returns (chat_id, peer access token)."""
    st, users = admin("GET", "/admin/users")
    users = users if isinstance(users, list) else (users or {}).get("users") or []
    peer = next((u for u in users if u.get("login") == peer_handle), None)
    if peer is None:
        st, v = admin("POST", "/admin/users/test", {"handle": peer_handle, "name": "Long-task eval peer"})
        if st != 201:
            raise RuntimeError(f"create peer: {st} {v}")
        peer_id = v["user"]["id"]
    else:
        peer_id = peer["id"]
    st, tok = admin("POST", f"/admin/users/{peer_id}/devices", {"name": "lt eval driver"})
    if st not in (200, 201):
        raise RuntimeError(f"peer device: {st} {tok}")
    st, chat = relay("POST", "/chats/dm", {"handle": peer_handle})
    if st not in (200, 201):
        raise RuntimeError(f"dm create: {st} {chat}")
    chat_id = chat["id"]
    st, v = relay("POST", f"/chats/{chat_id}/agents", {"project_id": project})
    if st not in (200, 201, 409):
        raise RuntimeError(f"agent add: {st} {v}")
    return chat_id, {"access_token": tok["access_token"], "device_id": tok.get("device_id", "")}


def people_group_with_agents(peer_handle, projects, title):
    """A people GROUP (the Arbonites shape) between the eval user and a scratch peer, with these
    projects' agents added. Returns (chat_id, peer access token) like `people_dm_with_agent`, so
    the `chat: "dm"` turn kinds (peer rows, quiet checks) work on it."""
    st, users = admin("GET", "/admin/users")
    users = users if isinstance(users, list) else (users or {}).get("users") or []
    peer = next((u for u in users if u.get("login") == peer_handle), None)
    if peer is None:
        st, v = admin("POST", "/admin/users/test", {"handle": peer_handle, "name": "Long-task eval peer"})
        if st != 201:
            raise RuntimeError(f"create peer: {st} {v}")
        peer_id = v["user"]["id"]
    else:
        peer_id = peer["id"]
    st, tok = admin("POST", f"/admin/users/{peer_id}/devices", {"name": "lt eval driver"})
    if st not in (200, 201):
        raise RuntimeError(f"peer device: {st} {tok}")
    st, chat = relay("POST", "/chats", {"kind": "group", "members": [peer_handle], "agent_ids": list(projects), "title": title})
    if st not in (200, 201):
        raise RuntimeError(f"group create: {st} {chat}")
    chat_id = (chat.get("chat") or chat).get("id") if isinstance(chat, dict) else None
    if not chat_id:
        raise RuntimeError(f"group create: no id in {chat}")
    return chat_id, {"access_token": tok["access_token"], "device_id": tok.get("device_id", "")}


def people_send_as(peer, chat_id, text):
    """One human row from the peer into the DM, written through the peer's own arbosctl config
    (device id included, so the row carries the peer's identity rather than `dev:anon`)."""
    cfg = STATE_DIR / "ctl-peer" / "arbosctl.json"
    cfg.parent.mkdir(parents=True, exist_ok=True)
    cfg.write_text(json.dumps({"relay_url": RELAY, "device_token": peer["access_token"], "device_id": peer.get("device_id", "")}))
    env = dict(os.environ, ARBOSCTL_CONFIG=str(cfg))
    p = subprocess.run([ARBOSCTL, "chat", "send", "people", chat_id, text, "--timeout", "5"], capture_output=True, text=True, timeout=60, env=env)
    return p.returncode


def capped_user_turn(handle, cap_usd, prompt, timeout_s=240, second_prompt=None):
    """Create (once) a scratch user with a tiny daily cap, one project, send one message as them
    and return what the chat showed. Lets the suite see the cap behaviour without capping lt-eval."""
    st, users = admin("GET", "/admin/users")
    users = users if isinstance(users, list) else (users or {}).get("users") or []
    u = next((x for x in users if x.get("login") == handle), None)
    if u is None:
        st, v = admin("POST", "/admin/users/test", {"handle": handle, "name": "Long-task eval capped"})
        if st != 201:
            raise RuntimeError(f"create capped user: {st} {v}")
        uid = v["user"]["id"]
    else:
        uid = u["id"]
    st, tok = admin("POST", f"/admin/users/{uid}/devices", {"name": "lt eval capped driver"})
    if st not in (200, 201):
        raise RuntimeError(f"capped device: {st} {tok}")
    cfg = STATE_DIR / "ctl-capped" / "arbosctl.json"
    cfg.parent.mkdir(parents=True, exist_ok=True)
    cfg.write_text(json.dumps({"relay_url": RELAY, "device_token": tok["access_token"], "device_id": tok.get("device_id", "")}))
    env = dict(os.environ, ARBOSCTL_CONFIG=str(cfg))
    subprocess.run([ARBOSCTL, "admin", "cap", uid, str(cap_usd)], capture_output=True, text=True, timeout=60, env=dict(os.environ, ARBOSCTL_CONFIG=str(CTL_CONFIG)))
    st, projects_v = http("GET", f"{RELAY}/projects", token=tok["access_token"])
    plist = projects_v if isinstance(projects_v, list) else (projects_v or {}).get("projects") or []
    if not plist:
        subprocess.run([ARBOSCTL, "project", "create", "--name", "capped-lane"], capture_output=True, text=True, timeout=120, env=env)
        st, projects_v = http("GET", f"{RELAY}/projects", token=tok["access_token"])
        plist = projects_v if isinstance(projects_v, list) else (projects_v or {}).get("projects") or []
    pid = plist[0]["id"]
    subprocess.run([ARBOSCTL, "machine", "start"], capture_output=True, text=True, timeout=120, env=env)
    for _ in range(30):
        st, m = http("GET", f"{RELAY}/machine", token=tok["access_token"])
        if st == 200 and m.get("state") == "awake":
            break
        time.sleep(10)
    time.sleep(20)
    p = subprocess.run([ARBOSCTL, "chat", "open", pid, "--title", "cap check"], capture_output=True, text=True, timeout=60, env=env)
    chat_id = p.stdout.strip().splitlines()[-1].split()[-1]
    time.sleep(3)
    t0 = time.time()
    p = subprocess.run([ARBOSCTL, "chat", "send", pid, chat_id, prompt, "--stream", "--timeout", str(timeout_s)], capture_output=True, text=True, timeout=timeout_s + 60, env=env)
    out = p.stdout + "\n" + p.stderr
    if second_prompt:
        # the cap is judged on debits: the first message spends, the second must be refused plainly
        time.sleep(5)
        p = subprocess.run([ARBOSCTL, "chat", "send", pid, chat_id, second_prompt, "--stream", "--timeout", str(timeout_s)], capture_output=True, text=True, timeout=timeout_s + 60, env=env)
        out += "\n" + p.stdout + "\n" + p.stderr
    p2 = subprocess.run([ARBOSCTL, "chat", "dump", pid, chat_id], capture_output=True, text=True, timeout=60, env=env)
    s = p2.stdout
    doc = json.loads(s[s.find("{"):]) if "{" in s else {"messages": []}
    rows = [m for m in doc.get("messages", []) if m.get("role") == "assistant"]
    # what the user reads: text parts and the error line the product shows when the cap trips
    reply = "\n\n".join("".join((pp.get("text") or pp.get("message") or "") for pp in m.get("parts", []) if pp.get("kind") in ("text", "error")) for m in rows)
    users_after = [m for m in doc.get("messages", []) if m.get("role") == "user"]
    answered = sum(1 for m in rows if any(pp.get("kind") in ("text", "error") and (pp.get("text") or pp.get("message")) for pp in m.get("parts", [])))
    errors = [pp.get("message") or pp.get("text") for m in rows for pp in m.get("parts", []) if pp.get("kind") in ("error", "status") and (pp.get("code") in ("failed", "error", "cap", "daily_cap") or "cap" in str(pp.get("message") or "").lower())]
    st, me = http("GET", f"{RELAY}/me", token=tok["access_token"])
    return {"user_id": uid, "project": pid, "chat_id": chat_id, "reply": reply, "errors": errors, "user_messages": len(users_after), "answered_rows": answered, "routes": [m.get("decision") for m in rows], "wall_s": round(time.time() - t0, 1), "stream_tail": out[-800:], "credits": (me or {}).get("credits"), "cap_hit": any("cap" in str(e).lower() for e in errors) or "cap" in reply.lower()}


def project_activity(project):
    """The working-row line the relay shows for the project (agentd `post_activity`), or None."""
    st, v = relay("GET", f"/projects/{project}")
    if st != 200:
        return None
    a = ((v or {}).get("project") or v or {}).get("activity")
    if isinstance(a, dict):
        return a.get("text")
    return a
