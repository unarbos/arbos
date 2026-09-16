"""The acceptance journey (docs/acceptance-journeys.md) on the Linux rig: one fresh project, a real
challenge, follow-ups, steer/interrupt/read-only ask, leave and come back, the result on disk,
the awkward parts. Scored per step J1..J8 as pass | fail | unverified; one line per run in
journey-history.jsonl; a step failing twice in a row becomes a named bug (qal-jNN).

Needs the desktop build (ARBOS_DESKTOP_BIN, ARBOS_DESKTOP_DRIVER, Xvfb) and a model key.
"""

import json
import os
import re
import shutil
import signal
import subprocess
import sys
import time
from pathlib import Path

DESKTOP_BIN = os.environ.get("ARBOS_DESKTOP_BIN", "")
DRIVER_DIR = os.environ.get("ARBOS_DESKTOP_DRIVER", "")
STEPS = ["J1", "J2", "J3", "J4", "J5", "J6", "J7", "J8"]
# The prompt's own `status "<-ing verb> <what>"` form, written as a reply line instead of called as a tool.
STATUS_PROSE = re.compile(r'^\s*status\s*[:"“]', re.I)


def available():
    return bool(DESKTOP_BIN and Path(DESKTOP_BIN).exists() and DRIVER_DIR and (Path(DRIVER_DIR) / "arbosdriver.py").exists() and shutil.which("Xvfb"))


def read_transcript(place, agent="root"):
    for base in (Path(place) / ".arbos" / "agents" / agent, Path(place) / ".arbos" / "archive" / "agents" / agent):
        tr = base / "transcript.jsonl"
        if tr.exists():
            out = []
            for l in tr.read_text(errors="replace").splitlines():
                try:
                    out.append(json.loads(l))
                except Exception:  # noqa: BLE001
                    pass
            return out
    return []


def agents_of(place):
    names = set()
    for d in (Path(place) / ".arbos" / "agents", Path(place) / ".arbos" / "archive" / "agents"):
        if d.exists():
            names |= {p.name for p in d.iterdir() if p.is_dir()}
    return names


def kernel_pid(place):
    for p in (Path(place) / ".arbos" / "runtime" / "kernel.json", Path(place) / ".arbos" / "kernel.json"):
        if p.exists():
            try:
                return int(json.loads(p.read_text())["pid"])
            except Exception:  # noqa: BLE001
                return None
    return None


def git(place, *args):
    return subprocess.run(["git", "-c", "user.name=qa", "-c", "user.email=qa@qa", *args], cwd=place, capture_output=True, text=True).stdout.strip()


def seed_project(folder):
    """A small Python project with a failing test and no CHANGELOG: the challenge's ground."""
    folder.mkdir(parents=True, exist_ok=True)
    (folder / "shapes").mkdir()
    (folder / "shapes" / "__init__.py").write_text("")
    (folder / "shapes" / "geometry.py").write_text('def area(w, h):\n    """Area of a rectangle."""\n    return w + h  # bug: should multiply\n\n\ndef perimeter(w, h):\n    return 2 * (w + h)\n')
    (folder / "tests").mkdir()
    (folder / "tests" / "__init__.py").write_text("")
    (folder / "tests" / "test_geometry.py").write_text("import unittest\nfrom shapes.geometry import area, perimeter\n\n\nclass T(unittest.TestCase):\n    def test_area(self):\n        self.assertEqual(area(3, 4), 12)\n\n    def test_perimeter(self):\n        self.assertEqual(perimeter(3, 4), 14)\n\n\nif __name__ == '__main__':\n    unittest.main()\n")
    (folder / "README.md").write_text("# shapes\n\nRun the tests with `python3 -m unittest`.\n")
    for args in (["init", "-q", "-b", "main"], ["config", "user.name", "qa"], ["config", "user.email", "qa@qa"], ["add", "-A"], ["commit", "-q", "-m", "shapes: first cut"]):
        subprocess.run(["git", *args], cwd=folder, capture_output=True)


def tests_pass(folder):
    r = subprocess.run([sys.executable, "-m", "unittest", "-q"], cwd=folder, capture_output=True, text=True, timeout=60)
    return r.returncode == 0, (r.stderr or r.stdout).strip().splitlines()[-1:] or [""]


class Hang(Exception):
    """The window stopped answering the driver."""


class Rig:
    """Xvfb + the app on one or two folders, through the JSON driver."""

    def __init__(self, cx, folders, tag="app", reseed=True):
        self.cx = cx
        self.display = f":{9000 + os.getpid() % 900}"
        self.xvfb = subprocess.Popen(["Xvfb", self.display, "-screen", "0", "1600x1000x24", "-nolisten", "tcp"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        time.sleep(0.8)
        sys.path.insert(0, DRIVER_DIR)
        import arbosdriver  # noqa: E402

        env = dict(cx.env)
        env["DISPLAY"] = self.display
        env["PATH"] = f"{Path(cx.binary).parent}:{env.get('PATH', '')}"
        env["ARBOS_DRIVER"] = "1"
        # A session bus of our own and dunst on it: the app's OS notifications need a daemon, and
        # `dunstctl history` is the daemon's own record — the cross-check that the window did not just claim a post.
        self.dunst = None
        self.bus_env = None
        if shutil.which("dbus-daemon") and shutil.which("dunst"):
            try:
                addr = subprocess.run(["dbus-daemon", "--session", "--fork", "--print-address", "--nopidfile"], capture_output=True, text=True, timeout=10).stdout.strip()
                if addr:
                    env["DBUS_SESSION_BUS_ADDRESS"] = addr
                    self.bus_env = {"DISPLAY": self.display, "DBUS_SESSION_BUS_ADDRESS": addr, "PATH": env["PATH"], "HOME": env.get("HOME", "/tmp")}
                    self.dunst = subprocess.Popen(["dunst"], env=self.bus_env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                    time.sleep(0.5)
            except Exception as e:  # noqa: BLE001
                cx.rec.notes.setdefault("dunst_error", str(e)[:120])
        self.log = cx.rec.dir / f"{tag}.log"
        xdg = cx.scratch / "xdg"
        if reseed:
            self.app = arbosdriver.Arbos.launch(binary=DESKTOP_BIN, env=env, log=self.log, xdg=xdg, projects=[str(f) for f in folders], timeout=120)
        else:
            # The app's own state.toml decides what comes back — that is the point of "leave and come back".
            env["XDG_CONFIG_HOME"] = str(xdg)
            env["XDG_DATA_HOME"] = str(xdg / "data")
            self.app = arbosdriver.Arbos.launch(binary=DESKTOP_BIN, env=env, log=self.log, xdg=None, timeout=120)
        self.pulses = []
        self.pulse("launch")

    def state(self):
        return self.app.state()

    PULSE_LIMIT_S = 1.0

    def pulse(self, after):
        """Liveness: the window must answer the cheapest driver call within a second after any action that opens
        or changes something (a sheet, a dialog, a tab, a send, a Stop). A hang there reaches Jacob instantly
        and reads as the app being broken — and no state assertion would ever notice it, because a hung window
        answers nothing. Returns the round-trip in ms, or raises Hang."""
        sock = self.app._sock
        old = sock.gettimeout()
        t0 = time.time()
        try:
            sock.settimeout(self.PULSE_LIMIT_S)
            self.app.hello()
        except Exception as e:  # noqa: BLE001 — a timeout, a closed socket: the window is not answering
            ms = round((time.time() - t0) * 1000)
            self.cx.rec.log(f"HANG after {after}: no driver answer in {ms} ms ({type(e).__name__})")
            try:
                shot = self.cx.rec.dir / f"hang-{int(t0)}.xwd"
                subprocess.run(["xwd", "-root", "-display", self.display, "-out", str(shot)], capture_output=True, timeout=5)
            except Exception:  # noqa: BLE001
                pass
            raise Hang(f"the window stopped answering after {after} ({ms} ms, {type(e).__name__})") from e
        finally:
            try:
                sock.settimeout(old)
            except Exception:  # noqa: BLE001
                pass
        ms = round((time.time() - t0) * 1000)
        self.pulses.append((after, ms))
        return ms

    def active_path(self):
        st = self.state()
        ix = st.get("active_project")
        try:
            return str(st["projects"][ix].get("path", "")).rstrip("/")
        except Exception:  # noqa: BLE001
            return None

    def project(self, folder):
        for p in self.state()["projects"]:
            if str(p.get("path", "")).rstrip("/") == str(folder).rstrip("/"):
                return p
        return None

    def root_chat(self, folder):
        p = self.project(folder)
        if not p:
            return None
        for c in p["sessions"]:
            if c.get("agent_session") == "root":
                return c
        return p["sessions"][0] if p["sessions"] else None

    def focus(self, folder):
        for i, p in enumerate(self.state()["projects"]):
            if str(p.get("path", "")).rstrip("/") == str(folder).rstrip("/"):
                try:
                    self.app.click(f"tab-{i}")
                except Exception:  # noqa: BLE001
                    pass
                self.pulse(f"click tab-{i}")
                st = self.app.wait_state(lambda s: s.get("active_project") == i, timeout=10, what="active project")
                return st.get("active_project") == i
        return False

    def busy(self, folder):
        c = self.root_chat(folder)
        return bool(c and (c.get("streaming") or c.get("turn_open")))

    def wait_busy(self, folder, timeout):
        end = time.time() + timeout
        while time.time() < end:
            if self.busy(folder):
                return True
            time.sleep(0.5)
        return False

    def wait_idle(self, folder, timeout):
        end = time.time() + timeout
        while time.time() < end:
            if not self.busy(folder):
                return True
            time.sleep(1)
        return False

    def send(self, text):
        self.app.wait_element("composer-field", reachable=True)
        self.app.click("composer-field")
        self.app.type(text + "\n")
        self.pulse("send")

    def dunst_history(self):
        """The daemon's record, oldest first, or None without a daemon. dunst files a notification only once it is
        closed (so close the popups first) and keeps twenty — hence a nonce in whatever we look for."""
        if not self.bus_env or not shutil.which("dunstctl"):
            return None
        try:
            subprocess.run(["dunstctl", "close-all"], capture_output=True, timeout=5, env=self.bus_env)
            raw = subprocess.run(["dunstctl", "history"], capture_output=True, text=True, timeout=5, env=self.bus_env).stdout
            data = json.loads(raw).get("data", [[]])
            entries = data[0] if data else []
            return [f"{e.get('summary', {}).get('data', '')} | {e.get('body', {}).get('data', '')}" for e in reversed(entries)]
        except Exception:  # noqa: BLE001
            return None

    def leaves(self):
        return {str(e.get("path", "")).split(".")[-1] for e in self.app.elements("*")}

    def close(self, folders=(), stop_kernels=True):
        try:
            self.app.close()
        except Exception:  # noqa: BLE001
            pass
        time.sleep(1)
        if stop_kernels:
            for f in list(folders) + [self.cx.scratch / "home" / ".arbos"]:
                pid = kernel_pid(f)
                if pid:
                    try:
                        os.kill(pid, signal.SIGINT)
                    except ProcessLookupError:
                        pass
            time.sleep(2)
        if self.dunst:
            self.dunst.terminate()
        self.xvfb.terminate()


NOTIFY_KEYS = ("unseen", "unread", "notifications", "badge", "unseen_count", "unread_count", "away")
STORE = Path(os.environ.get("ARBOS_QA_STORE_ROOT", "/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983"))


def notify_surface(project, chat):
    """The unseen/notification state the driver exposes for a project or its chat, or None when it exposes none yet.
    #329: per project `tab_dot` (the badge exactly as drawn) and `unseen` (summed over its chats)."""
    if project and "tab_dot" in project:
        return {"tab_dot": project.get("tab_dot"), "unseen": project.get("unseen")}
    for holder in (chat or {}, project or {}):
        for k in NOTIFY_KEYS:
            if k in holder:
                return {k: holder[k]}
    return None


def phone_j8c(max_age_h=24):
    """J8c (dropped connection) from the phone loop's history: the newest run within max_age_h that scored it."""
    hist = STORE / "internal" / "mobile-journey-history.jsonl"
    if not hist.exists():
        return None
    rows = []
    for l in hist.read_text(errors="replace").splitlines():
        try:
            rows.append(json.loads(l))
        except Exception:  # noqa: BLE001
            pass
    now = time.time()
    for r in reversed(rows):
        v = (r.get("steps") or {}).get("J8c")
        if not v:
            continue
        try:
            ts = time.mktime(time.strptime(r["ts"], "%Y-%m-%dT%H:%M:%SZ")) - time.timezone
        except Exception:  # noqa: BLE001
            ts = now
        if now - ts > max_age_h * 3600:
            return {"verdict": "unverified", "why": f"phone loop's last J8c is older than {max_age_h} h ({r.get('ts')})"}
        return {"verdict": v, "ts": r.get("ts"), "target": r.get("target"), "evidence": r.get("evidence")}
    return None


def register(scenario, registry, transcript, now_ms, branch):
    def reg(name):
        def deco(fn):
            scenario(name, needs_model=True, tags=("journey", "desktop", "headline"))(fn)
            registry[name]["branch"] = branch
            return fn

        return deco

    @scenario("journey-j8a-headless", needs_model=True, tags=("journey", "adversarial"))
    def j8a_headless(cx):
        """J8a without the desktop: a bash command with a side effect (`echo ran-<tag> >> side-effects.log; sleep 30`), SIGKILL the kernel during the sleep, restart it. The continued turn must not run the command again — side-effects.log holds the tag once (qal-j02 / #316)."""
        tag = f"H{now_ms() % 100000}"
        side = cx.place / "side-effects.log"
        k = cx.kernel()
        cx.rec.expect(k.start(), "kernel-start", "kernel did not come up")
        c = k.attach()
        c.wait(lambda f: f.get("type") == "snapshot", 5)
        c.user("root", f"Run exactly this with bash and nothing else first: `echo ran-{tag} >> side-effects.log; sleep 30; echo waited`. Then say done.")
        cx.rec.expect(c.wait_turn("root", "running", 15) is not None, "turn-never-started", "no running turn")
        end = time.time() + 60
        while time.time() < end and not side.exists():
            time.sleep(0.5)
        cx.rec.expect(side.exists(), "side-effect-never-ran", "the command's first run left no side effect within 60 s")
        time.sleep(3)
        k.kill()
        time.sleep(0.5)
        k2 = cx.kernel(tag="kernel-restart")
        cx.rec.expect(k2.start(), "kernel-restart", "kernel did not restart on a mid-turn folder")
        c2 = k2.attach()
        c2.wait(lambda f: f.get("type") == "snapshot", 5)
        c2.wait_turn("root", "idle", 150)
        time.sleep(2)
        runs = side.read_text().count(f"ran-{tag}") if side.exists() else 0
        evs = read_transcript(cx.place)
        interrupted_tool = [e for e in evs if e.get("kind") == "tool" and ("interrupted" in json.dumps(e).lower() or "kernel" in str(e.get("result", e.get("output", ""))).lower())]
        cx.rec.notes.update({"side_effect_runs": runs, "tool_lines": sum(1 for e in evs if e.get("kind") == "tool"), "interrupted_tool_record": len(interrupted_tool)})
        cx.rec.expect(runs == 1, "side-effect-doubled" if runs > 1 else "side-effect-missing", f"side-effects.log holds the tag {runs} time(s) across the kernel restart (expected once; qal-j02)")

    @reg("journey-linux")
    def journey(cx):
        """The acceptance journey J1..J8 on the Linux rig (docs/acceptance-journeys.md): a fresh project, a real challenge with a failing test, follow-ups, steer/interrupt/read-only ask, leave and come back, the result on disk, kernel restart and a second project. Scored per step."""
        steps = {s: ("unverified", "not reached") for s in STEPS}
        ev = {}

        def mark(step, verdict, why):
            steps[step] = (verdict, why)
            cx.rec.log(f"{step}: {verdict} — {why}")


        def finish():
            passed = sum(1 for v, _ in steps.values() if v == "pass")
            unv = [s for s, (v, _) in steps.items() if v == "unverified"]
            failed = [s for s, (v, _) in steps.items() if v == "fail"]
            cx.rec.notes.update({"steps": {s: {"verdict": v, "why": w} for s, (v, w) in steps.items()}, "score": f"{passed}/8", "unverified": unv, "failed": failed, "evidence": ev})
            hist = Path(__file__).resolve().parent / "journey-history.jsonl"
            line = {"ts": now_ms(), "rollout": cx.rec.name, "branch": branch, "score": passed, "unverified": unv, "failed": failed, "steps": {s: v for s, (v, _) in steps.items()}}
            prev = []
            if hist.exists():
                prev = [json.loads(l) for l in hist.read_text().splitlines() if l.strip()]
            with open(hist, "a") as f:
                f.write(json.dumps(line) + "\n")
            print(f"    journey: {passed}/8 pass, {len(unv)} unverified, {len(failed)} fail — " + " ".join(f"{s}{'✓' if v == 'pass' else ('?' if v == 'unverified' else '✗')}" for s, (v, _) in steps.items()))
            # A step failing twice in a row is a bug with a name.
            for s in failed:
                cx.rec.broke(f"journey-{s}", steps[s][1], "docs/acceptance-journeys.md")
                if prev and prev[-1].get("steps", {}).get(s) == "fail":
                    cx.rec.broke(f"journey-{s}-twice", f"{s} failed in two consecutive runs ({prev[-1].get('rollout')}, {cx.rec.name}): {steps[s][1]}", "file as qal-jNN")

        if not available():
            cx.rec.notes["skipped"] = "desktop binary/driver/Xvfb missing"
            for s in STEPS:
                steps[s] = ("unverified", "no desktop rig")
            finish()
            return
        folder = cx.scratch / "shapes-project"
        seed_project(folder)
        second = cx.scratch / "second-project"
        tag = f"J{now_ms() % 100000}"

        # ── J1: create a project from nothing ──────────────────────────────
        rig = Rig(cx, [folder])
        try:
            time.sleep(3)
            chat = rig.root_chat(folder)
            got_tab = chat is not None
            if got_tab:
                rig.focus(folder)
                rig.wait_idle(folder, 90)
                chat = rig.root_chat(folder)
                items = chat.get("items", [])
                agent_lines = [i for i in items if i.get("kind") == "agent" and str(i.get("text", "")).strip() and not STATUS_PROSE.match(str(i.get("text", "")).strip())]
                failed_notices = [i for i in items if i.get("kind") == "notice" and (i.get("failed") or "403" in str(i.get("text")) or "Policy" in str(i.get("text")))]
                ev["J1"] = {"items": len(items), "agent_lines": len(agent_lines), "failed_notices": [str(n.get("text"))[:100] for n in failed_notices], "arbos_dir": (folder / ".arbos").exists()}
                if failed_notices:
                    mark("J1", "fail", f"a failed notice greets the new user: {ev['J1']['failed_notices'][:1]}")
                elif agent_lines and (folder / ".arbos").exists():
                    mark("J1", "pass", f"tab present, greeting shown ({agent_lines[0].get('text', '')[:60]!r}), .arbos/ created")
                else:
                    mark("J1", "fail", f"no greeting from the kickoff turn (items {len(items)})")
            else:
                mark("J1", "fail", "no session for the folder after launch")

            # ── J2: the challenge ──────────────────────────────────────────
            challenge = ("Fix the failing test in this project (python3 -m unittest shows it), add a CHANGELOG.md entry describing the fix, "
                         "run the tests to prove they pass, and commit the work on a new branch (not main). Tell me the branch name and the last line of the test output.")
            rig.send(challenge)
            busy = rig.wait_busy(folder, 30)
            workers = set()
            spawn_cards = 0
            end = time.time() + 150
            while time.time() < end:
                workers = agents_of(folder) - {"root"}
                c = rig.root_chat(folder) or {}
                spawn_cards = sum(1 for i in c.get("items", []) if i.get("kind") == "tool" and "spawn" in json.dumps(i).lower())
                if workers or spawn_cards:
                    break
                if not rig.busy(folder) and time.time() > end - 120:
                    break
                time.sleep(2)
            ev["J2"] = {"went_busy": busy, "workers": sorted(workers), "spawn_cards": spawn_cards}
            if not busy:
                mark("J2", "fail", "the chat never went busy after the challenge")
            elif workers or spawn_cards:
                mark("J2", "pass", f"chat busy; workers {sorted(workers)} / {spawn_cards} spawn card(s)")
            else:
                mark("J2", "unverified", "the coordinator worked without a visible worker; the journey continues")

            # ── J4a + J5 steer/read-only mid-flight ────────────────────────
            # "Mid-flight" means the work is running — root's turn open, or a live worker still on its turn
            # (a coordinator root delegates and ends its own turn within seconds; the worker is the work).
            def work_in_flight():
                if rig.busy(folder):
                    return True
                for w in agents_of(folder) - {"root"}:
                    if (Path(folder) / ".arbos" / "agents" / w).exists():
                        tr = read_transcript(folder, w)
                        if tr and tr[-1].get("kind") not in ("turn_complete", "interrupted"):
                            return True
                return False

            midflight = {}
            if work_in_flight():
                rig.send(f"Also add a line to CHANGELOG.md saying who asked for this: QA-{tag}.")
                midflight["followup_sent_at"] = time.time()
                end = time.time() + 120
                while time.time() < end:
                    if any(e.get("kind") == "user" and f"QA-{tag}" in e.get("text", "") for e in read_transcript(folder)):
                        midflight["followup_reached_root_s"] = round(time.time() - midflight["followup_sent_at"], 1)
                        break
                    time.sleep(0.5)
                time.sleep(4)
                if work_in_flight():
                    rig.send("Use British spelling in the CHANGELOG.")
                    midflight["steer_sent"] = True
                    time.sleep(4)
                if work_in_flight():
                    rig.send("Quick read-only question while you work: roughly what time is it now, in one line?")
                    midflight["readonly_sent"] = True
            else:
                midflight["note"] = "the turn had already ended before the mid-flight lines"

            # ── J3: watch it work honestly ─────────────────────────────────
            idle = rig.wait_idle(folder, 600)
            # workers may still be running after root's turn; let them end
            end = time.time() + 240
            while time.time() < end:
                open_ = [w for w in agents_of(folder) - {"root"} if (read_transcript(folder, w) or [{}])[-1].get("kind") not in ("turn_complete", "interrupted")]
                if not open_ and not rig.busy(folder):
                    break
                time.sleep(3)
            # Quiet means: the chat reads idle AND root's transcript has ended its turn. A wake that sits
            # unserviced while the chat reads idle for 45 s is a dangling wake (the qa-032 class).
            open_ = locals().get("open_", [])
            quiet_since = None
            end = time.time() + 120
            while time.time() < end:
                evs = read_transcript(folder)
                trailing = [e.get("kind") for e in evs if e.get("kind") != "nudge"]
                if not rig.busy(folder) and trailing and trailing[-1] in ("turn_complete", "interrupted"):
                    quiet_since = quiet_since or time.time()
                    if time.time() - quiet_since >= 5:
                        break
                else:
                    quiet_since = None
                time.sleep(1)
            evs = read_transcript(folder)
            ks = [e.get("kind") for e in evs]
            trailing = [k for k in ks if k != "nudge"]
            items = (rig.root_chat(folder) or {}).get("items", [])
            texts = [str(i.get("text", "")).strip() for i in items if i.get("kind") == "agent"]
            dup = [t[:60] for a, b in zip(texts, texts[1:]) for t in [a] if a and a == b]
            status_prose = [t[:60] for t in texts if STATUS_PROSE.match(t)]
            honest = (not rig.busy(folder)) and bool(trailing) and trailing[-1] in ("turn_complete", "interrupted")
            ev["J3"] = {"idle_within_600s": idle, "duplicate_agent_items": dup[:3], "status_drawn_as_reply": status_prose[:3], "transcript_ends": trailing[-1] if trailing else None, "workers_open": open_}
            if not idle:
                mark("J3", "fail", "the chat was still busy after 600 s")
            elif dup:
                mark("J3", "fail", f"the same assistant text drawn twice in a row: {dup[:1]}")
            elif not honest:
                mark("J3", "fail", f"the chat reads idle but the transcript ends in {trailing[-1] if trailing else None} (dangling wake)")
            elif status_prose:
                mark("J3", "fail", f"the status line is drawn as a reply bubble ({len(status_prose)}x), e.g. {status_prose[0]!r}")
            else:
                mark("J3", "pass", "workers finished, no duplicate lines, status shown as status, idle means the turn ended")

            # ── J4: follow-ups (mid-flight judged from the transcript; after-finish now) ───
            j4 = {}
            if "followup_sent_at" in midflight:
                i_task = next((i for i, e in enumerate(evs) if e.get("kind") == "user" and "Fix the failing test" in e.get("text", "")), None)
                i_follow = next((i for i, e in enumerate(evs) if e.get("kind") == "user" and f"QA-{tag}" in e.get("text", "")), None)
                task_tc = next((i for i, k in enumerate(ks) if k == "turn_complete" and i > (i_task if i_task is not None else 10**9)), None)
                same_turn = i_follow is not None and task_tc is not None and i_follow < task_tc
                follow_tc = next((i for i, k in enumerate(ks) if k == "turn_complete" and i > (i_follow if i_follow is not None else 10**9)), len(evs))
                turn_tools = [e.get("name") for e in evs[(i_follow or 0):follow_tc] if e.get("kind") == "tool"]
                relayed = "say" in turn_tools and "spawn" not in turn_tools
                workers_now = agents_of(folder) - {"root"}
                j4["midflight"] = {"index": i_follow, "reached_root_s": midflight.get("followup_reached_root_s"), "task_turn_complete": task_tc, "same_turn": same_turn, "relayed_to_worker": relayed, "turn_tools": turn_tools, "workers": sorted(workers_now), "taken": same_turn or (relayed and len(workers_now) == len(ev.get("J2", {}).get("workers", [])))}
            rig.send("Summarise what you changed in two lines.")
            after_busy = rig.wait_busy(folder, 30)
            rig.wait_idle(folder, 180)
            evs2 = read_transcript(folder)
            answered = any(e.get("kind") == "assistant" and e.get("text", "").strip() for e in evs2[len(evs):])
            j4["after"] = {"started": after_busy, "answered": answered}
            ev["J4"] = j4
            if "midflight" not in j4:
                mark("J4", "unverified" if answered else "fail", "mid-flight half not checkable (turn ended first); after-finish follow-up " + ("answered" if answered else "unanswered"))
            elif not j4["midflight"]["taken"]:
                mark("J4", "fail", f"the mid-flight follow-up was not taken into the running work (no same-turn line, no relay to the live worker, or a second worker): {j4['midflight']}")
            elif not answered:
                mark("J4", "fail", "the follow-up after the finish got no answer")
            else:
                mark("J4", "pass", ("mid-flight line taken in the same turn" if j4["midflight"]["same_turn"] else "mid-flight line relayed to the running worker, no second worker") + "; the later one started its own turn and was answered")

            # ── J5: steer / interrupt / read-only ──────────────────────────
            j5 = {}
            if midflight.get("steer_sent"):
                i_steer = next((i for i, e in enumerate(evs) if e.get("kind") == "user" and "British" in e.get("text", "")), None)
                i_tc = next((i for i, k in enumerate(ks) if k == "turn_complete" and i > (i_steer or 10**9)), None)
                j5["steer_in_turn"] = i_steer is not None and i_tc is not None
            if midflight.get("readonly_sent"):
                j5["readonly_answered"] = any(e.get("kind") == "assistant" and re.search(r"\d{1,2}[:.]\d{2}|o'clock|time", e.get("text", ""), re.I) for e in evs)
            # Interrupt: start a slow turn, press Stop.
            rig.send("Run `sleep 40; echo slept` with bash and then say done.")
            if rig.wait_busy(folder, 30):
                time.sleep(6)
                stopped_at = time.time()
                try:
                    rig.app.click("composer-stop")
                    rig.pulse("Stop")
                    j5["stop_clicked"] = True
                except Exception as e:  # noqa: BLE001
                    j5["stop_error"] = str(e)[:100]
                    try:
                        rig.app.key("Escape")
                    except Exception:  # noqa: BLE001
                        pass
                ended = rig.wait_idle(folder, 20)
                evs3 = read_transcript(folder)
                interrupted = any(e.get("kind") == "interrupted" for e in evs3[len(evs2):])
                j5["interrupt"] = {"ended_within_20s": ended, "interrupted_line": interrupted, "took_s": round(time.time() - stopped_at, 1)}
                rig.send("Reply with the single word AFTERSTOP.")
                rig.wait_busy(folder, 20)
                rig.wait_idle(folder, 120)
                j5["prompt_after_stop_answered"] = any("AFTERSTOP" in e.get("text", "") for e in read_transcript(folder)[len(evs3):] if e.get("kind") == "assistant")
            else:
                j5["interrupt"] = "the slow turn never started"
            ev["J5"] = j5
            parts_fail = []
            parts_unv = []
            if "steer_in_turn" in j5 and not j5["steer_in_turn"]:
                parts_fail.append("steer not taken in the running turn")
            if "steer_in_turn" not in j5:
                parts_unv.append("steer (turn ended first)")
            if "readonly_answered" in j5 and not j5["readonly_answered"]:
                parts_fail.append("read-only question unanswered")
            if "readonly_sent" not in midflight:
                parts_unv.append("read-only ask (turn ended first)")
            it = j5.get("interrupt")
            if isinstance(it, dict):
                if not it["ended_within_20s"]:
                    parts_fail.append("Stop did not end the turn within 20 s")
                if not j5.get("prompt_after_stop_answered"):
                    parts_fail.append("no answer after Stop")
            else:
                parts_unv.append("interrupt (slow turn never started)")
            if parts_fail:
                mark("J5", "fail", "; ".join(parts_fail))
            elif parts_unv:
                mark("J5", "unverified", "; ".join(parts_unv))
            else:
                mark("J5", "pass", "steer taken, read-only question answered, Stop ended the turn and the next prompt ran")

            # ── J6: leave and come back ────────────────────────────────────
            before_items = len((rig.root_chat(folder) or {}).get("items", []))
            before_sessions = len((rig.project(folder) or {}).get("sessions", []))
            notes_before = (folder / ".arbos" / "notes.md").read_text(errors="replace") if (folder / ".arbos" / "notes.md").exists() else ""
            rig.focus(folder)
            # Something arrives while the user is away: a reply that lands after the window is closed.
            rig.send(f"Run `sleep 15` with bash, then reply with the single word LATER-{tag}.")
            rig.wait_busy(folder, 20)
            rig.close(folders=[], stop_kernels=False)  # quit the app only; the kernel keeps serving
            end = time.time() + 90
            while time.time() < end and not any(e.get("kind") == "assistant" and f"LATER-{tag}" in e.get("text", "") for e in read_transcript(folder)):
                time.sleep(2)
            late_reply_landed = any(e.get("kind") == "assistant" and f"LATER-{tag}" in e.get("text", "") for e in read_transcript(folder))
            time.sleep(2)
            rig = Rig(cx, [folder], tag="app-relaunch", reseed=False)
            time.sleep(4)
            p = rig.project(folder)
            # The app puts the home tab in front at every launch by design (workspace.rs); the user
            # clicks their project tab. Recorded, not scored — Jacob decides whether that is what "come back" means.
            active = rig.active_path()
            # Read the notification surface BEFORE clicking the tab (clicking marks things seen).
            st_open = rig.state()
            notif_open = st_open.get("notifications") or {}
            touched_before = notif_open.get("touched")  # #336: "looked at" needs a press, a key, or a return after the first 5 s
            posted_on_relaunch = len(notif_open.get("posted", []))  # expected 0: no OS notification for a reply that landed while the app was closed; the badge carries it
            notify_before_click = notify_surface(p, rig.root_chat(folder))
            rig.focus(folder)
            time.sleep(1)
            notify_after_click = notify_surface(rig.project(folder), rig.root_chat(folder))
            touched_after = (rig.state().get("notifications") or {}).get("touched")
            chat = rig.root_chat(folder)
            after_items = len((chat or {}).get("items", []))
            after_sessions = len((p or {}).get("sessions", [])) if p else 0
            leaves = rig.leaves()
            worker_rows = [l for l in leaves if l.startswith(("panel-agent", "panel-archived"))]
            notes_after = (folder / ".arbos" / "notes.md").read_text(errors="replace") if (folder / ".arbos" / "notes.md").exists() else ""
            ev["J6"] = {"tab_back": p is not None, "landed_on": active, "landed_on_project": active == str(folder).rstrip("/"), "late_reply_landed_while_closed": late_reply_landed, "notify_before_click": notify_before_click, "notify_after_click": notify_after_click, "touched_before_click": touched_before, "touched_after_click": touched_after, "os_posts_on_relaunch": posted_on_relaunch, "items_before": before_items, "items_after": after_items, "sessions_before": before_sessions, "sessions_after": after_sessions, "worker_rows": len(worker_rows), "notes_unchanged": notes_before == notes_after, "notifications": "unverified: the driver exposes no unseen count"}
            if p is None:
                mark("J6", "fail", "the project tab did not come back after relaunch")
            elif after_items < before_items:
                mark("J6", "fail", f"transcript shorter after relaunch: {before_items} -> {after_items} items")
            elif not (worker_rows or agents_of(folder) - {"root"} == set()):
                mark("J6", "fail", "workers existed but no worker/archived row is shown after relaunch")
            elif late_reply_landed and not any(f"LATER-{tag}" in str(i.get("text", "")) for i in (chat or {}).get("items", [])):
                mark("J6", "fail", "a reply that arrived while the window was closed is not in the transcript after reopening")
            elif notify_before_click is None:
                mark("J6", "unverified", "tab, transcript and workers back" + ("" if ev["J6"]["landed_on_project"] else " (the app landed on the home tab, not the project)") + "; the driver exposes no notification state yet, so the unseen reply cannot be checked")
            else:
                # The driver exposes notification state: a reply arrived while the user was away, so the project must show it unseen, and clicking the tab must clear it.
                def seen_count(surface):
                    if not surface:
                        return 0
                    if "tab_dot" in surface:
                        return int(surface.get("unseen") or 0) or int(bool(surface.get("tab_dot")))
                    v = next(iter(surface.values()))
                    if isinstance(v, bool):
                        return int(v)
                    if isinstance(v, (int, float)):
                        return int(v)
                    if isinstance(v, (list, dict)):
                        return len(v)
                    return 1 if v else 0
                if touched_before is True:
                    mark("J6", "fail", "the window counted itself as touched before the user pressed or typed anything after the relaunch (the qal-j03 race)")
                elif late_reply_landed and seen_count(notify_before_click) == 0:
                    mark("J6", "fail", f"a reply arrived while the window was closed but nothing is shown unseen on reopening ({notify_before_click}, touched={touched_before})")
                elif posted_on_relaunch:
                    mark("J6", "fail", f"{posted_on_relaunch} OS notification(s) posted on relaunch for a reply that landed while the app was closed — the badge should carry it, a notification could not have reached a closed app")
                elif touched_before is False and touched_after is not True:
                    mark("J6", "fail", "clicking the project tab did not count as touching the window")
                elif seen_count(notify_after_click) > 0 and seen_count(notify_before_click) > 0:
                    mark("J6", "fail", f"the unseen mark did not clear when the user opened the tab ({notify_after_click})")
                else:
                    mark("J6", "pass", f"tab, transcript, workers back; unseen reply shown {notify_before_click} before any touch" + (f" (touched={touched_before})" if touched_before is not None else "") + ", no OS post on relaunch, cleared on opening the tab" + ("" if ev["J6"]["landed_on_project"] else " (landed on the home tab)"))

            # ── J6b: notified while away in another tab (the OS notification, checked against the daemon) ──
            j6b = {}
            st0 = rig.state()
            if "notifications" in st0 and (rig.project(folder) or {}).get("tab_dot") is not None:
                posted_before = len((st0.get("notifications") or {}).get("posted", []))
                nonce = f"away{now_ms() % 100000}"
                rig.focus(folder)
                rig.send(f"Run `sleep 8` with bash, then reply with exactly: notification check {nonce}.")
                rig.wait_busy(folder, 20)
                try:
                    rig.app.click("tab-0")  # the home tab in front; the project is now "away"
                except Exception:  # noqa: BLE001
                    pass
                end = time.time() + 90
                while time.time() < end and not any(e.get("kind") == "assistant" and nonce in e.get("text", "") for e in read_transcript(folder)):
                    time.sleep(2)
                time.sleep(2)
                st1 = rig.state()
                pr = rig.project(folder) or {}
                posted = (st1.get("notifications") or {}).get("posted", [])[posted_before:]
                hit = next((n for n in posted if nonce in ((n.get("body") or "") + (n.get("title") or "")).lower()), None)
                history = rig.dunst_history()
                daemon = None if history is None else any(nonce in e.lower() for e in history)
                rig.focus(folder)
                time.sleep(1)
                pr2 = rig.project(folder) or {}
                j6b = {"nonce": nonce, "unseen_away": pr.get("unseen"), "tab_dot_away": pr.get("tab_dot"), "posted_new": len(posted), "posted_hit": bool(hit), "post_error": (hit or {}).get("error"), "daemon_has_it": daemon, "daemon_entries": None if history is None else len(history), "unseen_after_click": pr2.get("unseen"), "tab_dot_after_click": pr2.get("tab_dot"), "notifier": (st1.get("notifications") or {}).get("notifier")}
                ev["J6"]["away"] = j6b
                problems = []
                if not (pr.get("unseen") or 0) >= 1 or not pr.get("tab_dot"):
                    problems.append(f"no badge while away (unseen={pr.get('unseen')}, tab_dot={pr.get('tab_dot')})")
                if not hit:
                    problems.append(f"the window posted no OS notification carrying the nonce ({len(posted)} new post(s))")
                elif daemon is False:
                    problems.append("the window claims a post the notification daemon never received (dunstctl history)")
                if pr2.get("tab_dot") or (pr2.get("unseen") or 0) > 0:
                    problems.append("the badge did not clear on opening the tab")
                if problems and steps["J6"][0] != "fail":
                    mark("J6", "fail", "away-tab notification: " + "; ".join(problems))
                elif not problems and steps["J6"][0] == "pass":
                    mark("J6", "pass", steps["J6"][1] + "; away-tab reply: badge + unseen, OS notification posted" + (" and confirmed by the daemon" if daemon else " (no daemon record to check)") + ", cleared on click")
            else:
                ev["J6"]["away"] = "unverified: the driver has no notifications/tab_dot surface (pre-#329 build)"
                if steps["J6"][0] == "pass":
                    mark("J6", "unverified", steps["J6"][1] + "; the away-tab OS notification is not checkable on this build (pre-#329)")

            # ── J7: the result on disk ─────────────────────────────────────
            ok, last = tests_pass(folder)
            changelog = (folder / "CHANGELOG.md").read_text(errors="replace") if (folder / "CHANGELOG.md").exists() else ""
            branches = [b.strip("* ").strip() for b in git(folder, "branch", "--list").splitlines() if b.strip()]
            fix_branches = [b for b in branches if b and b != "main"]
            ahead = {b: git(folder, "rev-list", "--count", f"main..{b}") for b in fix_branches}
            main_moved = git(folder, "rev-list", "--count", "main") != "1"
            ev["J7"] = {"tests_pass": ok, "last_test_line": last, "changelog": bool(changelog), "changelog_has_qa": f"QA-{tag}" in changelog, "branches": branches, "ahead_of_main": ahead, "main_moved": main_moved}
            problems = []
            if not ok:
                problems.append(f"tests still fail: {last}")
            if not changelog:
                problems.append("no CHANGELOG.md")
            if not any(int(v or 0) > 0 for v in ahead.values()):
                problems.append(f"no branch with a commit ahead of main ({branches})")
            if main_moved:
                problems.append("main was committed to")
            mark("J7", "fail" if problems else "pass", "; ".join(problems) or f"tests pass, CHANGELOG written{' (QA line taken)' if ev['J7']['changelog_has_qa'] else ''}, commit on {list(ahead)}")

            # ── J8: the awkward parts ──────────────────────────────────────
            j8 = {}
            rig.focus(folder)
            side = folder / "side-effects.log"
            # A command with a side effect, then a long wait: killing the kernel during the wait shows whether the
            # continued turn re-runs the in-flight tool (the features agent's known wrinkle — no record of an
            # in-flight tool, so the model runs it again; harmless for reads, a real bug for this).
            rig.send(f"Run exactly this with bash and nothing else first: `echo ran-{tag} >> side-effects.log; sleep 30; echo waited`. Then say done.")
            if rig.wait_busy(folder, 30):
                end = time.time() + 25
                while time.time() < end and not side.exists():
                    time.sleep(0.5)
                time.sleep(3)
                pid = kernel_pid(folder)
                if pid:
                    os.kill(pid, signal.SIGKILL)
                time.sleep(2)
                rig.send(f"After the restart, reply with the single word BACK-{tag}.")
                # The continued turn may finish (or re-run) its command before this line's turn: poll for the
                # answer itself, never infer it from an idle moment.
                end = time.time() + 180
                while time.time() < end:
                    evs8 = read_transcript(folder)
                    if any(e.get("kind") == "assistant" and f"BACK-{tag}" in e.get("text", "") for e in evs8):
                        break
                    time.sleep(2)
                evs8 = read_transcript(folder)
                backs = [e for e in evs8 if e.get("kind") == "user" and f"BACK-{tag}" in e.get("text", "")]
                answered = any(e.get("kind") == "assistant" and f"BACK-{tag}" in e.get("text", "") for e in evs8)
                notices = [str(i.get("text"))[:80] for i in (rig.root_chat(folder) or {}).get("items", []) if i.get("kind") == "notice" and i.get("failed")]
                # Give the continued turn time to (re)run the command before counting the side effect.
                rig.wait_idle(folder, 90)
                runs = side.read_text().count(f"ran-{tag}") if side.exists() else 0
                j8["restart"] = {"pid_before": pid, "pid_after": kernel_pid(folder), "line_count": len(backs), "answered": answered, "failed_notices": notices[-2:], "side_effect_runs": runs}
            else:
                j8["restart"] = "the slow turn never started"
            # Second project at the same time.
            second.mkdir(exist_ok=True)
            try:
                rig.app.click("new-tab")
                rig.pulse("new-tab (the opener sheet)")
                time.sleep(1)
                # The opener lists recent places; type-to-open is not driven here, so open via the CLI arg on relaunch instead.
                rig.app.key("Escape")
            except Exception:  # noqa: BLE001
                pass
            rig.close(folders=[], stop_kernels=False)
            rig = Rig(cx, [folder, second], tag="app-two")
            time.sleep(4)
            if rig.focus(second):
                rig.send(f"Reply with the single word SECOND-{tag}.")
                rig.wait_busy(second, 30)
                rig.wait_idle(second, 120)
            time.sleep(2)
            in_second = [e for e in read_transcript(second) if e.get("kind") == "user" and f"SECOND-{tag}" in e.get("text", "")]
            in_first = [e for e in read_transcript(folder) if e.get("kind") == "user" and f"SECOND-{tag}" in e.get("text", "")]
            in_home = [e for e in read_transcript(cx.scratch / "home" / ".arbos") if f"{tag}" in e.get("text", "")]
            j8["second_project"] = {"in_second": len(in_second), "in_first": len(in_first), "in_home_store": len(in_home)}
            phone = phone_j8c()
            j8["dropped_connection"] = phone or {"verdict": "unverified", "why": "no J8c in the phone loop's history (internal/mobile-journey-history.jsonl)"}
            ev["J8"] = j8
            problems = []
            r = j8.get("restart")
            if isinstance(r, dict):
                if r["line_count"] != 1:
                    problems.append(f"the line after the restart reached the transcript {r['line_count']} time(s)")
                if not r["answered"]:
                    problems.append("the line after the restart was not answered")
                if r["pid_after"] in (None, r["pid_before"]):
                    problems.append("the kernel was not respawned")
                if len(r["failed_notices"]) > 1:
                    problems.append(f"more than one failed notice after the restart: {r['failed_notices']}")
                if r["side_effect_runs"] > 1:
                    problems.append(f"the in-flight command ran {r['side_effect_runs']} times across the kernel restart (a non-idempotent side effect doubled)")
                if r["side_effect_runs"] == 0:
                    problems.append("the in-flight command's side effect never happened (before or after the restart)")
            if j8["second_project"]["in_second"] != 1 or in_first or in_home:
                problems.append(f"the second project's line went astray: {j8['second_project']}")
            c_verdict = j8["dropped_connection"].get("verdict")
            if c_verdict == "fail":
                problems.append(f"dropped connection failed on the phone loop's last run ({j8['dropped_connection'].get('ts')}, {j8['dropped_connection'].get('evidence')})")
            if problems:
                mark("J8", "fail", "; ".join(problems))
            elif c_verdict == "pass":
                mark("J8", "pass", f"restart once (side effect once), second project isolated; dropped connection: pass on the phone loop's run {j8['dropped_connection'].get('ts')}")
            else:
                mark("J8", "unverified", "restart and second project fine; dropped connection: " + str(j8["dropped_connection"].get("why", c_verdict)))
        except Hang as e:
            # The window stopped answering: the step in progress fails, the rest are not reached, and the
            # journey files it as its own break — the class of bug that reads as "the app is broken".
            cur = next((s for s in STEPS if steps[s][1] == "not reached"), None)
            if cur:
                mark(cur, "fail", f"window hang: {e}")
            for s in STEPS:
                if steps[s][1] == "not reached":
                    steps[s] = ("unverified", f"not reached: the window hung earlier ({str(e)[:60]})")
            cx.rec.broke("journey-hang", str(e), "the window must answer the driver within 1 s after any sheet, dialog, tab or send")
        except Exception as e:  # noqa: BLE001
            cx.rec.log(f"journey exception: {type(e).__name__}: {e}")
            for s in STEPS:
                if steps[s][1] == "not reached":
                    steps[s] = ("unverified", f"not reached: {type(e).__name__}: {str(e)[:80]}")
        finally:
            try:
                pulses = getattr(rig, "pulses", [])
                if pulses:
                    slowest = max(pulses, key=lambda x: x[1])
                    ev["liveness"] = {"pulses": len(pulses), "slowest_ms": slowest[1], "slowest_after": slowest[0], "limit_ms": int(Rig.PULSE_LIMIT_S * 1000)}
            except Exception:  # noqa: BLE001
                pass
            try:
                rig.close(folders=[folder, second])
            except Exception:  # noqa: BLE001
                pass
            for name, place in (("project", folder), ("second", second), ("home-store", cx.scratch / "home" / ".arbos")):
                try:
                    cx.rec.snapshot(place, f"{name}-after")
                except Exception:  # noqa: BLE001
                    pass
            try:
                (cx.rec.dir / "project-git-log.txt").write_text(git(folder, "log", "--all", "--oneline") + "\n" + git(folder, "branch", "--list"))
            except Exception:  # noqa: BLE001
                pass
        finish()
