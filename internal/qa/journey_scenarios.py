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
        self.log = cx.rec.dir / f"{tag}.log"
        xdg = cx.scratch / "xdg"
        if reseed:
            self.app = arbosdriver.Arbos.launch(binary=DESKTOP_BIN, env=env, log=self.log, xdg=xdg, projects=[str(f) for f in folders], timeout=120)
        else:
            # The app's own state.toml decides what comes back — that is the point of "leave and come back".
            env["XDG_CONFIG_HOME"] = str(xdg)
            env["XDG_DATA_HOME"] = str(xdg / "data")
            self.app = arbosdriver.Arbos.launch(binary=DESKTOP_BIN, env=env, log=self.log, xdg=None, timeout=120)

    def state(self):
        return self.app.state()

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
        self.xvfb.terminate()


def register(scenario, registry, transcript, now_ms, branch):
    def reg(name):
        def deco(fn):
            scenario(name, needs_model=True, tags=("journey", "desktop", "headline"))(fn)
            registry[name]["branch"] = branch
            return fn

        return deco

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
            midflight = {}
            if rig.busy(folder):
                rig.send(f"Also add a line to CHANGELOG.md saying who asked for this: QA-{tag}.")
                midflight["followup_sent_at"] = time.time()
                time.sleep(4)
                if rig.busy(folder):
                    rig.send("Use British spelling in the CHANGELOG.")
                    midflight["steer_sent"] = True
                    time.sleep(4)
                if rig.busy(folder):
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
                j4["midflight"] = {"index": i_follow, "task_turn_complete": task_tc, "same_turn": same_turn, "relayed_to_worker": relayed, "turn_tools": turn_tools, "workers": sorted(workers_now), "taken": same_turn or (relayed and len(workers_now) == len(ev.get("J2", {}).get("workers", [])))}
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
            rig.close(folders=[], stop_kernels=False)  # quit the app only; the kernel keeps serving
            time.sleep(2)
            rig = Rig(cx, [folder], tag="app-relaunch", reseed=False)
            time.sleep(4)
            p = rig.project(folder)
            # The app puts the home tab in front at every launch by design (workspace.rs); the user
            # clicks their project tab. Recorded, not scored — Jacob decides whether that is what "come back" means.
            active = rig.active_path()
            rig.focus(folder)
            time.sleep(1)
            chat = rig.root_chat(folder)
            after_items = len((chat or {}).get("items", []))
            after_sessions = len((p or {}).get("sessions", [])) if p else 0
            leaves = rig.leaves()
            worker_rows = [l for l in leaves if l.startswith(("panel-agent", "panel-archived"))]
            notes_after = (folder / ".arbos" / "notes.md").read_text(errors="replace") if (folder / ".arbos" / "notes.md").exists() else ""
            ev["J6"] = {"tab_back": p is not None, "landed_on": active, "landed_on_project": active == str(folder).rstrip("/"), "items_before": before_items, "items_after": after_items, "sessions_before": before_sessions, "sessions_after": after_sessions, "worker_rows": len(worker_rows), "notes_unchanged": notes_before == notes_after, "notifications": "unverified: the driver exposes no unseen count"}
            if p is None:
                mark("J6", "fail", "the project tab did not come back after relaunch")
            elif after_items < before_items:
                mark("J6", "fail", f"transcript shorter after relaunch: {before_items} -> {after_items} items")
            elif not (worker_rows or agents_of(folder) - {"root"} == set()):
                mark("J6", "fail", "workers existed but no worker/archived row is shown after relaunch")
            else:
                mark("J6", "unverified", "tab, transcript and workers back" + ("" if ev["J6"]["landed_on_project"] else " (the app landed on the home tab, not the project)") + "; notifications not checkable on this rig")

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
            rig.send("Run `sleep 30; echo waited` with bash, then say done.")
            if rig.wait_busy(folder, 30):
                time.sleep(4)
                pid = kernel_pid(folder)
                if pid:
                    os.kill(pid, signal.SIGKILL)
                time.sleep(2)
                rig.send(f"After the restart, reply with the single word BACK-{tag}.")
                rig.wait_idle(folder, 120)
                time.sleep(2)
                evs8 = read_transcript(folder)
                backs = [e for e in evs8 if e.get("kind") == "user" and f"BACK-{tag}" in e.get("text", "")]
                answered = any(e.get("kind") == "assistant" and f"BACK-{tag}" in e.get("text", "") for e in evs8)
                notices = [str(i.get("text"))[:80] for i in (rig.root_chat(folder) or {}).get("items", []) if i.get("kind") == "notice" and i.get("failed")]
                j8["restart"] = {"pid_before": pid, "pid_after": kernel_pid(folder), "line_count": len(backs), "answered": answered, "failed_notices": notices[-2:]}
            else:
                j8["restart"] = "the slow turn never started"
            # Second project at the same time.
            second.mkdir(exist_ok=True)
            try:
                rig.app.click("new-tab")
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
            j8["dropped_connection"] = "unverified: the kernel is local on this rig; the phone loop owns the network drop"
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
            if j8["second_project"]["in_second"] != 1 or in_first or in_home:
                problems.append(f"the second project's line went astray: {j8['second_project']}")
            mark("J8", "fail" if problems else "unverified", "; ".join(problems) or "restart and second project fine; dropped connection not checkable here")
        except Exception as e:  # noqa: BLE001
            cx.rec.log(f"journey exception: {type(e).__name__}: {e}")
            for s in STEPS:
                if steps[s][1] == "not reached":
                    steps[s] = ("unverified", f"not reached: {type(e).__name__}: {str(e)[:80]}")
        finally:
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
