#!/usr/bin/env python3
"""Production smoke: a busy agent still answers a new message within seconds.

The scenario is Doug's (Jacob, 2026-10-22): the agent is in the middle of a long act turn
(here: `bash sleep`), and the user asks "what is going on now?" in the same chat and then
"hello?" in a brand-new chat. Both must get a first visible reply within --gate-seconds
(default 5) while the long turn is still running. Before the fix both waited for the long turn
to end (measured 93–98 s on the test box).

Runs against one test user on the relay (default: arbosdemo-alice) with THREE devices minted
through the admin API (one per arbosctl client: the long turn, the same-chat probe, the new-chat
probe) and revoked at the end. One device per replica is a rule, not a nicety: rows from a
client's own device are excluded on the way back (relay `excludeOwn`, arbosctl's own-device
skip), so two replicas behind one device id never see each other's rows, and the box's reply —
whose ops depend on the user message — sits as a pending Loro import in the probe's replica.
That showed as "same-chat probe: no row for 150 s" on images 0.1.81 / 0.1.84 (the box had
answered in 2.7 s) and passed on the images in between by timing alone. Needs `arbosctl` on PATH and ARBOS_RELAY_ADMIN_TOKEN in
the environment (the deploy worker passes it). Prints one JSON report line; exit 0 = gate held,
1 = gate failed, 2 = the smoke itself could not run (no verdict). A SIGTERM (an outer `timeout`)
still prints the report — with the verdict if both probes had finished — and appends it to the
log, which is line-buffered so a killed run keeps every arbosctl line it saw.

    busy-reply-smoke.py [--relay URL] [--user USER_ID] [--gate-seconds 5] [--busy-seconds 90]
"""
import argparse
import json
import os
import re
import select
import signal
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request

DEFAULT_USER = "test_01M50KGEEN5RRVNRRPFBMF2N8R"  # arbosdemo-alice (test user)
BUSY_PROMPT = (
    "Use the bash tool yourself to run exactly this command: sleep {n} && echo finished  "
    "(set the tool timeout to {t} seconds). Wait for it to end, then tell me in one line that "
    "it finished. Do it yourself with bash in this turn; do not hand it to a subagent and do not "
    "background it."
)
PROBE_SAME = "what is going on now?"
PROBE_OTHER = "hello? are you there?"


class Terminated(Exception):
    pass


def on_sigterm(_sig, _frame):
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
    raise Terminated()


def api(relay, token, method, path, body=None):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(
        f"{relay}{path}",
        data=data,
        method=method,
        headers={"Authorization": f"Bearer {token}", "content-type": "application/json"},
    )
    with urllib.request.urlopen(req, timeout=30) as r:
        raw = r.read()
        return r.status, (json.loads(raw) if raw.strip() else None)


class Timed:
    """Run `arbosctl chat send … --stream` and time what a user would see on screen."""

    def __init__(self, cmd, log, env=None, procs=None):
        self.cmd, self.log, self.env = cmd, log, env
        self.procs = procs if procs is not None else []
        self.t_sent = self.t_row = self.t_text = self.t_done = None
        self.rows_before, self.rows_after = set(), set()
        self.cur_row, self.first_text = None, ""

    def _handle(self, fd, data):
        now = time.time()
        text = data.decode("utf-8", "replace")
        self.log.write(f"{now:.3f} {self.cmd[4][-6:]} {fd} {text!r}\n")
        if fd == 2:
            if self.t_sent is None and "[sent message" in text:
                self.t_sent = now
            return
        if self.t_sent is None:
            for m in re.finditer(r"\[assistant (\w{6})\] |\[(\w{6}) · \w+: ", text):
                self.rows_before.add(m.group(1) or m.group(2))
            return
        for m in re.finditer(r"\[assistant (\w{6})\] ", text):
            rid = m.group(1)
            if rid in self.rows_before:
                self.cur_row = None
                continue
            self.rows_after.add(rid)
            self.cur_row = rid
        for m in re.finditer(r"\[(\w{6}) · (\w+): ([^\]]*)\]", text):
            rid, kind, _label = m.groups()
            if rid not in self.rows_before and kind in ("working", "queued", "deciding"):
                self.rows_after.add(rid)
                if self.t_row is None:
                    self.t_row = now
            if kind == "done" and rid in self.rows_after and self.t_done is None:
                self.t_done = now
        stripped = re.sub(r"\[assistant \w{6}\] |\[\w{6} · \w+: [^\]]*\]|\[\w{6} done [^\]]*\]|\s", "", text)
        if stripped and self.cur_row in self.rows_after and self.t_text is None:
            self.t_text = now
            self.first_text = re.sub(r"\[[^\]]*\]", "", text).strip()[:200]

    def run(self):
        p = subprocess.Popen(self.cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, bufsize=0, env=self.env)
        self.procs.append(p)
        fds = {p.stdout.fileno(): 1, p.stderr.fileno(): 2}
        while fds:
            r, _, _ = select.select(list(fds), [], [], 0.2)
            for f in r:
                d = os.read(f, 65536)
                if d:
                    self._handle(fds[f], d)
                else:
                    del fds[f]
        p.wait()
        return self.summary(time.time(), p.returncode)

    def summary(self, t_exit=None, rc=None):
        """Timings so far; `t_exit`/`rc` stay None for a probe cut off mid-run."""

        def dl(t):
            return None if (t is None or self.t_sent is None) else round(t - self.t_sent, 2)

        return {
            "row_s": dl(self.t_row),
            "first_text_s": dl(self.t_text),
            "done_s": dl(self.t_done) if self.t_done else dl(t_exit),
            "first_text": self.first_text,
            "exit": rc,
        }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--relay", default=os.environ.get("RELAY_URL", "https://relay.arbos.life"))
    ap.add_argument("--user", default=os.environ.get("SMOKE_USER_ID", DEFAULT_USER))
    ap.add_argument("--gate-seconds", type=float, default=float(os.environ.get("SMOKE_GATE_S", "5")))
    ap.add_argument("--busy-seconds", type=int, default=90)
    ap.add_argument("--probe-delay", type=float, default=10.0)
    ap.add_argument("--log", default=None, help="append the raw arbosctl stream here")
    args = ap.parse_args()
    admin = os.environ.get("ARBOS_RELAY_ADMIN_TOKEN", "").strip()
    if not admin:
        print(json.dumps({"ok": None, "error": "ARBOS_RELAY_ADMIN_TOKEN missing"}))
        return 2
    relay = args.relay.rstrip("/")
    work = tempfile.mkdtemp(prefix="busy-reply-smoke-")
    log = open(args.log or os.path.join(work, "stream.log"), "a", buffering=1)
    devices = []  # (token, device_id), one per arbosctl client
    procs, threads, timers = [], [], {}
    report = {"ok": None, "relay": relay, "user": args.user, "gate_s": args.gate_seconds}

    def emit():
        line = json.dumps(report)
        print(line, flush=True)
        log.write(f"{time.time():.3f} report {line}\n")

    def stop_children():
        for p in procs:
            if p.poll() is None:
                p.kill()
        for t in threads:
            t.join(timeout=5)

    try:
        signal.signal(signal.SIGTERM, on_sigterm)
        cfgs = []
        for name in ("busy", "same", "other"):
            st, dev = api(relay, admin, "POST", f"/admin/users/{args.user}/devices",
                          {"name": f"busy-reply smoke {name} (deploy worker; revoked at the end)"})
            if st != 200 and st != 201:
                report["error"] = f"device mint HTTP {st}"
                emit()
                return 2
            devices.append((dev["access_token"], dev["device_id"]))
            d = os.path.join(work, name)
            os.makedirs(d)
            path = os.path.join(d, "arbosctl.json")
            with open(path, "w") as fh:
                json.dump({"relay_url": relay, "device_token": dev["access_token"],
                           "device_id": dev["device_id"]}, fh)
            os.chmod(path, 0o600)
            cfgs.append(path)
        token = devices[0][0]
        report["devices"] = [d for _, d in devices]
        _, projects = api(relay, token, "GET", "/projects")
        projects = projects if isinstance(projects, list) else (projects or {}).get("projects", [])
        if not projects:
            report["error"] = "the smoke user has no project"
            emit()
            return 2
        project = projects[0]["id"]
        report["project"] = project
        env = dict(os.environ)

        def open_chat(cfg, title):
            env["ARBOSCTL_CONFIG"] = cfg
            out = subprocess.run(["arbosctl", "chat", "open", project, "--title", title],
                                 env=env, capture_output=True, text=True, timeout=60)
            cid = out.stdout.strip().splitlines()[-1].strip() if out.stdout.strip() else ""
            if not re.fullmatch(r"[0-9A-Z]{26}", cid):
                raise RuntimeError(f"chat open failed: {out.stdout} {out.stderr}")
            return cid

        stamp = time.strftime("%Y-%m-%d %H:%M UTC", time.gmtime())
        chat1 = open_chat(cfgs[0], f"busy-reply smoke {stamp}")
        report["chat_busy"] = chat1
        busy_env = dict(env, ARBOSCTL_CONFIG=cfgs[0])
        busy_err = open(os.path.join(work, "busy.stderr"), "w+")
        busy = subprocess.Popen(
            ["arbosctl", "chat", "send", project, chat1,
             BUSY_PROMPT.format(n=args.busy_seconds, t=args.busy_seconds + 60),
             "--stream", "--timeout", str(args.busy_seconds + 240)],
            env=busy_env, stdout=subprocess.DEVNULL, stderr=busy_err)
        procs.append(busy)
        chat2 = open_chat(cfgs[2], f"busy-reply smoke (other chat) {stamp}")
        report["chat_other"] = chat2

        def dump(cfg, chat):
            out = subprocess.run(["arbosctl", "chat", "dump", project, chat],
                                 env=dict(env, ARBOSCTL_CONFIG=cfg), capture_output=True,
                                 text=True, timeout=60)
            try:
                return json.loads(out.stdout).get("messages", [])
            except json.JSONDecodeError:
                return []

        def busy_msg_id():
            busy_err.seek(0)
            m = re.search(r"\[sent message (\w{26})", busy_err.read())
            return m.group(1) if m else None

        seen_streaming = {"ids": set()}

        def long_turn_state():
            """`running` once the long turn's own row has been streaming across two polls in
            chat1 (a quick acknowledgement is also briefly `streaming` while its two pushes
            land, so one sighting is not enough), `done` when a finished non-quick row exists
            and nothing streams, `waiting` otherwise (the box may still be on an earlier turn,
            in which case the busy message got only a quick acknowledgement so far)."""
            mid = busy_msg_id()
            if not mid:
                return "waiting"
            rows = [m for m in dump(cfgs[1], chat1)
                    if m.get("role") == "assistant" and m.get("inReplyTo") == mid]
            streaming = {m.get("id") for m in rows if m.get("status") != "done"}
            finished_real = [m for m in rows if m.get("status") == "done" and m.get("decision") != "quick"]
            again = streaming & seen_streaming["ids"]
            seen_streaming["ids"] = streaming
            if again:
                return "running"
            if finished_real and not streaming:
                return "done"
            return "waiting"

        # Probe only once the long turn itself is streaming: a box still busy with an earlier
        # turn answers the busy prompt with a quick acknowledgement first (the harness must not
        # mistake that for the long turn).
        t_wait = time.time()
        state = long_turn_state()
        while state != "running" and time.time() - t_wait < 180:
            if state == "done":
                break
            time.sleep(2)
            state = long_turn_state()
        report["long_turn_started_after_s"] = round(time.time() - t_wait, 1)
        time.sleep(args.probe_delay)
        # Both probes run while the long turn is still going: the same-chat one first, the
        # new-chat one 10 s later, each under its own arbosctl config (own room replica).
        results = {}
        running_at = {}

        def probe(name, cfg, chat, text):
            running_at[name] = long_turn_state() == "running"
            env_p = dict(os.environ, ARBOSCTL_CONFIG=cfg)
            timers[name] = Timed(["arbosctl", "chat", "send", project, chat, text,
                                  "--stream", "--timeout", "150"], log, env_p, procs)
            results[name] = timers[name].run()

        # Daemon threads: a SIGTERM lands in the main thread, which must be able to exit
        # without waiting for a probe that is still streaming.
        t_same = threading.Thread(target=probe, args=("same_chat", cfgs[1], chat1, PROBE_SAME), daemon=True)
        t_other = threading.Thread(target=probe, args=("other_chat", cfgs[2], chat2, PROBE_OTHER), daemon=True)
        threads.extend([t_same, t_other])
        t_same.start()
        time.sleep(10)
        t_other.start()
        t_same.join()
        t_other.join()
        r_same, r_other = results["same_chat"], results["other_chat"]
        busy_still_running = all(running_at.values())
        report.update({
            "same_chat": r_same,
            "other_chat": r_other,
            "busy_turn_still_running_at_probes": busy_still_running,
        })
        verdicts = []
        for name, r in (("same_chat", r_same), ("other_chat", r_other)):
            ft = r.get("first_text_s")
            ok = ft is not None and ft <= args.gate_seconds and bool(r.get("first_text"))
            verdicts.append(ok)
            report[f"{name}_ok"] = ok
        report["ok"] = all(verdicts) and busy_still_running
        if not busy_still_running:
            report["note"] = "the long turn ended before the probes; the gate was not exercised"
        try:
            busy.wait(timeout=args.busy_seconds + 300)
        except subprocess.TimeoutExpired:
            busy.kill()
        # Leave the agent idle for the next run: wait for the long turn to finish.
        t_end = time.time()
        while long_turn_state() != "done" and time.time() - t_end < args.busy_seconds + 180:
            time.sleep(3)
        busy_err.close()
        emit()
        return 0 if report["ok"] else 1
    except Terminated:
        stop_children()
        for name, t in timers.items():
            report.setdefault(name, t.summary())
        report["terminated"] = ("SIGTERM while waiting for the long turn to end (verdict stands)"
                                if report["ok"] is not None else "SIGTERM before the gate verdict")
        emit()
        return 2 if report["ok"] is None else (0 if report["ok"] else 1)
    except Exception as e:  # noqa: BLE001 — one report line, whatever broke
        report["error"] = str(e)[:400]
        emit()
        return 2
    finally:
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        stop_children()
        for tok, dev_id in devices:
            try:
                api(relay, tok, "DELETE", f"/auth/devices/{dev_id}")
            except (urllib.error.URLError, urllib.error.HTTPError, OSError):
                pass
        log.close()


if __name__ == "__main__":
    sys.exit(main())
