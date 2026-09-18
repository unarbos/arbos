#!/usr/bin/env python3
"""UI QA pass: click every interactive element arbos-desktop exposes through
its driver socket, assert the state change, and write results.

Usage:
    python3 ui_pass.py --branch <label> --binary <arbos-desktop> --kernel <arbos-kernel> \
        [--driver-py <arbosdriver.py>] [--model openai/gpt-5.4-mini] [--phases a,b,...]

Needs: DISPLAY (an X server; Xvfb is fine), scrot, wmctrl, xdotool, xclip,
OPENROUTER_API_KEY in the environment.

Lives in the repository (desktop/qa/parity) since 2026-09-16, when the store
dropped internal/parity/ whole; the store copy is a mirror of this one.

Writes (to /tmp/qa-ui/<branch>/ first, mirrored to the store after every phase,
because the store mount drops a write now and then):
    $STORE/media/qa-ui/<branch>/NNN-<element>.png   one still per check (scrot of the whole display)
    $STORE/media/qa-ui/<branch>/results.json         every check as a row
    $STORE/media/qa-ui/<branch>/inventory.json       interactive ids seen per screen
(ui_pass_report.py, which turned results.json into a table, was lost with the
store folder and had no copy.)

Notes learned on 2026-09-13:
  - `pills` live on the session (`state.sessions[].pills`), not the project.
  - The Worked fold (`work-N`) does not grow when it opens; the footer below it moves.
  - `session-[0-9]*` also matches `session-N.session-dots-N`; filter the dots out.
  - Enter while busy STEERS (kernel inbox file `kind = "steer"`, user card at
    once); ⇧⌘↩ or the `composer-queue` disc holds for the next turn (kernel
    inbox `message`, drawn as `followup-*` rows; `session.held`). The window
    keeps no queue of its own beyond a prompt typed before the socket is up
    (`session.queued`). No plan strip: `plan-head` must never exist.

Result values:
    pass           the expected state change was observed
    fail           the action was accepted but the state did not change as expected
    unverified     the action was accepted; the driver's state exposes nothing to assert on
    not-reachable  no element / driver cannot do it (a driver gap) / the state never appeared
    skipped        deliberately not exercised (destructive or opens a native dialog)

Phases (letters, default all):
    L launch+inventory  C composer  T turn (stop/follow-ups/footer/folds)  Q question card
    P standing work (panel only, no strip)  S sub-agents  A artifacts  B tab bar+opener
    R right panel/sidebar  W settings tab  M menus  K shortcuts
    X multitasking audit (steer file, queue held by the kernel and across a relaunch,
      typed words while a question stands, deleted child, first-spawn connect)
    G pull-request flow (a worker runs `gh pr create` against the fake gh on PATH;
      the kernel records it; the "PRs N" pill and the driver's pills.prs agree)
    O provider offer (second launch, no key)
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import re
import shutil
import subprocess
import sys
import time
import traceback
from pathlib import Path

from PIL import Image, ImageChops

sys.path.insert(0, str(Path(__file__).resolve().parent))
from rig import DisplayHung, binary_matches_tree, desktop_build, kernel_build, pulse as display_pulse, still as display_still, tree_sha  # noqa: E402

STORE = Path(os.environ.get("STORE", "/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983"))
# This folder: the scripts and fake gh beside this file. The driver module
# lives with the app at desktop/driver/arbosdriver.py.
PARITY = Path(os.environ.get("PARITY", Path(__file__).resolve().parent))
DRIVER_PY = Path(os.environ.get("ARBOS_DRIVER_PY", Path(__file__).resolve().parent.parent.parent / "driver" / "arbosdriver.py"))
PROJ = Path(os.environ.get("PROJ", "/tmp/parity-proj"))
DISPLAY = os.environ.get("DISPLAY", ":1")
ENV = {**os.environ, "DISPLAY": DISPLAY}

# A coordinator root has no shell of its own (project.toml role = coordinator on a
# new place), so the long turn is a worker that sleeps, waited on.
P_LONG = ("Spawn one sub-agent with the brief: run the shell command `sleep 45`, then reply with the word done. "
          "Wait for it (wait=true). When it returns, reply with the single word done.")
P_ASK = ("Before doing anything else, use your ask tool to ask me ONE multiple-choice question "
         "with exactly two options, 'alpha' and 'beta', about which name to use. Wait for my answer, then reply with the chosen name.")
# Named tool call, so the standing row is there to test on every run (a free
# prompt gets a clarifying question from the model about one time in two).
P_PLAN = ("Call the subscribe tool once with kind=shell, every=1h, cmd='python3 main.py', deliver_to=user, "
          "notify='main.py failed: {output}'. Then reply with the single word Scheduled.")
P_SUB = ("Use parallel sub-agents: one reviews math_utils.py for edge cases, one writes docstrings for every function, "
         "one drafts a CHANGELOG.md. Then merge their results.")
P_ART = "Create a file named report.md containing three bullet points about this project, then show me the file."
P_EDIT = "Add a mul(a, b) function to math_utils.py and call it from main.py with mul(4, 5)."
# A turn the root does itself — a command of its own — so the fold under
# test is the turn under test: a delegating turn's fold is bare by design
# (F-104) and the row had been clicking whatever fold was on screen (R10).
P_OWN = "Run `ls` yourself with bash — no workers — and tell me in one line what is here."
P_PR = ("Spawn one sub-agent whose only task is to run exactly this shell command and report the URL it prints: "
        "gh pr create --base master --head cursor/parity-pill --title 'Parity PR' --body 'Opened by the parity pass.' "
        "Wait for it, then reply with that URL.")


def log(msg: str) -> None:
    print(time.strftime("[%H:%M:%S] ") + msg, file=sys.stderr, flush=True)


def load_driver(path: Path):
    spec = importlib.util.spec_from_file_location("arbosdriver", path)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)  # type: ignore[union-attr]
    return mod


def dunst_history() -> list[str] | None:
    """The notification daemon's own record, one string per entry (summary +
    body), oldest first; None when there is no dunstctl. The proof that a
    posted notification reached a daemon rather than a log line."""
    if not shutil.which("dunstctl"):
        return None
    try:
        # History holds notifications once they are closed; a popup still
        # on screen is not in it yet. Close them first, then read.
        subprocess.run(["dunstctl", "close-all"], capture_output=True, timeout=5, env=ENV)
        raw = subprocess.run(["dunstctl", "history"], capture_output=True, text=True, timeout=5, env=ENV).stdout
        data = json.loads(raw).get("data", [[]])
        entries = data[0] if data else []
        out = []
        for e in reversed(entries):
            out.append(f"{e.get('summary', {}).get('data', '')} | {e.get('body', {}).get('data', '')}")
        return out
    except Exception:  # noqa: BLE001
        return None


def sessions(state: dict) -> list[dict]:
    return [c for p in state.get("projects", []) for c in p.get("sessions", [])]


def busy(state: dict) -> bool:
    return any(c["streaming"] or c["turn_open"] for c in sessions(state))


def inbox_kinds(agent: str = "root") -> list[str]:
    """`kind = "..."` of every file in the agent's kernel inbox."""
    out = []
    for f in sorted((PROJ / ".arbos" / "agents" / agent / "inbox").glob("*")):
        try:
            m = re.search(r'^kind\s*=\s*"([^"]+)"', f.read_text(), re.M)
            out.append(m.group(1) if m else f.suffix or f.name)
        except OSError:
            pass
    return out


def transcript_has(agent: str, text: str) -> bool:
    """The kernel's own record of `agent` carries a user line with `text`."""
    path = PROJ / ".arbos" / "agents" / agent / "transcript.jsonl"
    try:
        for line in path.read_text(errors="replace").splitlines():
            try:
                e = json.loads(line)
            except ValueError:
                continue
            if (e.get("kind") or e.get("type")) == "user" and text in str(e.get("text", "")):
                return True
    except OSError:
        pass
    return False


def project_sessions(state: dict) -> list[dict]:
    return [c for p in state.get("projects", []) if Path(p["path"]).resolve() == PROJ.resolve() for c in p.get("sessions", [])]


def attach_opens() -> int:
    log_path = PROJ / ".arbos" / "runtime" / "kernel.log"
    try:
        return sum(1 for line in log_path.read_text(errors="replace").splitlines() if '"attach_open"' in line)
    except OSError:
        return 0


def active(state: dict) -> dict | None:
    for c in sessions(state):
        if c["id"] == state.get("active_session"):
            return c
    return None


class Pass:
    def __init__(self, drv, app, branch: str, outdir: Path, store_dir: Path):
        self.drv = drv
        self.app = app
        self.branch = branch
        self.outdir = outdir          # local work dir (/tmp); the store mount drops writes now and then
        self.store_dir = store_dir    # mirrored here on every save()
        self.rows: list[dict] = []
        self.inventory: dict[str, list[str]] = {}
        self.n = 0
        self.tabs = False
        # The config home the app was launched with; set by main().
        self.xdg: Path = Path(f"/tmp/qa-ui-xdg-{branch}")

    # -- plumbing ---------------------------------------------------------

    def reconnect(self) -> None:
        try:
            self.app.hello()
        except Exception:
            self.app._drop()
            self.app.connect(timeout=15)
            log("driver reconnected")

    def state(self) -> dict:
        self.reconnect()
        return self.app.state()

    def still(self, name: str) -> str:
        self.n += 1
        safe = re.sub(r"[^A-Za-z0-9._-]+", "-", name)[:60]
        path = self.outdir / f"{self.n:03d}-{safe}.png"
        # Bounded: a hung display raises `DisplayHung`, which ends the run
        # with a row that says so rather than a clean-looking pass.
        display_still(path, DISPLAY)
        return str((self.store_dir / path.name).relative_to(STORE))

    def ids(self, pattern: str | None = None) -> list[str]:
        self.reconnect()
        els = self.app.snapshot()["elements"]
        return [e["path"] for e in els if e.get("interactive") and (pattern is None or self.drv._matches(pattern, e["path"]))]

    def inv(self, screen: str) -> list[str]:
        # A new scenario: a recover in the last one no longer taints rows.
        self.current_screen = screen
        if getattr(self, "context_lost", None) not in (None, screen):
            self.context_lost = None
        found = sorted(set(p.rsplit(".", 1)[-1] for p in self.ids()))
        self.inventory[screen] = found
        log(f"inventory {screen}: {len(found)} interactive ids")
        return found

    def record(self, element: str, screen: str, action: str, expected: str, observed: str, result: str, still: str = "") -> None:
        # After a recover moved the run to a fresh chat, later rows in the
        # same scenario run without the scenario's history: a pass there is
        # not the pass the row names (rig audit R5).
        if result == "pass" and getattr(self, "context_lost", None) == screen:
            result = "pass-after-recover"
        self.rows.append({"element": element, "screen": screen, "action": action, "expected": expected,
                          "observed": observed, "result": result, "branch": self.branch, "still": still})
        log(f"{result:13s} {element:38s} {action[:40]}")

    def check(self, element: str, screen: str, action: str, expected: str, do, ok=None, settle: float = 0.8, still: bool = True, wait: float = 0.0) -> dict | None:
        """Run `do()`, compare state before/after with `ok(before, after)`.

        `ok` returns True (pass), False (fail), or a string (observed detail
        that counts as pass). If `ok` is None the check is `unverified`.
        `wait` keeps re-reading the state, half a second at a time, until
        `ok` holds or `wait` seconds have passed — for an action whose
        effect is the kernel's to deliver (a fork, a spawn) and lands late
        on a loaded box (R31: `fork-turn` read "no state change" on three
        full gates and passed every time its phase ran alone).
        """
        try:
            before = self.state()
        except Exception as err:
            self.record(element, screen, action, expected, f"state failed: {err}", "not-reachable")
            return None
        try:
            do()
        except self.drv.DriverError as err:
            msg = str(err)
            # "not on screen … clipped away" is the driver refusing to click
            # what is not visible (R13) — the row is unreachable in this
            # scroll position, not failing. Cycle 36 read two settings
            # controls below the fold as fails (R24).
            kind = "not-reachable" if ("no element" in msg or "not reachable" in msg or "occluded" in msg or "not on screen" in msg) else "fail"
            self.record(element, screen, action, expected, msg, kind, self.still(element) if still else "")
            return None
        except Exception as err:
            self.record(element, screen, action, expected, f"{type(err).__name__}: {err}", "fail", self.still(element) if still else "")
            return None
        time.sleep(settle)
        try:
            after = self.state()
        except Exception as err:
            self.record(element, screen, action, expected, f"state after failed: {err}", "fail")
            return None
        if ok is not None and wait > 0:
            deadline = time.time() + wait
            while time.time() < deadline:
                try:
                    if ok(before, after) is True:
                        break
                except Exception:
                    break
                time.sleep(0.5)
                try:
                    after = self.state()
                except Exception:
                    break
        shot = self.still(element) if still else ""
        if ok is None:
            self.record(element, screen, action, expected, "action accepted; no state field to assert", "unverified", shot)
            return after
        try:
            verdict = ok(before, after)
        except Exception as err:
            verdict = f"assert error: {err}"
            self.record(element, screen, action, expected, verdict, "fail", shot)
            return after
        if verdict is True:
            self.record(element, screen, action, expected, "as expected", "pass", shot)
        elif verdict is False or verdict is None or verdict == "":
            self.record(element, screen, action, expected, self.diff(before, after), "fail", shot)
        elif isinstance(verdict, str) and verdict.startswith("unverified:"):
            # The assert could not decide: it says so instead of passing.
            # `self.diff(a, b) or "unverified: click accepted, nothing changed"` used to pass on no change at
            # all — an assertion that cannot fail (rig audit R14, cycle 31).
            self.record(element, screen, action, expected, verdict[len("unverified:"):].strip(), "unverified", shot)
        else:
            self.record(element, screen, action, expected, str(verdict), "pass", shot)
        return after

    @staticmethod
    def diff(a: dict, b: dict) -> str:
        keys = ["active_session", "active_project", "panel_open", "sidebar_open", "settings_open", "opener_open", "menu_open", "renaming", "active_surface"]
        out = [f"{k}: {a.get(k)} -> {b.get(k)}" for k in keys if a.get(k) != b.get(k)]
        if a.get("composer") != b.get("composer"):
            out.append(f"composer: {a.get('composer')} -> {b.get('composer')}")
        sa, sb = len(sessions(a)), len(sessions(b))
        if sa != sb:
            out.append(f"sessions: {sa} -> {sb}")
        return "; ".join(out) or "no state change"

    def skip(self, element: str, screen: str, action: str, why: str) -> None:
        self.record(element, screen, action, "-", why, "skipped")

    def gap(self, element: str, screen: str, action: str, why: str) -> None:
        self.record(element, screen, action, "-", why, "not-reachable")

    def stop_phase(self, screen: str, why: str, remaining: list[str], fault: bool) -> None:
        """A phase that cannot go on says so for every row it will not run.

        A gap followed by a bare `return` shrank the results instead of
        marking them: the settings phase would have recorded one
        `not-reachable` and stopped once its window was gone (#451's author
        found it), and the gate stayed green over a phase that had quit.
        `fault` says whether the stop is the app's or the rig's (a fail) or
        the world's — a model that never asked, a kernel from before a
        frame (not-reachable, said as such on each row).
        """
        kind = "fail" if fault else "not-reachable"
        self.record(f"phase {screen} stopped", screen, "-", "the phase runs to its end", f"{why}; {len(remaining)} check(s) after it did not run", kind)
        for name in remaining:
            self.record(name, screen, "-", "-", f"not run: the phase stopped early ({why})", "not-reachable")

    # -- app helpers ------------------------------------------------------

    def reveal(self, el: str, body: str, tries: int = 8) -> bool:
        """Scroll `body` until `el` is on screen. True when it is (or was)."""
        for _ in range(tries):
            try:
                found = self.app.find(el)
            except self.drv.DriverError:
                return False
            if found.get("visible") or found.get("reachable"):
                return True
            win_h = (self.state().get("window") or {}).get("height") or 900
            y = found.get("y", 0)
            step = 240 if y > win_h / 2 else -240
            try:
                self.app.scroll(body, dy=-step)
            except self.drv.DriverError:
                return False
            time.sleep(0.4)
        return False

    def wait(self, pred, timeout: float = 60, every: float = 0.5, what: str = "condition"):
        t0 = time.monotonic()
        last = None
        while time.monotonic() - t0 < timeout:
            last = self.state()
            if pred(last):
                return last
            time.sleep(every)
        log(f"timeout waiting for {what}")
        return None

    def wait_idle(self, timeout: float = 100):
        s = self.wait(lambda s: not busy(s), timeout, what="idle")
        if s is None:
            self.recover()
            s = self.state()
        return s

    def recover(self) -> None:
        """A turn that never ends blocks every later check. Stop it; if that
        fails, move to a fresh chat and record the stuck turn as a finding."""
        if not busy(self.state()):
            return
        if self.app.exists("composer-stop"):
            # The turn can end between the check and the click; a vanished
            # disc is not a failure of the recover (it aborted phase T once,
            # cycle 32).
            try:
                self.app.click("composer-stop")
            except self.drv.DriverError:
                if not busy(self.state()):
                    return
            if self.wait(lambda s: not busy(s), 10, what="stop"):
                self.record("recover", "turn-running", "Stop after a hung turn", "turn ends", "Stop ended it", "pass", self.still("recover-stop"))
                return
        # The root's turn is over but its workers run on: Cursor's card over
        # the pills has Stop All for that (cycle 8). The card sits behind
        # the Working pill (F-82); open it first.
        if not self.app.exists("working-stop-all") and self.app.exists("pill-working"):
            self.app.click("pill-working")
            time.sleep(0.5)
        if self.app.exists("working-stop-all"):
            self.app.click("working-stop-all")
            if self.wait(lambda s: not busy(s), 15, what="stop all"):
                self.record("recover", "turn-running", "Stop after a hung turn", "turn ends", "Stop All ended the workers", "pass", self.still("recover-stop-all"))
                return
        # Which agents are still busy, in the kernel's own words, so the
        # row says who held the turn — a root waiting on a spawn (the
        # kernel holds Stop until the child returns; filed 2026-09-17) reads
        # differently from a worker that ignored Stop.
        who = [(c.get("title") or c.get("name") or c.get("id"), c.get("live_status") or c.get("status")) for c in sessions(self.state()) if c.get("streaming") or c.get("turn_open")]
        # The kernel's side of the same moment, from its files: each busy
        # session's agent folder — last transcript record and status.toml.
        # A turn the kernel has ended (`turn_complete`, no status) while the
        # window still holds `turn_open` is the window's; one the kernel is
        # still in is the kernel's (F-179, cycle 39).
        kernel_side = []
        for c in sessions(self.state()):
            if not (c.get("streaming") or c.get("turn_open")):
                continue
            sid = c.get("agent_session") or ""
            d = PROJ / ".arbos" / "agents" / sid
            last = ""
            try:
                lines = (d / "transcript.jsonl").read_text().strip().splitlines()
                if lines:
                    last = json.loads(lines[-1]).get("kind", "")
            except Exception:
                last = "?"
            st = ""
            try:
                st = (d / "status.toml").read_text().splitlines()[0][:50]
            except Exception:
                pass
            kernel_side.append((sid, last, st))
        who = f"window={who!r} kernel={kernel_side!r}"
        self.stop_failures = getattr(self, "stop_failures", 0) + 1
        self.record("recover", "turn-running", "Stop after a hung turn", "turn ends", f"turn still busy after Stop ({self.stop_failures}x this run); {who}; opening a new chat", "fail", self.still("recover-stuck"))
        self.context_lost = getattr(self, "current_screen", None)
        # Twice in one run is the kernel holding Stop, not a row's fault:
        # every phase after this would fail the same way and bury the
        # run's real rows (cycle 37: 17 fails from one event). Say so once
        # and let the main loop mark what follows not-reachable (R27).
        if self.stop_failures >= 2:
            self.kernel_holds_stop = True
        self.app.key("cmd-n"); time.sleep(1.5)
        try:
            self.app.wait_element("composer-field", timeout=8, reachable=True)
        except Exception:
            pass

    def clear_composer(self) -> None:
        self.app.click("composer-field")
        for chord in ("cmd-a", "ctrl-a"):
            self.app.key(chord); self.app.key("backspace")
            if not self.state()["composer"]["text"]:
                return
        n = len(self.state()["composer"]["text"]) + 5
        self.app.key(" ".join(["backspace"] * n))

    def send(self, text: str) -> None:
        if self.state()["composer"]["text"]:
            self.clear_composer()
        self.app.click("composer-field")
        self.app.type(text + "\n")
        # The line must leave the composer: once (cycle 32's full gate) the
        # typed Enter landed on nothing and the prompt sat in the field while
        # every row after it measured a turn that never ran. Wait for the
        # field to empty; press Enter once more if it has not; say so.
        if self.wait(lambda s: not s["composer"]["text"], 4) is None:
            log(f"send: composer still holds the line after Enter; pressing Enter again ({text[:40]!r})")
            self.app.click("composer-field"); self.app.key("enter")
            if self.wait(lambda s: not s["composer"]["text"], 4) is None:
                log("send: the line did not leave the composer (R17)")

    def escape(self) -> None:
        try:
            self.app.key("escape")
        except Exception:
            pass

    def project_index(self) -> int | None:
        for p in self.state()["projects"]:
            if Path(p["path"]).resolve() == PROJ.resolve():
                return p["index"]
        return None

    def go_project(self) -> None:
        ix = self.project_index()
        if self.tabs and ix is not None:
            self.app.click(f"tab-{ix}")
            time.sleep(0.8)

    def seen(self, target: str) -> bool:
        """On screen, not merely laid out: `exists` lists elements scrolled or
        clipped out of view, and a row that means "the person sees X" must
        not pass on one of those (rig audit R1)."""
        if not self.app.exists(target):
            return False
        try:
            found = self.app.find(target)
        except Exception:
            return False
        # `visible` is bounds ∩ content mask; the PRs pill read False while
        # plainly on screen (cycle 31, `128-pill-prs.png`). `reachable` is
        # the hit test at the element's centre — the stronger word for an
        # interactive element. Either counts; neither is assumed (R15).
        return bool(found.get("visible")) or bool(found.get("reachable"))

    def first(self, pattern: str) -> str | None:
        found = self.ids(pattern)
        return found[0] if found else None

    def turn_ids(self) -> dict[str, str | None]:
        """Footer controls of the last finished turn, by kind."""
        out = {}
        for kind in ("copy-turn", "fork-turn", "vote-up", "vote-down", "rewind-turn", "turn-time"):
            found = self.ids(f"{kind}-*")
            out[kind] = found[-1] if found else None
        return out

    # -- phases ------------------------------------------------------------

    def phase_launch(self) -> None:
        s = self.state()
        self.tabs = self.app.exists("tab-bar")
        self.inv("launch")
        self.record("window", "launch", "launch", "main window with a composer", f"tabs layout={self.tabs}; projects={len(s['projects'])}", "pass", self.still("launch"))
        # F-52: a pre-version state.toml carrying `bionic_reading = true`
        # (every old build wrote its default back) must not weight the prose.
        self.record("bionic-leak", "launch", "launch on a state.toml with bionic_reading = true and no version", "bionic_reading false in the driver state (old default dropped, prose one weight)",
                    f"bionic_reading={s.get('bionic_reading')}", "pass" if s.get("bionic_reading") is False else "fail")
        # First launch: the Home face sheet, then the permissions sheet — a
        # modal in the main window (older builds: Settings › Permissions in
        # its own window). Either must close on Escape and give the keyboard
        # back to the chat; a first-launch window with no way out sat over
        # the tabs (features agent, main 52d4ae4).
        def perms(st):
            return st.get("permissions") or {}
        if self.app.exists("tab-sheet-done"):
            self.check("tab-sheet (first launch)", "launch", "Escape the Home face sheet", "sheet closes; the permissions sheet (or Settings › Permissions) follows",
                       lambda: self.app.key("escape"),
                       lambda a, b: not self.app.exists("tab-sheet-done") and (perms(b).get("open") is True or b.get("settings_open") is True), settle=1.5)
        if perms(self.state()).get("open"):
            st = self.state()
            rows = perms(st).get("rows", [])
            self.record("permissions-sheet", "launch", "first launch", "one row per applicable permission with a status; Enable all or Skip",
                        "; ".join(f"{r['permission']}={r['status']}/{r['phase']}" for r in rows) or "no rows",
                        "pass" if rows and all(r["phase"] == "idle" for r in rows) else "fail", self.still("permissions-sheet"))
            self.check("permissions-skip (first launch)", "launch", "Skip for now / Done", "sheet closes; permissions.seen true; the composer has the keyboard",
                       lambda: self.app.click("permissions-skip"),
                       lambda a, b: perms(b).get("open") is False and perms(b).get("seen") is True and b["composer"]["focused"] and "closed, seen, composer focused", settle=1.2)
        if self.state().get("settings_open"):
            self.check("settings-escape (first launch)", "launch", "Escape with the Settings tab in front", "back to the chat; the composer has the keyboard",
                       lambda: self.app.key("escape"),
                       lambda a, b: b.get("front") == "project" and b["composer"]["focused"] and "left the tab, composer focused", settle=1.2)
            self.check("tab-settings-close (first launch)", "launch", "the Settings pill's close mark", "the tab leaves the strip",
                       lambda: self.app.click("tab-settings-close"),
                       lambda a, b: b.get("settings_open") is False, settle=1.0)
        if self.tabs:
            ix = self.project_index()
            self.check(f"tab-{ix}", "tab-bar", "click project tab", "active_project becomes the sample project",
                       lambda: self.app.click(f"tab-{ix}"), lambda a, b: b["active_project"] == ix)
        try:
            self.app.wait_element("composer-field", timeout=8, reachable=True)
        except Exception:
            self.check("cmd-n", "launch", "cmd-n when no chat is open", "a chat opens with a composer",
                       lambda: self.app.key("cmd-n"), lambda a, b: self.seen("composer-field"))
        self.inv("empty-chat")

    def phase_composer(self) -> None:
        sc = "composer"
        self.check("composer-field", sc, "click, type 'hello'", "composer.text == 'hello' and focused",
                   lambda: (self.app.click("composer-field"), self.app.type("hello")),
                   lambda a, b: b["composer"]["text"] == "hello" and b["composer"]["focused"])
        self.check("composer-field", sc, "cmd-a + backspace", "composer empty",
                   self.clear_composer, lambda a, b: b["composer"]["text"] == "")
        # Model picker.
        after = self.check("composer-model", sc, "click", "model list opens (composer-model-list present)",
                           lambda: self.app.click("composer-model"), lambda a, b: self.seen("composer-model-list"))
        if after is not None and self.app.exists("composer-model-list"):
            self.inv("model-picker")
            if self.app.exists("composer-model-toggle"):
                self.check("composer-model-toggle", "model-picker", "click", "list changes length (all models / favourites)",
                           lambda: self.app.click("composer-model-toggle"),
                           lambda a, b: (len(self.ids('composer-model-*')) > 0 and f"{len(self.ids('composer-model-*'))} model rows after toggle") or "unverified: no model rows to count")
            rows = self.ids("composer-model-[0-9]*")
            if len(rows) > 1:
                cur = (active(self.state()) or {}).get("model")
                self.check(rows[1].rsplit(".", 1)[-1], "model-picker", "click second model row", "session.model changes and list closes",
                           lambda: self.app.click(rows[1]),
                           lambda a, b: ((active(b) or {}).get("model") != cur or not self.app.exists("composer-model-list")) and f"model {cur} -> {(active(b) or {}).get('model')}")
            self.escape()
        if self.app.exists("composer-model-list"):
            self.escape()
        # Slash commands.
        self.check("composer-field", sc, "type '/'", "slash command list opens (composer-commands-list)",
                   lambda: (self.app.click("composer-field"), self.app.type("/")),
                   lambda a, b: self.seen("composer-commands-list") and f"{len(self.ids('composer-slash-*'))} commands")
        if self.app.exists("composer-commands-list"):
            self.inv("slash-menu")
            cmds = self.ids("composer-slash-*")
            if cmds:
                self.check(cmds[0].rsplit(".", 1)[-1], "slash-menu", "click first command", "composer.text becomes that command",
                           lambda: self.app.click(cmds[0]), lambda a, b: b["composer"]["text"] != "/" and f"text -> {b['composer']['text']!r}")
        self.escape(); self.clear_composer()
        for el in ("composer-usage", "composer-cost", "composer-context", "composer-machine"):
            if self.app.exists(el):
                r = self.check(el, sc, "click", "a popover or menu opens (menu_open/opener_open), or it is a label",
                               lambda el=el: self.app.click(el),
                               lambda a, b: (b.get("menu_open") or b.get("opener_open") or self.diff(a, b) != "no state change") and self.diff(a, b))
                if self.rows and self.rows[-1]["element"] == el and self.rows[-1]["result"] == "fail":
                    self.rows[-1]["result"] = "unverified"; self.rows[-1]["observed"] = "click accepted, nothing changed (label, or a dead control)"
                self.escape()
            else:
                self.gap(el, sc, "click", "element not on screen in this state")
        self.skip("composer-attach", sc, "click", "opens the native file dialog (prompt_for_paths); would block the driver")
        self.check("composer-voice", sc, "click mic", "recording starts, or a voice notice appears (Linux has no dictation)",
                   lambda: self.app.click("composer-voice"),
                   lambda a, b: b["composer"]["recording"] and "recording" or (self.app.exists("composer-voice-note") and "voice note under the field") or (self.app.exists("composer-voice-status") and "voice status shown") or False)
        if self.state()["composer"]["recording"]:
            self.app.click("composer-voice")
        # Hold-Fn dictation, by the same calls the native key monitor makes
        # (driver method `fn`): down must start a take into the composer;
        # where the machine has no dictation (Linux), the voice notice it
        # raises is the proof the path is wired. Guards Jacob's constraint
        # that capture/permission work never breaks "hold Fn, talk".
        def fn_dictation():
            self.app.call("fn", down=True)
            time.sleep(1.2)
            self.app.call("fn", down=False)
        def fn_ok(a, b):
            if b["composer"]["recording"]:
                return "recording"
            if b["composer"]["text"] != a["composer"]["text"]:
                return f"composer text -> {b['composer']['text']!r}"
            if self.app.exists("composer-voice-note"):
                return "voice note under the composer field"
            act = active(b)
            if act:
                notices = [it.get("text", "") for it in act["items"] if it.get("kind") == "notice" and "voice" in it.get("text", "").lower()]
                if len(notices) > len([it for it in (active(a) or {"items": []})["items"] if it.get("kind") == "notice" and "voice" in it.get("text", "").lower()]):
                    return f"voice notice in the transcript (should be under the composer): {notices[-1][:60]}"
            return False
        self.check("fn-dictation", sc, "hold Fn 1.2 s (driver `fn` down/up)", "a take starts into the composer, or words land, or the dictation notice appears (no dictation on Linux)",
                   fn_dictation, fn_ok, settle=1.5)
        if self.state()["composer"]["recording"]:
            self.app.call("fn", down=False)
        self.check("composer-send", sc, "type text then click the send disc", "a turn starts (turn_open/streaming)",
                   lambda: (self.app.click("composer-field"), self.app.type("Reply with the single word pong."), self.app.click("composer-send")),
                   lambda a, b: busy(b) or len(active(b)["items"]) > len(active(a)["items"]) if active(a) and active(b) else busy(b), settle=1.5)
        self.wait_idle(90)

    def phase_turn(self) -> None:
        sc = "turn-running"
        # Run alone (`--phases T`), this phase starts five seconds after
        # launch, inside the kickoff turn: P_LONG is then held behind it and
        # the steer rows measure the kickoff (cycle 33's TD run). Let the
        # place settle first; in a full run this returns at once.
        self.wait_idle(90)
        self.send(P_LONG)
        s = self.wait(lambda s: busy(s), 20, what="turn start")
        self.inv(sc)
        self.check("composer-stop", sc, "present while running", "stop disc replaces mic/send", lambda: None,
                   lambda a, b: self.seen("composer-stop"), still=True)
        pills = (active(self.state()) or {}).get("pills")
        self.record("pill-working", sc, "read session.pills while the main turn runs", "pills present (working counts child agents, so 0 here is fine)", json.dumps(pills)[:120], "pass" if pills is not None else "unverified", self.still("pills"))
        # Enter while busy steers: the words go to the running turn now.
        steer_text = "Afterwards say hello."
        def steered(a, b):
            act = active(b) or {}
            users = [it.get("text", "") for it in act.get("items", []) if it.get("kind") == "user"]
            landed = any(steer_text in u for u in users)
            return landed and act.get("queued", 0) == 0 and f"user card landed; queued={act.get('queued')} held={act.get('held')} inbox={inbox_kinds()}"
        self.check("composer-field", sc, "type follow-up + Enter while busy", "steer: user card lands at once, no window queue, kernel inbox gets kind=steer",
                   lambda: (self.app.click("composer-field"), self.app.type(steer_text + "\n")), steered, settle=1.5)
        agent = (active(self.state()) or {}).get("agent_session") or "root"
        self.record("steer-inbox", sc, "read the kernel inbox / transcript after Enter while busy", "a `kind = \"steer\"` inbox file, or the line already taken into the running turn's transcript",
                    f"inbox kinds={inbox_kinds(agent)} in_transcript={transcript_has(agent, steer_text)}", "pass" if "steer" in inbox_kinds(agent) or transcript_has(agent, steer_text) else "fail", "")
        self.check("composer-stop", sc, "present beside Send while running", "Stop is its own disc; no composer-force", lambda: None,
                   lambda a, b: self.seen("composer-stop") and not self.app.exists("composer-force") and "stop disc, no force disc", still=True)
        # Queue: ⇧⌘↩ holds the words for the next turn; the kernel keeps them.
        if not busy(self.state()):
            self.send(P_LONG); self.wait(lambda s: busy(s), 20, what="turn start")
        self.check("composer-field cmd-shift-enter", sc, "type + ⇧⌘↩ while busy", "kernel follow-up row (followups-head), session.held > 0, composer empty",
                   lambda: (self.app.click("composer-field"), self.app.type("Also say thanks."), self.app.key("cmd-shift-enter")),
                   # Since the coordinator shell (#190) the root's own turn is short and its workers
                   # carry the long part: `busy` here is F-97's disc over running workers, and a
                   # prompt to an idle root runs at once — Cursor's shape too (its steer while a
                   # worker ran became its own Worked turn). Held, or landed as a user card, both pass.
                   lambda a, b: b["composer"]["text"] == "" and (self.seen("followups-head") or (active(b) or {}).get("held", 0) > 0 or any("Also say thanks." in it.get("text", "") for it in (active(b) or {}).get("items", []) if it.get("kind") == "user")) and f"held={(active(b) or {}).get('held')} followups-head={self.app.exists('followups-head')} landed={any('Also say thanks.' in it.get('text', '') for it in (active(b) or {}).get('items', []) if it.get('kind') == 'user')}", settle=2)
        self.inv("follow-up-row")
        if busy(self.state()):
            agent = (active(self.state()) or {}).get("agent_session") or "root"
            self.check("composer-queue", sc, "type text while busy, click the Queue disc", "composer empty, nothing in the window queue; the kernel holds it (held grows) or has already run it (transcript)",
                       lambda: (self.app.click("composer-field"), self.app.type("Then say goodbye."), self.app.click("composer-queue")),
                       lambda a, b: b["composer"]["text"] == "" and (active(b) or {}).get("queued", 0) == 0
                       and ((active(b) or {}).get("held", 0) > (active(a) or {}).get("held", 0) or transcript_has(agent, "Then say goodbye.") or "message" in inbox_kinds(agent))
                       and f"held {(active(a) or {}).get('held')}->{(active(b) or {}).get('held')} inbox={inbox_kinds(agent)}", settle=2)
        else:
            self.gap("composer-queue", sc, "click", "turn already idle before the Queue disc could be tried")
        edit = self.first("followup-edit-*")
        if edit:
            self.check(edit.rsplit(".", 1)[-1], "follow-up-row", "click Edit", "row gone, composer holds the text",
                       lambda: self.app.click(edit), lambda a, b: b["composer"]["text"] != "" and (active(b) or {}).get("held", 0) < (active(a) or {}).get("held", 0), settle=2)
            self.clear_composer()
        else:
            self.gap("followup-edit", "follow-up-row", "click", "no followup-edit-* element after queuing")
        rem = self.first("followup-remove-*")
        if rem:
            self.check(rem.rsplit(".", 1)[-1], "follow-up-row", "click Remove", "held count drops, composer stays empty",
                       lambda: self.app.click(rem), lambda a, b: (active(b) or {}).get("held", 0) < (active(a) or {}).get("held", 0) and b["composer"]["text"] == "", settle=2)
        else:
            self.gap("followup-remove", "follow-up-row", "click", "no followup-remove-* element after queuing")
        if busy(self.state()):
            self.app.click("composer-field"); self.app.type("And then count to three."); self.app.key("cmd-shift-enter"); time.sleep(2)
        snd = self.first("followup-send-*")
        if snd:
            self.check(snd.rsplit(".", 1)[-1], "follow-up-row", "click Send now", "row gone, words steer the running turn (user card lands, still busy)",
                       lambda: self.app.click(snd), lambda a, b: not self.app.exists(snd) and f"busy={busy(b)} held={(active(b) or {}).get('held')}", settle=2)
        else:
            self.gap("followup-send", "follow-up-row", "click", "no followup-send-* element after queuing")
        for el in ("force-queue", "unqueue-*", "composer-force", "plan-head"):
            if self.first(el):
                self.record(el, sc, "must not exist", "no window-side queue controls, no plan strip", "present", "fail", self.still(f"stale-{el}"))
        # Stop.
        if not busy(self.state()):
            self.send(P_LONG); self.wait(lambda s: busy(s), 20, what="turn start")
        root_was_busy = bool((active(self.state()) or {}).get("streaming") or (active(self.state()) or {}).get("turn_open"))
        self.check("composer-stop", sc, "click Stop", "turn ends (not busy) within 5 s",
                   lambda: self.app.click("composer-stop"), lambda a, b: not busy(self.wait(lambda s: not busy(s), 8) or b), settle=1)
        if root_was_busy:
            self.notice_check("stop-notice", sc, "notice after Stop", "'Stopped by you' and no failed notice (PR #83)")
        else:
            # F-97: the disc was over running workers with the root idle; Stop stops
            # them and the root's transcript gets no interrupted line — Cursor's
            # shape too (its Stop over workers left bare Worked headers, no
            # "Stopped by you"). No failed notice is the check that remains.
            items = (active(self.state()) or {}).get("items", [])
            failed = [it.get("text", "")[:60] for it in items[-6:] if it.get("kind") == "notice" and it.get("failed")]
            self.record("stop-notice", sc, "Stop over running workers (root idle)", "no failed notice; no 'Stopped by you' expected on the idle root",
                        f"failed notices={failed}", "pass" if not failed else "fail", self.still("stop-notice"))
        # Stop word.
        self.send(P_LONG); self.wait(lambda s: busy(s), 20, what="turn start")
        self.check("stop-word", sc, "type 'stop' + Enter while busy", "turn interrupted, nothing queued",
                   lambda: (self.app.click("composer-field"), self.app.type("stop\n")),
                   lambda a, b: (not busy(self.wait(lambda s: not busy(s), 8) or b)) and (active(self.state()) or {}).get("queued", 0) == 0, settle=1)
        self.notice_check("stop-word-notice", sc, "notice after a stop word", "'Stopped by you' and no failed notice (PR #83)")
        self.wait_idle(30)
        # Footer of the last turn.
        sc = "turn-footer"
        self.send("Reply with exactly: The quick brown fox."); self.wait_idle(60); time.sleep(1)
        self.inv(sc)
        ids = self.turn_ids()
        if ids["copy-turn"]:
            def copied(a, b):
                out = subprocess.run(["xclip", "-o", "-selection", "clipboard"], capture_output=True, text=True, env=ENV).stdout
                return ("quick brown fox" in out) or f"clipboard={out[:40]!r}"
            self.check("copy-turn", sc, "click copy", "clipboard holds the answer", lambda: self.app.click(ids["copy-turn"]), copied)
        else:
            self.gap("copy-turn", sc, "click", "no copy-turn-* element")
        for name in ("vote-up", "vote-down"):
            if ids[name]:
                self.check(name, sc, "click", "message.feedback set", lambda n=name: self.app.click(ids[n]),
                           lambda a, b: json.dumps([m.get("feedback") for m in active(b)["items"] if m.get("feedback")]) )
            else:
                self.gap(name, sc, "click", f"no {name}-* element")
        if ids["turn-time"]:
            self.check("turn-time", sc, "hover", "tooltip; no state change", lambda: self.app.hover(ids["turn-time"]), None)
        if ids["fork-turn"]:
            self.check("fork-turn", sc, "click fork", "a new session appears and becomes active",
                       lambda: self.app.click(ids["fork-turn"]), lambda a, b: len(sessions(b)) == len(sessions(a)) + 1 and b["active_session"] != a["active_session"], settle=1.5, wait=10.0)
            main = [c for c in sessions(self.state()) if not c.get("parent")]
            if main:
                self.app.action("arbos::FocusSession", main[0]["id"]) if False else None
        else:
            self.gap("fork-turn", sc, "click", "no fork-turn-* element")
        # Fold lines from an edit turn.
        self.go_main()
        self.send(P_EDIT); self.wait_idle(120); time.sleep(1)
        self.send(P_OWN); self.wait_idle(90); time.sleep(1)
        sc = "turn-folds"
        self.inv(sc)
        # `work-bare-N` is a headline over nothing foldable (F-104): no chevron,
        # nothing to click. Only a real fold is exercised here.
        # The last fold that is actually on screen. The first non-bare fold
        # in id order was the kickoff's, scrolled far out of view after an
        # edit turn; the click landed on nothing and the row read "no state
        # change" (F-123's last case, cycle 27).
        def on_screen(w):
            try:
                f = self.app.find(w); return bool(f.get("visible")) and f.get("y", -1) >= 0
            except Exception:
                return False
        folds = [w for w in self.ids("work-*") if "work-bare-" not in w and on_screen(w)]
        work = folds[-1] if folds else None
        if work:
            # What a fold shows or hides is the rows under it — `run-*`,
            # `tool-*`, `thought-*`, a card. The footer's y is not a proxy:
            # the transcript is bottom-anchored, so a fold opening above the
            # viewport's bottom shifts content up and leaves the footer where
            # it was — "no state change" on this row, cycles 23–25 (F-123),
            # while the fold had in fact opened.
            def rows():
                # Everything a Project-chat fold can hold: runs, tools,
                # thoughts, cards — and the checklist card, which is all a
                # delegating turn's fold holds.
                kinds = ("run-*", "tool-*", "thought-*", "diff-card-*", "term-card-*", "todo-card-*", "worker-line-*")
                return set().union(*(set(self.ids(k)) for k in kinds))
            r0 = rows()
            self.check("work", sc, "click 'Worked' fold", "fold toggles: rows under it appear or disappear",
                       lambda: self.app.click(work), lambda a, b: (rows() != r0) and f"rows {len(r0)} -> {len(rows())}")
            self.check("work", sc, "click 'Worked' fold again", "fold toggles back", lambda: self.app.click(work), lambda a, b: rows() == r0 and f"rows back to {len(r0)}")
        else:
            bare = self.first("work-bare-*")
            self.gap("work", sc, "click", "no work-* fold on screen after an edit turn" + (" (a bare 'Worked' headline over a delegating turn — nothing to fold, F-104)" if bare else ""))
        for kind in ("tool", "thought", "diff-card", "term-card", "term-body", "diff-body"):
            el = self.first(f"{kind}-*")
            if not el:
                self.gap(kind, sc, "click", f"no {kind}-* element after an edit turn")
                continue
            h0 = self.app.find(el)["h"]
            self.check(kind, sc, "click fold line", "element height changes (expands/collapses)",
                       lambda el=el: self.app.click(el), lambda a, b: (self.app.find(el)["h"] != h0) and f"h {h0:.0f} -> {self.app.find(el)['h']:.0f}")
        el = self.first("jump-to-end-*")
        if el:
            self.check("jump-to-end", sc, "scroll up then click", "no error", lambda: (self.app.scroll("composer-field", dy=800), self.app.click(el)), None)
        ids = self.turn_ids()
        if ids["rewind-turn"]:
            # A worker's late report can wake the root into a new turn right
            # here; the app then rightly refuses "stop the turn before
            # rewinding" (cycle 32t: `turn_ended=None` at the click). Let
            # that turn end first, so the row measures Rewind, not the race.
            self.wait_idle(90)
            # Idle is not enough: cycle 33's run had a wake land one second
            # before the click (`progress=1s ago`). Wait for a quiet stretch
            # — no worker running, nothing arrived for four seconds — so the
            # click meets a chat with no turn about to open.
            self.wait(lambda s: not busy(s) and not ((active(s) or {}).get("pills") or {}).get("working") and (active(s) or {}).get("quiet_secs", 0) >= 4, 120, what="quiet before rewind")
            ids = self.turn_ids()
        if ids["rewind-turn"]:
            n_items = len(active(self.state())["items"])
            self.check("rewind-turn", sc, "click Rewind here (idle)", "transcript cut, prompt back in the composer",
                       lambda: self.app.click(ids["rewind-turn"]),
                       lambda a, b: (len(active(b)["items"]) < n_items or "mul(" in b["composer"]["text"]) and f"items {n_items} -> {len(active(b)['items'])}, composer={b['composer']['text'][:30]!r}", settle=6)
            self.clear_composer()
        else:
            self.gap("rewind-turn", sc, "click", "no rewind-turn-* element")

    def notice_check(self, element: str, screen: str, action: str, expected: str) -> None:
        time.sleep(1.5)
        items = (active(self.state()) or {}).get("items", [])
        # Since the turn the user typed: a worker's report can wake the
        # agent into a segment after the stop (#292), pushing the notice
        # past a fixed window of four.
        last_user = max((i for i, it in enumerate(items) if it.get("kind") == "user"), default=max(0, len(items) - 4))
        tail = [it for it in items[last_user:] if it.get("kind") == "notice"]
        text = " / ".join(f"{'FAILED ' if it.get('failed') else ''}{it.get('text', '')[:60]}" for it in tail) or "no notice"
        ok = any("Stopped by you" in it.get("text", "") for it in tail) and not any(it.get("failed") for it in tail)
        self.record(element, screen, action, expected, text, "pass" if ok else "fail", self.still(element))

    def go_main(self) -> None:
        """Make a root (non-child) session active by clicking rows until one is."""
        if (active(self.state()) or {}).get("parent") is None and active(self.state()):
            return
        rows = self.ids("panel-agent-*") if self.tabs else self.ids("session-[0-9]*")
        for row in rows:
            self.app.click(row); time.sleep(0.5)
            if (active(self.state()) or {}).get("parent") is None:
                return

    def phase_question(self) -> None:
        sc = "question-card"
        self.send(P_ASK)
        s = self.wait(lambda s: (active(s) or {}).get("questions"), 90, what="question card")
        if not s:
            self.stop_phase(sc, "the model never asked (no state.questions within 90 s)", ["ask-option", "ask-other", "ask-prev", "ask-next", "ask-continue", "ask-skip", "ask-skip follow-through", "ask-typed-text"], fault=False)
            self.wait_idle(60); return
        self.inv(sc)
        opt = self.first("ask-*-0")
        if opt:
            self.check(opt.rsplit(".", 1)[-1], sc, "click option A", "option selected (card stays)", lambda: self.app.click(opt), None)
        other = self.first("ask-*-other")
        if other:
            self.check("ask-other", sc, "click Other…", "other row selected; a text field appears", lambda: self.app.click(other), lambda a, b: self.diff(a, b) or "unverified: card still up, nothing changed")
            if opt:
                self.app.click(opt)
        for el in ("ask-prev", "ask-next"):
            if self.app.exists(el):
                self.check(el, sc, "click", "page changes or no-op on a single question", lambda el=el: self.app.click(el), None)
            else:
                self.gap(el, sc, "click", "not shown (single question)")
        if self.app.exists("ask-continue"):
            self.check("ask-continue", sc, "click Continue", "questions cleared, turn continues",
                       lambda: self.app.click("ask-continue"), lambda a, b: not (active(b) or {}).get("questions"), settle=1.5)
        else:
            self.gap("ask-continue", sc, "click", "no ask-continue element")
        self.wait_idle(90)
        # Skip path.
        self.send(P_ASK)
        if self.wait(lambda s: (active(s) or {}).get("questions"), 90, what="question card"):
            if self.app.exists("ask-skip"):
                self.check("ask-skip", sc, "click Skip", "questions cleared", lambda: self.app.click("ask-skip"), lambda a, b: not (active(b) or {}).get("questions"), settle=1.5)
                s3 = self.wait_idle(60)
                items = (active(s3) or {}).get("items", []) if s3 else []
                agent = [it.get("text", "") for it in items if it.get("kind") == "agent"]
                notices = [it.get("text", "") for it in items[-4:] if it.get("kind") == "notice"]
                last = (agent[-1] if agent else "")[:140]
                bad = any(w in last.lower() for w in ("empty", "blocked", "no answer", "didn't receive", "did not receive")) or any("refused" in n for n in notices)
                self.record("ask-skip follow-through", sc, "read the answer after Skip", "the model is told the user skipped and carries on", f"agent: {last!r}; notices: {notices}", "fail" if bad else "pass", self.still("ask-skip-answer"))
            else:
                self.gap("ask-skip", sc, "click", "no ask-skip element")
        self.wait_idle(60)
        self.recover()
        # The approval card (`permission-strip`) is gone: nothing ever
        # emitted the two events that opened it, so this row read
        # `not-reachable` in every gate since it was written. The kernel's
        # approvals arrive as `ask` frames and are driven by the ask rows
        # above (rig audit R23).
        self.wait_idle(90)

    def phase_plan(self) -> None:
        """Standing work: in the Project panel, once; nothing pinned over the composer."""
        sc = "standing"
        # Scenario 17: a fresh chat has nothing above the composer, and the
        # kernel's own weekly git gc chore is not the user's standing work.
        self.app.key("cmd-n"); time.sleep(1.2)
        self.record("empty-chat-strip", sc, "new chat", "no plan-head, no followups-head, held == 0, no panel-standing row for a kernel chore",
                    f"plan-head={self.app.exists('plan-head')} followups-head={self.app.exists('followups-head')} held={(active(self.state()) or {}).get('held')} standing_rows={len(self.ids('panel-standing-*'))}",
                    "pass" if not self.app.exists("plan-head") and not self.app.exists("followups-head") and (active(self.state()) or {}).get("held", 0) == 0 else "fail", self.still("empty-chat"))
        self.go_project()
        self.send(P_PLAN)
        s = self.wait(lambda s: bool(self.ids("panel-standing-*")) or not busy(s), 90, what="standing row")
        self.wait_idle(60)
        rows = self.ids("panel-standing-*")
        self.inv(sc)
        # Scenario 19: one subscription, one row, in the panel; no strip.
        self.record("standing-once", sc, "recurring plan prompt", "exactly one panel-standing row for the new subscription; no plan-head",
                    f"panel-standing rows={len(rows)} plan-head={self.app.exists('plan-head')}",
                    "pass" if len(rows) == 1 and not self.app.exists("plan-head") else ("fail" if rows or self.app.exists("plan-head") else "not-reachable"), self.still("standing"))
        if rows:
            self.check(rows[0].rsplit(".", 1)[-1], sc, "click standing row", "opens the owning chat / no error", lambda: self.app.click(rows[0]), None)

    def phase_subagents(self) -> None:
        sc = "sub-agents"
        n0 = len(sessions(self.state()))
        self.send(P_SUB)
        s = self.wait(lambda s: len(sessions(s)) > n0, 90, what="child sessions")
        time.sleep(3)
        self.inv(sc)
        pills = (active(self.state()) or {}).get("pills")
        self.record("pill-working", sc, "read session.pills with children running", "pills.working > 0", json.dumps(pills)[:120], "pass" if pills and pills.get("working") else "fail", self.still("pills-subagents"))
        # The line under the turn appears once the spawn record names its
        # child, a beat after the panel row.
        t0 = time.time()
        child = self.first("child-line-*")
        while not child and time.time() - t0 < 20:
            time.sleep(0.5)
            child = self.first("child-line-*")
        if child:
            def click_child():
                # The line may have scrolled off the top while the turn
                # streamed; bring it back into the viewport first.
                # The wheel must land on the transcript itself: over the
                # composer it reaches nothing that scrolls the chat.
                pane = self.first("transcript-*") or "composer-field"
                for _ in range(8):
                    el = self.app.find(child)
                    if el.get("visible") and el.get("reachable"):
                        break
                    self.app.scroll(pane, dy=-(el["cy"] - 400))
                    time.sleep(0.4)
                r = self.app.click(child)
                log(f"child-line click -> {json.dumps(r)[:300]}")
            def child_ok(a, b):
                log(f"child-line active {a['active_session']} -> {b['active_session']}; parent of active after: {(active(b) or {}).get('parent')}")
                return b["active_session"] != a["active_session"] and (active(b) or {}).get("parent") is not None
            # F-82: while the root's turn waits on several workers the
            # transcript has one "N Working  <step>" line (Cursor's shape);
            # it opens the Working card, whose rows open the workers. The
            # card is closed until then — a delegated task shows the line
            # and the "Working N" pill, nothing more.
            several = (pills or {}).get("working", 0) > 1 and child.endswith("child-line-live")
            if several:
                self.record("working-card-closed", sc, "read the pills row while workers run", "no Working card until the pill or the line opens it",
                            f"working-card exists={self.app.exists('working-card')}", "fail" if self.app.exists("working-card") else "pass", "")
                self.check("child-line", sc, "click the 'N Working' line with several workers", "the Working card opens (a row per worker)",
                           click_child, lambda a, b: self.seen("working-card"), settle=0.8)
                row = self.first("working-row-*")
                if row:
                    self.check("working-row", sc, "click a Working card row", "active_session becomes the child",
                               lambda: self.app.click(row), child_ok)
                else:
                    self.gap("working-row", sc, "click", "no working-row-* after the line opened the card")
            else:
                self.check("child-line", sc, "click inline sub-agent line", "active_session becomes the child", click_child, child_ok)
            back = self.first("chat-header-crumb-*") or (self.first("panel-agent-*") if self.tabs else None)
            if back:
                self.check("chat-header-crumb", sc, "click crumb / back", "active_session back to the parent",
                           lambda: self.app.click(back), lambda a, b: (active(b) or {}).get("parent") is None)
            else:
                self.gap("chat-header-crumb", sc, "click", "no crumb element on this layout; went back through the sidebar")
                self.go_main()
        else:
            self.gap("child-line", sc, "click", "no child-line-* element while children run")
        if self.tabs:
            rows = self.ids("panel-agent-*")
            if len(rows) > 1:
                self.check("panel-agent (child)", "right-panel", "click child row", "active_session becomes that child",
                           lambda: self.app.click(rows[1]), lambda a, b: (active(b) or {}).get("parent") is not None)
                self.check("panel-agent (main)", "right-panel", "click main row", "active_session back to main",
                           lambda: self.app.click(rows[0]), lambda a, b: (active(b) or {}).get("parent") is None)
                self.check("panel-agent right-click", "right-panel", "right-click child row", "context menu opens (menu_open)",
                           lambda: self.app.right_click(rows[1]), lambda a, b: b.get("menu_open") is True)
                self.escape()
                self.check("panel-agent double-click", "right-panel", "double-click child row", "rename field (renaming true)",
                           lambda: self.app.double_click(rows[1]), lambda a, b: b.get("renaming") is True)
                self.escape()
        else:
            row = self.first("session-dots-*")
            if row:
                self.check("session-dots", "sidebar", "click a row's … button", "chat menu opens (menu_open)", lambda: self.app.click(row), lambda a, b: b.get("menu_open") is True)
                self.escape()
        self.wait_idle(180)
        self.go_main()

    def phase_artifacts(self) -> None:
        sc = "artifacts"
        self.send(P_ART); self.wait_idle(120); time.sleep(1)
        self.inv(sc)
        card = self.first("artifact-*") or self.first("artifacts-*")
        if card:
            self.check(card.rsplit(".", 1)[-1], sc, "click artifact card", "a surface opens (active_surface set / surfaces grow)",
                       lambda: self.app.click(card), lambda a, b: (b.get("active_surface") != a.get("active_surface") or len(a["projects"][0]["surfaces"]) != len(b["projects"][0]["surfaces"])) and f"active_surface={b.get('active_surface')}", settle=1.5)
            for el in ("surface-open", "surface-document", "surface-files", "surface-pane-close-*", "surface-close-*"):
                f = self.first(el)
                if f:
                    self.check(el, "surface", "click", "surface acts / closes", lambda f=f: self.app.click(f), lambda a, b: self.diff(a, b) or "unverified: click accepted, nothing changed")
        else:
            self.gap("artifact-card", sc, "click", "no artifact-* card after a file-creating turn")

    def phase_tabs(self) -> None:
        if not self.tabs:
            self.gap("tab-bar", "tab-bar", "-", "no tab bar on this branch (sidebar layout)")
            # Sidebar controls instead.
            sc = "sidebar"
            self.inv(sc)
            for el, exp, ok in (
                ("toggle-sidebar", "sidebar_open flips", lambda a, b: a.get("sidebar_open") != b.get("sidebar_open")),
                ("open-project", "opener opens", lambda a, b: b.get("opener_open") is True),
            ):
                if self.app.exists(el):
                    self.check(el, sc, "click", exp, lambda el=el: self.app.click(el), ok)
                    if el == "toggle-sidebar":
                        self.app.click(el)
                    self.escape()
            p = self.first("project-[0-9]*")
            if p:
                self.check("project (hover) + project-add", sc, "hover project, click +", "new session in that project",
                           lambda: (self.app.hover(p), self.app.click(self.first("project-add-*"))), lambda a, b: len(sessions(b)) == len(sessions(a)) + 1)
                self.check("project right-click", sc, "right-click project heading", "menu opens", lambda: self.app.right_click(p), lambda a, b: b.get("menu_open") is True)
                self.escape()
            for el, exp, ok in (("quick-new-chat", "new session", lambda a, b: len(sessions(b)) == len(sessions(a)) + 1),
                                ("quick-open-folder", "opener opens", lambda a, b: b.get("opener_open") is True)):
                if self.app.exists(el):
                    self.check(el, sc, "click", exp, lambda el=el: self.app.click(el), ok); self.escape()
            dots = self.first("session-dots-*")
            if dots:
                self.check("session-dots", sc, "click row … button", "chat menu opens (menu_open)", lambda: self.app.click(dots), lambda a, b: b.get("menu_open") is True)
                if self.state().get("menu_open"): self.inv("session-menu")
                self.escape()
            srow = [r_ for r_ in self.ids("session-[0-9]*") if "session-dots" not in r_]
            if len(srow) > 1:
                for r_ in srow[:2]:
                    self.check("session row", sc, f"click row {r_.rsplit('.', 1)[-1]}", "that chat becomes active; no menu opens",
                               lambda r_=r_: self.app.click(r_), lambda a, b: (not b.get("menu_open")) and f"active {a['active_session']} -> {b['active_session']}, menu_open={b.get('menu_open')}")
                    self.escape()
                self.check("session row right-click", sc, "right-click chat row", "menu opens", lambda: self.app.right_click(srow[1]), lambda a, b: b.get("menu_open") is True); self.escape()
                self.check("session row double-click", sc, "double-click chat row", "rename (renaming true)", lambda: self.app.double_click(srow[1]), lambda a, b: b.get("renaming") is True); self.escape()
                self.check("sidebar-split", sc, "drag split 80 px right", "sidebar_width changes", lambda: self.app.drag("sidebar-split", (self.app.find("sidebar-split")["cx"] + 80, self.app.find("sidebar-split")["cy"])), lambda a, b: a.get("sidebar_width") != b.get("sidebar_width"))
            for el in ("project-mark-*", "pinned-head", "project-archive-*"):
                f = self.first(el)
                if f:
                    self.check(el, sc, "click", "state changes", lambda f=f: self.app.click(f), lambda a, b: self.diff(a, b) or "unverified: click accepted, nothing changed")
                    self.escape()
            return
        sc = "tab-bar"
        self.inv(sc)
        ix = self.project_index()
        self.check("ctrl-tab", sc, "ctrl-tab", "active_project changes", lambda: self.app.key("ctrl-tab"), lambda a, b: b["active_project"] != a["active_project"])
        self.check("ctrl-shift-tab", sc, "ctrl-shift-tab", "active_project back", lambda: self.app.key("ctrl-shift-tab"), lambda a, b: b["active_project"] == ix)
        self.check("cmd-shift-]", sc, "cmd-shift-]", "active_project changes", lambda: self.app.key("cmd-shift-]"), lambda a, b: b["active_project"] != a["active_project"])
        self.check("cmd-shift-[", sc, "cmd-shift-[", "active_project back", lambda: self.app.key("cmd-shift-["), lambda a, b: b["active_project"] == ix)
        self.check("tab-0", sc, "click Home tab", "active_project 0", lambda: self.app.click("tab-0"), lambda a, b: b["active_project"] == 0)
        self.check(f"tab-{ix}", sc, "click project tab", f"active_project {ix}", lambda: self.app.click(f"tab-{ix}"), lambda a, b: b["active_project"] == ix)
        self.check("tab double-click", sc, "double-click project tab", "tab edit sheet opens (tab-sheet-done present)",
                   lambda: self.app.double_click(f"tab-{ix}"), lambda a, b: self.seen("tab-sheet-done") or self.seen("tab-sheet-cancel"))
        self.escape()
        # Kernel notifications (#293/#297): a reply that lands while another
        # tab is in front stays unseen (the tab's dot) until the chat is
        # opened, which sends `seen`; the window posts an OS notification
        # for it. The rig proves it can see what it asserts: the daemon's
        # own history (dunst on Linux) must hold the alert the window says
        # it posted — a notification check that cannot fail is no check.
        try:
            self.app.click(f"tab-{ix}"); time.sleep(0.6)
            # The chat the line goes to is the one in front now (after the
            # fork check it may be the copy, also a root); find it by id.
            sent_in = self.state().get("active_session")
            def root_of(st):
                pr = next((p for p in st["projects"] if p["index"] == ix), {})
                chats = pr.get("sessions", [])
                mine = next((c for c in chats if c.get("id") == sent_in), None)
                return pr, mine or next((c for c in chats if c.get("parent") is None), {})
            _, root0 = root_of(self.state())
            seen_at_send = root0.get("seen_through") or 0
            posted_before = len(self.state().get("notifications", {}).get("posted", []))
            # A nonce in the reply: dunst keeps a capped history (20), so a
            # repeat of the same words could be an old entry.
            nonce = f"nc{int(time.time()) % 100000}"
            self.send(f"Run the shell command `sleep 6` and then reply with exactly: notification check {nonce}.")
            # The turn must have opened before "idle" means anything: on a
            # loaded box the kernel's first frame comes past the half
            # second the tab click takes, wait_idle saw nothing running
            # and the row read unseen=0 before the reply had landed (R31,
            # the notify trio on cycle-41c and 44c).
            self.wait(lambda s: busy(s), 20, what="turn start")
            time.sleep(0.3); self.app.click("tab-0"); time.sleep(0.5)
            self.wait_idle(120); time.sleep(3)
            pr, away = root_of(self.state())
            verdict = "pass" if (away.get("unseen") or 0) >= 1 else ("unverified" if (away.get("seen_through") or 0) > seen_at_send else "fail")
            self.record("notify-unseen", sc, "reply lands while the Home tab is in front", "the project's root chat holds 1+ unseen notification; unverified when a seen from elsewhere consumed it first (F-78)",
                        f"unseen={away.get('unseen')} kinds={away.get('unseen_kinds')} seen_through={away.get('seen_through')}", verdict, self.still("notify-unseen"))
            self.record("notify-tab-dot", sc, "read the project tab while its reply is unseen", "tab_dot true (the badge Cursor draws on a chat with news)",
                        f"tab_dot={pr.get('tab_dot')} unseen={pr.get('unseen')}", "pass" if pr.get("tab_dot") else ("unverified" if verdict == "unverified" else "fail"), "")
            posted = self.state().get("notifications", {}).get("posted", [])
            new_posts = posted[posted_before:]
            # The post's body is the reply's first line; a model may put
            # words before the nonce, so any new post counts and the daemon
            # is checked for that post's own words.
            hit = next((n for n in new_posts if nonce in (n.get("body") or "").lower()), None) or (new_posts[-1] if new_posts else None)
            key = ((hit or {}).get("body") or nonce).strip()[:40].lower()
            # notify-send hands the alert to the daemon a beat after the
            # window records the post; give the daemon a moment.
            dunst_after, seen_by_daemon = None, False
            for _ in range(10):
                dunst_after = dunst_history()
                seen_by_daemon = dunst_after is not None and any(key in e.lower() or nonce in e.lower() for e in dunst_after)
                if seen_by_daemon or dunst_after is None:
                    break
                time.sleep(0.5)
            if hit and hit.get("error"):
                os_verdict, why = "fail", f"the window could not start {self.state()['notifications'].get('notifier')}: {hit['error']}"
            elif hit and dunst_after is None:
                os_verdict, why = "unverified", "posted, but no dunstctl on this rig to confirm the daemon got it"
            elif hit and seen_by_daemon:
                os_verdict, why = "pass", f"posted {hit.get('title')!r} and dunst's history holds it"
            elif hit:
                os_verdict, why = "fail", "the window says it posted, dunst's history has nothing new — the rig cannot see what it asserts"
            else:
                os_verdict, why = ("unverified" if verdict == "unverified" else "fail"), f"no OS notification posted for the reply (posted={len(new_posts)} new)"
            self.record("notify-os-posted", sc, "the reply lands with the Home tab in front", "an OS notification is posted and the daemon's history shows it", why, os_verdict, "")
            self.app.click(f"tab-{ix}"); time.sleep(2.0)
            pr2, back = root_of(self.state())
            self.record("notify-seen", sc, "open the project's chat", "unseen 0, tab_dot false; seen_through advanced (seen sent to the kernel)",
                        f"unseen={back.get('unseen')} tab_dot={pr2.get('tab_dot')} seen_through={back.get('seen_through')}", "pass" if back.get("unseen") == 0 and not pr2.get("tab_dot") and (back.get("seen_through") or 0) >= 1 else "fail", self.still("notify-seen"))
        except Exception as err:
            self.record("notify", sc, "reply on another tab, then open", "-", f"{type(err).__name__}: {err}", "fail")
        if self.app.exists("tab-sheet-done"):
            self.inv("tab-sheet")
            g = self.first("tab-sheet-glyph-*"); c = self.first("tab-sheet-color-*")
            if g: self.check("tab-sheet-glyph", "tab-sheet", "click a glyph", "selection changes (visual)", lambda: self.app.click(g), None)
            if c: self.check("tab-sheet-color", "tab-sheet", "click a colour", "selection changes (visual)", lambda: self.app.click(c), None)
            self.check("tab-sheet-cancel", "tab-sheet", "click Cancel", "sheet closes", lambda: self.app.click("tab-sheet-cancel"), lambda a, b: not self.app.exists("tab-sheet-done"))
            self.app.double_click(f"tab-{ix}"); time.sleep(0.6)
            if self.app.exists("tab-sheet-done"):
                self.check("tab-sheet-done", "tab-sheet", "click Done", "sheet closes", lambda: self.app.click("tab-sheet-done"), lambda a, b: not self.app.exists("tab-sheet-done"))
        else:
            self.escape()
        self.check("tab right-click", sc, "right-click project tab", "tab menu opens (menu_open)", lambda: self.app.right_click(f"tab-{ix}"), lambda a, b: b.get("menu_open") is True)
        if self.state().get("menu_open"):
            self.inv("tab-menu")
        self.escape()
        # Opener via +.
        self.check("new-tab", sc, "click +", "opener opens (opener_open)", lambda: self.app.click("new-tab"), lambda a, b: b.get("opener_open") is True)
        if self.state().get("opener_open"):
            self.inv("opener-step-1")
            rows = self.ids("opener-row-*")
            self.record("opener-step-1", "opener", "read rows", "machine rows listed", f"{len(rows)} rows", "pass" if rows else "fail", self.still("opener-step-1"))
            if rows:
                self.check("opener-row-0", "opener", "click first machine", "step 2: folder rows", lambda: self.app.click(rows[0]),
                           lambda a, b: b.get("opener_open") and f"{len(self.ids('opener-row-*'))} folder rows", settle=1.5)
                self.inv("opener-step-2")
                self.check("opener keyboard", "opener", "type 'par', down, escape", "filter narrows rows, escape closes",
                           lambda: (self.app.type("par"), self.app.key("down")), lambda a, b: (len(self.ids('opener-row-*')) > 0 and f"{len(self.ids('opener-row-*'))} rows after filter") or "unverified: no rows after filter")
                if self.app.exists("opener-grip"):
                    self.check("opener-grip", "opener", "drag grip 60 px down", "no error", lambda: self.app.drag("opener-grip", (400, 600)), None)
            self.check("opener escape", "opener", "escape", "opener closes", self.escape, lambda a, b: b.get("opener_open") is False)
        self.check("cmd-t", sc, "cmd-t", "opener opens", lambda: self.app.key("cmd-t"), lambda a, b: b.get("opener_open") is True); self.escape()
        self.check("cmd-o", sc, "cmd-o", "opener opens", lambda: self.app.key("cmd-o"), lambda a, b: b.get("opener_open") is True); self.escape()
        self.opener_home_checks(sc)
        self.project_page_checks()
        # Close the Home tab and reopen through the opener.
        n = len(self.state()["projects"])
        self.check("tab-close-0", sc, "hover Home tab, click its close mark", "projects count -1",
                   lambda: (self.app.hover("tab-0"), self.app.click("tab-close-0")), lambda a, b: len(b["projects"]) == n - 1, settle=1.5)
        self.check("cmd-w", sc, "cmd-w with one tab left", "closes the tab in front (projects -1) or refuses", lambda: self.app.key("cmd-w"),
                   lambda a, b: f"projects {len(a['projects'])} -> {len(b['projects'])}", settle=1.5)
        if not self.state()["projects"] or self.project_index() is None:
            # Reopen the sample project through the opener.
            self.app.key("cmd-t"); time.sleep(1)
            rows = self.ids("opener-row-*")
            if rows:
                self.app.click(rows[0]); time.sleep(1)
                self.app.type(str(PROJ)); time.sleep(0.5); self.app.key("enter"); time.sleep(2)
            self.record("reopen", "opener", "reopen sample project by typed path", "project tab back", f"projects={len(self.state()['projects'])} ix={self.project_index()}", "pass" if self.project_index() is not None else "fail", self.still("reopen"))
            self.go_project()
            try:
                self.app.wait_element("composer-field", timeout=8, reachable=True)
            except Exception:
                self.app.key("cmd-n")

    def opener_home_checks(self, sc: str) -> None:
        """Jacob's Mac: `~/Code` listed 'no folders', `~` alone nothing. The
        opener must read `~` as the home, list a folder's children, offer to
        create a path that is not there, and open the expanded path."""
        home = Path.home()
        probe = home / "parity-opener-tmp"
        shutil.rmtree(probe, ignore_errors=True)
        (probe / "alpha").mkdir(parents=True); (probe / "alpha2").mkdir(); (probe / "beta").mkdir()
        try:
            # Typed at the machine step, as Jacob did: a path there means
            # this machine and steps into the folder list with the text kept.
            self.app.key("cmd-t"); time.sleep(0.6)
            self.app.type("~"); time.sleep(1.0)
            n_home = len(self.ids("opener-row-*"))
            self.record("opener-tilde", "opener", "type ~", "the home folder's children listed (rows > 1)", f"{n_home} rows", "pass" if n_home > 1 else "fail", self.still("opener-tilde"))
            self.app.type("/parity-opener-tmp/"); time.sleep(1.0)
            n_probe = len(self.ids("opener-row-*"))
            self.record("opener-tilde-path", "opener", "type ~/parity-opener-tmp/", "Open parity-opener-tmp + alpha/ + alpha2/ + beta/ (4 rows)", f"{n_probe} rows", "pass" if n_probe == 4 else "fail", self.still("opener-tilde-path"))
            # Jacob's Mac: a folder whose name is a prefix of a sibling
            # (`Code` beside `Code2`) could not be opened without its slash;
            # Enter only completed the text. The first row is "Open alpha".
            self.app.type("alpha"); time.sleep(1.0)
            n_pre = len(self.ids("opener-row-*"))
            self.record("opener-prefix-sibling-rows", "opener", "type ~/parity-opener-tmp/alpha (alpha2 beside it)", "Open alpha + alpha2/ (2 rows)", f"{n_pre} rows", "pass" if n_pre == 2 else "fail", self.still("opener-prefix-sibling"))
            before = {p["path"] for p in self.state()["projects"]}
            self.app.key("enter"); time.sleep(2.0)
            after = {p["path"] for p in self.state()["projects"]}
            opened = [p for p in after - before if p.rstrip("/").endswith("parity-opener-tmp/alpha")]
            self.record("opener-prefix-sibling-open", "opener", "Enter with the text naming alpha whole", "a tab opens on alpha itself (not alpha2, not a completion)", f"new={sorted(after - before)} opener_open={self.state().get('opener_open')}", "pass" if opened and not self.state().get("opener_open") else "fail", self.still("opener-prefix-sibling-open"))
            if self.app.exists("tab-sheet-done"): self.app.key("escape"); time.sleep(0.6)
            ix = next((p["index"] for p in self.state()["projects"] if p["path"].rstrip("/").endswith("parity-opener-tmp/alpha")), None)
            if ix is not None:
                self.app.hover(f"tab-{ix}"); self.app.click(f"tab-close-{ix}"); time.sleep(1.0)
            # A folder row is a step in, never an open: Enter on `alpha/`
            # (two matches for `al`, no whole name) fills the text with
            # its slash and lists its children.
            self.app.key("cmd-t"); time.sleep(0.6)
            self.app.type("~/parity-opener-tmp/al"); time.sleep(1.0)
            n_al = len(self.ids("opener-row-*"))
            self.app.key("enter"); time.sleep(1.0)
            stepped = self.state().get("opener_open") and n_al == 2
            self.record("opener-dir-row-steps-in", "opener", "type ~/parity-opener-tmp/al, Enter on alpha/", "2 folder rows; Enter steps into alpha/ (opener stays open, one Open row)", f"rows={n_al} opener_open={self.state().get('opener_open')} rows_after={len(self.ids('opener-row-*'))}", "pass" if stepped and len(self.ids("opener-row-*")) == 1 else "fail", self.still("opener-dir-row-steps-in"))
            self.escape(); time.sleep(0.5)
            self.app.key("cmd-t"); time.sleep(0.6)
            self.app.type("~/parity-opener-tmp/newone"); time.sleep(1.0)
            n_new = len(self.ids("opener-row-*"))
            self.record("opener-create-offer", "opener", "type a name that is not there", "one row: Create ~/parity-opener-tmp/newone", f"{n_new} rows", "pass" if n_new == 1 else "fail", self.still("opener-create-offer"))
            before = {p["path"] for p in self.state()["projects"]}
            self.app.key("enter"); time.sleep(2.0)
            after = {p["path"] for p in self.state()["projects"]}
            made = (probe / "newone").is_dir()
            opened = any(p.rstrip("/").endswith("parity-opener-tmp/newone") for p in after - before)
            self.record("opener-create-open", "opener", "Enter on Create", "folder made under the real home; a tab opens on it (path expanded, no literal ~)",
                        f"made={made} opened={opened} new={sorted(after - before)}", "pass" if made and opened else "fail", self.still("opener-create-open"))
            # The new, empty project lands on the kickoff view, never the
            # Project page (Jacob's third bug). The view comes with the
            # kernel's first frame; on a loaded box that is past two
            # seconds (R31: four full gates read it absent, by hand it was
            # there at t+2 s every time).
            self.wait(lambda s: self.seen("kickoff") or self.app.exists("kickoff-greeting"), 12, what="kickoff view")
            st = self.state()
            self.record("new-project-kickoff", "new-project", "after the opener opens an empty folder", "pane chat; kickoff view (project head + greeting); composer asks what you are working on",
                        f"pane={st.get('pane')} kickoff={self.app.exists('kickoff')} greeting={self.app.exists('kickoff-greeting')} changes_pill={self.app.exists('pill-changes')} branch={self.app.exists('composer-branch')}",
                        "pass" if st.get("pane") == "chat" and self.seen("kickoff") and not self.app.exists("pill-changes") else "fail", self.still("new-project-kickoff"))
            # Close that tab again.
            ix = next((p["index"] for p in st["projects"] if p["path"].rstrip("/").endswith("newone")), None)
            if ix is not None:
                self.app.hover(f"tab-{ix}"); self.app.click(f"tab-close-{ix}"); time.sleep(1.0)
        finally:
            self.escape()
            shutil.rmtree(probe, ignore_errors=True)

    def project_page_checks(self) -> None:
        """Every way back from the Project page (Jacob could find none):
        the labelled control, Escape, ⌘1, the tab; and Start the page…
        lands in the chat with a prompt in the composer."""
        sc = "project-page"
        # Since the side-panel rewrite the Project page is the panel's
        # Project tab, closed by default: open the drawer first, and when
        # there is still no full-pane page, drive the ways out of the tab
        # instead (R25 — these rows read not-reachable in cycle 36 and said
        # nothing; Jacob's -24 asked for a clear way out of the page).
        panel = lambda: (self.state().get("panel") or {})
        if not self.app.exists("panel-project-head") and self.app.exists("toggle-panel") and not panel().get("open"):
            self.app.click("toggle-panel"); time.sleep(1.0)
        if self.app.exists("panel-tab-0"):
            tabs = panel().get("tabs") or []
            active = panel().get("active")
            self.record("panel-project-tab", sc, "open the drawer", "the Project tab is the panel's first tab and is active; the chat stays",
                        f"tabs={[t.get('kind') for t in tabs]} active={active} pane={self.state().get('pane')}",
                        "pass" if tabs and tabs[0].get("kind") == "project" and active == 0 and self.state().get("pane") == "chat" else "fail", self.still("panel-project-tab"))
            self.record("project-stays-in-panel", sc, "open Project", "no Project page over the chat, no Back to chat",
                        f"pane={self.state().get('pane')} back={self.app.exists('page-back-to-chat')}",
                        "pass" if self.state().get("pane") == "chat" and not self.app.exists("page-back-to-chat") else "fail",
                        self.still("project-stays-in-panel"))
            self.check("panel-escape-closes", sc, "Escape with the drawer open", "the drawer closes; the chat stays",
                       lambda: self.app.key("escape"), lambda a, b: (b.get("panel") or {}).get("open") is False and b.get("pane") == "chat")
            self.app.key("cmd-b"); time.sleep(0.8)
            if self.app.exists("toggle-panel"):
                self.check("toggle-panel-closes", sc, "click the strip's panel toggle", "the drawer closes",
                           lambda: self.app.click("toggle-panel"), lambda a, b: (b.get("panel") or {}).get("open") is False)
            else:
                self.gap("toggle-panel-closes", sc, "click", "no panel toggle on the tab strip")
            if self.app.exists("panel-close") or self.app.exists("panel-expand"):
                self.record("removed-panel-chrome", sc, "drawer open", "no header X and no expand grid in the panel",
                            f"close={self.app.exists('panel-close')} expand={self.app.exists('panel-expand')}",
                            "fail", self.still("removed-panel-chrome"))
            else:
                self.record("removed-panel-chrome", sc, "drawer open", "no header X and no expand grid in the panel",
                            "close and expand gone from the panel", "pass", self.still("removed-panel-chrome"))
            if self.app.exists("window-expand"):
                self.record("window-expand-pinned", sc, "read the tab strip", "one expand control at the window's top-right",
                            "window-expand on the strip", "pass", self.still("window-expand-pinned"))
            else:
                self.record("window-expand-pinned", sc, "read the tab strip", "one expand control at the window's top-right",
                            "missing", "fail", self.still("window-expand-pinned"))
            if self.app.exists("chat-clear"):
                self.record("chat-clear-gone", sc, "read the chat header", "no Clear button; typed clear / /clear still work",
                            "chat-clear still drawn", "fail", self.still("chat-clear-gone"))
            else:
                self.record("chat-clear-gone", sc, "read the chat header", "no Clear button; typed clear / /clear still work",
                            "gone", "pass", self.still("chat-clear-gone"))
            return
        self.gap("project-page-back", sc, "-", "no panel-tab-0 on this layout")

    def phase_panel(self) -> None:
        if not self.tabs:
            self.gap("panel", "right-panel", "-", "no right panel on this branch; sidebar covered in tab-bar phase")
            if self.app.exists("context-panel"):
                self.inv("context-panel")
                t = self.first("context-task-*")
                if t:
                    self.check("context-task", "context-panel", "click task row", "active_session changes", lambda: self.app.click(t), lambda a, b: b["active_session"] != a["active_session"])
            return
        sc = "right-panel"
        self.inv(sc)
        # `panel_shown` is what is on screen; `panel_open` is the wish (true
        # at 900 wide while nothing is drawn, F-161). The row asserts on
        # both: the wish flips and the drawing follows, or says why not.
        def flips(a, b):
            drawn = "panel_shown" in b
            ok = a["panel_open"] != b["panel_open"] and (not drawn or b.get("panel_shown") == b["panel_open"] or f"shown={b.get('panel_shown')} (window too narrow for the drawer)")
            return ok and f"open {a['panel_open']}->{b['panel_open']} shown={b.get('panel_shown')}"
        self.check("toggle-panel", sc, "click", "panel_open flips and panel_shown follows", lambda: self.app.click("toggle-panel"), flips)
        self.check("cmd-b", sc, "cmd-b", "panel_open flips back and panel_shown follows", lambda: self.app.key("cmd-b"), flips)
        if not self.state()["panel_open"]:
            self.app.key("cmd-b")
        if self.app.exists("panel-set-goals"):
            self.check("panel-set-goals", sc, "click Set goals…", "composer holds a goals prompt, nothing sent",
                       lambda: self.app.click("panel-set-goals"), lambda a, b: "GOALS" in b["composer"]["text"] and not busy(b))
            self.clear_composer()
        else:
            self.gap("panel-set-goals", sc, "click", "not shown (goals exist?)")
        if self.app.exists("panel-add-note"):
            self.check("panel-add-note", sc, "click", "composer holds a notes prompt", lambda: self.app.click("panel-add-note"), lambda a, b: b["composer"]["text"] != "")
            self.clear_composer()
        for pat in ("panel-surface-*", "panel-standing-*"):
            f = self.first(pat)
            if f:
                self.check(pat, sc, "click", "surface / node focused", lambda f=f: self.app.click(f), lambda a, b: self.diff(a, b) or "unverified: click accepted, nothing changed")
            else:
                self.gap(pat, sc, "click", "no such row in this run (no processes / standing nodes)")
        if self.app.exists("new-subchat"):
            self.record("new-subchat", sc, "bottom +", "the bottom + is gone",
                        "new-subchat still drawn", "fail", self.still("new-subchat"))
        else:
            self.record("new-subchat", sc, "bottom +", "the bottom + is gone; ⌘N still opens a sub-chat",
                        "gone", "pass", self.still("new-subchat"))
        self.check("cmd-n", sc, "cmd-n", "a child session", lambda: self.app.key("cmd-n"), lambda a, b: len(sessions(b)) == len(sessions(a)) + 1 and (active(b) or {}).get("parent") is not None)
        self.check("alt-cmd-up", sc, "alt-cmd-up", "steps to the previous agent in the tree", lambda: self.app.key("alt-cmd-up"), lambda a, b: b["active_session"] != a["active_session"])
        self.check("alt-cmd-down", sc, "alt-cmd-down", "steps to the next agent", lambda: self.app.key("alt-cmd-down"), lambda a, b: b["active_session"] != a["active_session"])
        self.check("panel-scroll", sc, "scroll the panel", "no error", lambda: self.app.scroll("panel-scroll", dy=-200), None)
        rows = self.ids("panel-agent-*")
        if rows and self.reveal(rows[0], "panel-scroll"):
            self.app.click(rows[0])

    def phase_settings(self) -> None:
        # The eyesight check (`weight-visible`) compares the main window's
        # prose before and after the bionic toggle: it needs a chat with a
        # real answer in front, not a New chat's static greeting, which is
        # no transcript item and takes no weight (cycle 34b read 0 pixels
        # and called it a fail). The main chat has answers by now.
        self.go_project()
        if not any(i.get("kind") == "agent" for i in (active(self.state()) or {}).get("items", [])):
            for row in self.ids("panel-agent-*"):
                # A row scrolled out of the drawer (a run that opened many
                # chats) is brought back or skipped, never a crash that
                # takes the phase with it (cycle 37: phases R and W died on
                # `panel-agent-27 … clipped away`).
                if not self.reveal(row, "panel-scroll"):
                    continue
                self.app.click(row); time.sleep(0.5)
                if any(i.get("kind") == "agent" for i in (active(self.state()) or {}).get("items", [])):
                    break
        """Settings is a tab of this window, not a window of its own: the gear
        opens it in the strip, it fills the width, and every route back out of
        it is driven here — Escape, ⌘1, the pill's close mark, ⌘W."""
        sc = "settings"
        gear = "status-bar-settings"
        SETTINGS_ROWS = ["section-*", "model-pick-*", "appearance-*", "watch-bounce-*", "update-channel-*", "size-up", "size-down", "reduce-transparency", "cursor-blink", "bionic-reading", "weight-visible", "cmd-,"]
        if not self.app.exists(gear):
            self.gap("settings", sc, "click gear", "no gear on the bar under the window")
            return
        # A closed tab first, so the gear's own check is the one that opens it.
        if self.state().get("settings_open"):
            self.app.key("cmd-w"); time.sleep(0.8)
        self.check("status-bar-settings", sc, "click the gear", "the Settings tab opens in the strip and comes to front",
                   lambda: self.app.click(gear),
                   lambda a, b: b.get("settings_open") is True and b.get("front") == "settings", settle=1.5)
        if not self.state().get("settings_open"):
            self.stop_phase(sc, "the gear did not open settings", SETTINGS_ROWS, fault=True)
            return
        # One window. The whole point of the change is that there is no second
        # one, so this is asserted rather than assumed.
        wins = [w.get("kind") for w in self.app.windows()]
        self.record("windows", sc, "driver windows()", "one window: Settings is a tab, not a window",
                    json.dumps(wins), "pass" if wins == ["main"] else "fail", self.still("settings-one-window"))
        found = self.inv("settings")
        self.record("tab-settings", sc, "the strip with Settings open", "a Settings pill with its own close mark, after the project tabs",
                    f"pill={'tab-settings' in found} close={'tab-settings-close' in found}",
                    "pass" if "tab-settings" in found and "tab-settings-close" in found else "fail")
        try:
            secs = self.ids("section-*")
            for s_ in secs:
                name = s_.rsplit(".", 1)[-1]
                self.check(name, "settings", "click section", "section body changes (element set differs)",
                           lambda s_=s_: self.app.click(s_), lambda a, b: (len(self.ids()) > 0 and f"{len(self.ids())} interactive ids in section") or "unverified: nothing listed", settle=0.6)
                self.inv(f"settings-{name}")
                # Only the tab's own controls. Settings shares the window
                # now, so a bare ids() lists the strip and the bar too — and
                # the walk clicked `tab-0` then `tab-close-0`, closing the
                # project tab under itself (cycle 35: "No tab open", every
                # phase after it without a composer). Rig audit R22.
                for el in [e for e in self.ids() if ".settings-body." in e]:
                    short = el.rsplit(".", 1)[-1]
                    # Bring a control below the fold onto the screen before
                    # touching it: the driver refuses a click on what is not
                    # visible (R13), and two settings controls read
                    # not-reachable for that alone (R24's next step).
                    self.reveal(el, "settings-body")
                    if short.startswith("section-") or short in ("settings-body", "settings-back-to-chat", "tab-settings", "tab-settings-close"):
                        continue
                    if short == "bionic-reading":
                        def flag():
                            return self.state().get("bionic_reading")
                        # F-53: the rig must be able to *see* a weight change,
                        # not just the flag — the bundled Inter carries the
                        # semibold the fixation letters are set in.
                        def prose_pixels():
                            # The chat's prose is behind the Settings tab now,
                            # so step back to the chat, photograph the window,
                            # and return to the tab on the section it was on.
                            # Nothing to mask: there is only one window.
                            def geo(name):
                                ids = subprocess.run(["xdotool", "search", "--name", name], capture_output=True, text=True, env=ENV).stdout.split()
                                if not ids:
                                    return None
                                g = dict(line.split("=", 1) for line in subprocess.run(["xdotool", "getwindowgeometry", "--shell", ids[0]], capture_output=True, text=True, env=ENV).stdout.split())
                                return int(g["X"]), int(g["Y"]), int(g["WIDTH"]), int(g["HEIGHT"])
                            main = geo("^Arbos$")
                            if not main:
                                return None
                            self.app.key("cmd-1"); time.sleep(0.5)
                            try:
                                self.app.hover("composer-field"); time.sleep(0.4)
                                shot = self.outdir / "weight-probe.png"
                                subprocess.run(["scrot", "-o", str(shot)], env=ENV, check=True)
                                im = Image.open(shot).convert("L")
                                x, y, w, h = main
                                return im.crop((x, y, x + w, y + h))
                            finally:
                                self.app.click("tab-settings"); time.sleep(0.5)
                        plain = prose_pixels()
                        self.app.click(el); time.sleep(0.6); on = flag()
                        self.record("bionic-reading", "settings", "click the toggle on", "bionic_reading true in the driver state", f"bionic_reading={on}", "pass" if on is True else "fail")
                        weighted = prose_pixels()
                        if plain is not None and weighted is not None and plain.size == weighted.size:
                            diff = ImageChops.difference(plain, weighted).tobytes()
                            changed = sum(1 for v in diff if v > 40); total = max(1, plain.size[0] * plain.size[1])
                            self.record("weight-visible", "settings", "compare the chat before/after the toggle (⌘1 out, the pill back in)", "the rig sees the weight change (> 0.2 % of the window's pixels differ)",
                                        f"{changed}/{total} pixels differ ({100.0 * changed / total:.2f} %)", "pass" if changed > 0.002 * total else "fail", self.still("weight-visible"))
                        else:
                            self.record("weight-visible", "settings", "compare prose before/after the toggle", "-", "no prose paragraph on screen to compare", "unverified")
                        self.app.click(el); time.sleep(0.6); off = flag()
                        self.record("bionic-reading", "settings", "click the toggle off again", "bionic_reading false", f"bionic_reading={off}", "pass" if off is False else "fail")
                        continue
                    if short in ("key-forget", "config-reveal", "key-paste"):
                        self.skip(short, "settings", "click", "destructive or opens a file manager / pastes clipboard into the key field")
                        continue
                    if short == "commit":
                        self.skip(short, "settings", "click", "opens the commit URL in the system browser (verified once: Chrome opened)")
                        continue
                    if short.startswith("provider-"):
                        self.skip(short, "settings", "click", "rewrites config.toml for the rest of the run; selection state is not in the driver dump")
                        continue
                    # The values these controls set live in the window's own
                    # state (`appearance`, `reduce_transparency`, `cursor_blink`,
                    # `watch_bounce`, `update_channel`, `text_size`); a model
                    # pick lands in config.toml. Read those, not the click
                    # (rig audit R3, cycle 32: these rows were `unverified`).
                    def settled(el, field, want=None, action="click", changed_ok=False):
                        a = self.state().get(field)
                        self.app.click(el); time.sleep(0.6)
                        b = self.state().get(field)
                        if want is not None:
                            self.record(short, "settings", action, f"{field} == {want!r}", f"{field}: {a!r} → {b!r}", "pass" if b == want else "fail", self.still(short))
                        elif changed_ok and b != a:
                            self.record(short, "settings", action, f"{field} changes", f"{field}: {a!r} → {b!r}", "pass", self.still(short))
                        elif changed_ok:
                            self.record(short, "settings", action, f"{field} changes", f"{field} unchanged at {a!r} (already the selection?)", "unverified", self.still(short))
                        return a, b
                    def flips(el, field):
                        a = self.state().get(field)
                        self.app.click(el); time.sleep(0.5)
                        mid = self.state().get(field)
                        self.app.click(el); time.sleep(0.5)
                        b = self.state().get(field)
                        ok = isinstance(a, bool) and mid == (not a) and b == a
                        self.record(short, "settings", "click toggle twice", f"{field} flips and flips back", f"{field}: {a!r} → {mid!r} → {b!r}", "pass" if ok else "fail", self.still(short))
                    if short.startswith("model-pick-"):
                        cfg = self.xdg / "arbos" / "config.toml"
                        line = lambda: next((l.strip() for l in cfg.read_text().splitlines() if l.strip().startswith("model")), None) if cfg.exists() else None
                        a = line(); self.app.click(el); time.sleep(0.6); b = line()
                        if b is not None and b != a:
                            self.record(short, "settings", "click", "config.toml's model line changes to the pick", f"{a} → {b}", "pass", self.still(short))
                        else:
                            self.record(short, "settings", "click", "config.toml's model line changes to the pick", f"model line unchanged: {b} (already the pick?)", "unverified", self.still(short))
                        continue
                    if short.startswith("appearance-"):
                        settled(el, "appearance", changed_ok=True)
                        continue
                    if short.startswith("watch-bounce-"):
                        settled(el, "watch_bounce", want=int(short.rsplit("-", 1)[-1]))
                        continue
                    if short.startswith("update-channel-"):
                        settled(el, "update_channel", want=short.rsplit("-", 1)[-1])
                        continue
                    if short in ("size-up", "size-down"):
                        a, b = settled(el, "text_size", changed_ok=True)
                        if isinstance(a, (int, float)) and isinstance(b, (int, float)) and b != a:
                            grew = b > a
                            self.record(short, "settings", "direction", "+ grows, − shrinks", f"{a} → {b}", "pass" if grew == (short == "size-up") else "fail")
                        continue
                    if short in ("reduce-transparency", "cursor-blink"):
                        flips(el, short.replace("-", "_"))
                        continue
                    if short in ("commit", "meter"):
                        self.check(short, "settings", "click", "copies / no-op", lambda el=el: self.app.click(el), None)
                        continue
                    self.check(short, "settings", "click", "no error", lambda el=el: self.app.click(el), None)
        except Exception as err:
            self.record("settings", sc, "walk controls", "-", f"{type(err).__name__}: {err}", "fail")
        self.phase_settings_ways_out()

    def phase_settings_ways_out(self) -> None:
        """Every route out of the Settings tab, driven one at a time. Jacob has
        been stuck in a full-width surface before — he opened the Project page
        and could not close it — so each of these is a check, not a comment."""
        sc = "settings"
        if not self.state().get("settings_open"):
            self.app.key("cmd-,"); time.sleep(1.2)
        if not self.state().get("settings_open"):
            self.gap("settings-ways-out", sc, "cmd-,", "the Settings tab would not open")
            return
        chat = lambda b: b.get("front") == "project" and b.get("settings_open") is True
        self.check("settings-escape", sc, "Escape with the Settings tab in front", "back to the chat; the tab stays in the strip",
                   lambda: self.app.key("escape"), lambda a, b: chat(b), settle=1.0)
        self.check("tab-settings", sc, "click the Settings pill again", "Settings comes forward on the section it was left on",
                   lambda: self.app.click("tab-settings"), lambda a, b: b.get("front") == "settings", settle=1.0)
        self.check("cmd-1", sc, "⌘1 with the Settings tab in front", "back to the chat; the tab stays",
                   lambda: self.app.key("cmd-1"), lambda a, b: chat(b), settle=1.0)
        self.check("settings-back-to-chat", sc, "the rail's Back to chat row", "back to the chat, without a chord",
                   lambda: (self.app.click("tab-settings"), time.sleep(0.6), self.app.click("settings-back-to-chat")),
                   lambda a, b: chat(b), settle=1.0)
        self.check("tab-settings-close", sc, "the pill's close mark", "the tab leaves the strip; the composer has the keyboard",
                   lambda: (self.app.click("tab-settings"), time.sleep(0.6), self.app.click("tab-settings-close")),
                   lambda a, b: b.get("settings_open") is False and b.get("front") == "project" and b["composer"]["focused"], settle=1.2)
        self.check("cmd-,", sc, "⌘, from the chat", "the Settings tab opens and comes to front",
                   lambda: self.app.key("cmd-,"), lambda a, b: b.get("settings_open") is True and b.get("front") == "settings", settle=1.5)
        self.check("settings-cmd-w", sc, "⌘W with the Settings tab in front", "the tab closes and the project tab is untouched",
                   lambda: self.app.key("cmd-w"),
                   lambda a, b: b.get("settings_open") is False and len(b["projects"]) == len(a["projects"]), settle=1.2)

    def phase_world(self, kernel: str) -> None:
        """After-failure states — what the window says when the world outside
        it changed (coverage row added cycle 33; QA found `af-03` here and
        the rotation had no row). Two changes on a scratch place of its own:
        the folder renamed under the kernel, and the kernel's file replaced
        under a running turn. Last in the order: it copies the kernel binary
        and serves the scratch place from the copy."""
        sc = "world"
        home = Path.home()
        place = home / "parity-world-tmp" / f"w{int(time.time()) % 100000}"
        moved = place.with_name(place.name + "-moved")
        shutil.rmtree(place.parent, ignore_errors=True)
        place.mkdir(parents=True)
        (place / "README.md").write_text("# world\n")
        def root_of(path):
            for p in self.state()["projects"]:
                if p["path"].rstrip("/") == str(path).rstrip("/"):
                    for c in p["sessions"]:
                        if c.get("parent") is None:
                            return c
            return None
        def notices(c):
            return [i.get("text", "") for i in (c or {}).get("items", []) if i.get("kind") == "notice"]
        def users(c):
            return [i.get("text", "") for i in (c or {}).get("items", []) if i.get("kind") == "user"]
        def kernel_pids(path):
            out = subprocess.run(["pgrep", "-f", f"arbos-kernel[-0-9a-z]* serve {path}$"], capture_output=True, text=True).stdout.split()
            return out
        try:
            # Open the scratch place by typed path and let its kickoff settle.
            self.app.key("cmd-t"); time.sleep(0.8)
            self.app.type(str(place)); time.sleep(1.2); self.app.key("enter"); time.sleep(2.5)
            if self.app.exists("tab-sheet-done"):
                self.app.key("escape"); time.sleep(0.6)
            opened = self.wait(lambda s: root_of(place) is not None, 20, what="world place open")
            if not opened:
                self.stop_phase(sc, "the opener did not open the scratch place", ["world-moved-notice", "world-moved-line-kept", "world-moved-second-line", "world-back-in-order", "world-deleted-build-plate", "world-deleted-build-click"], fault=True)
                return
            self.wait(lambda s: (lambda c: c and not (c.get("streaming") or c.get("turn_open")) and any(i.get("kind") == "agent" for i in c.get("items", [])))(root_of(place)), 90, what="world kickoff")
            # --- the folder moves under the kernel (af-03) ---
            place.rename(moved)
            time.sleep(2)
            self.send("Reply with exactly: after the move.")
            time.sleep(2.5)
            c = root_of(place)
            n = notices(c)
            said = [t for t in n if t.startswith("This project's folder is gone or was moved") and str(place) in t]
            self.record("world-moved-notice", sc, "rename the folder, type a line", "one notice: folder gone or moved, naming the expected path",
                        f"notices={len(n)}; named={bool(said)}; text={said[0][:120] if said else n[-1][:120] if n else '-'}", "pass" if len(said) == 1 else "fail", self.still("world-moved"))
            self.record("world-moved-line-kept", sc, "the typed line's fate", "its card is on the pane and the composer is empty; not 'archived'",
                        f"users={len(users(c))} composer={self.state()['composer']['text']!r} archived-word={any('archived' in t for t in n)}",
                        "pass" if users(c) and users(c)[-1].endswith("after the move.") and not self.state()["composer"]["text"] and not any("archived" in t for t in n) else "fail")
            self.send("And this one too.")
            time.sleep(2)
            c = root_of(place)
            self.record("world-moved-second-line", sc, "a second line into the moved place", "a second card, still one notice",
                        f"users={len(users(c))} notices={len(notices(c))}", "pass" if len(users(c)) >= 2 and len(notices(c)) == 1 else "fail")
            # --- the folder comes back: held lines go first, in order ---
            moved.rename(place)
            time.sleep(2)
            self.send("Reply with exactly: back again.")
            settled = self.wait(lambda s: (lambda c: c and not (c.get("streaming") or c.get("turn_open")) and len([i for i in c.get("items", []) if i.get("kind") == "agent"]) >= 4)(root_of(place)), 150, what="world held lines answered")
            c = root_of(place)
            agents = [i.get("text", "") for i in (c or {}).get("items", []) if i.get("kind") == "agent"]
            order = [next((k for k, key in enumerate(("after the move", "this one too", "back again")) if key in a), None) for a in agents[1:]]
            order = [o for o in order if o is not None]
            self.record("world-back-in-order", sc, "rename back, type a third line", "three answers, in the order typed, no duplicate prompt cards",
                        f"settled={bool(settled)} answers={order} users={len(users(c))}", "pass" if settled and order == [0, 1, 2] and len(users(c)) == 3 else "fail", self.still("world-back"))
            # --- the kernel's file is replaced under a running turn ---
            copy_dir = Path("/tmp/parity-world-kernel"); shutil.rmtree(copy_dir, ignore_errors=True); copy_dir.mkdir()
            copy = copy_dir / "arbos-kernel"; shutil.copy(kernel, copy)
            for pid in kernel_pids(place):
                subprocess.run(["kill", pid])
            time.sleep(1.5)
            served = subprocess.Popen([str(copy), "serve", str(place)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
            time.sleep(3)
            self.send("Run `sleep 120` yourself with bash right now, attached, no workers.")
            self.wait(lambda s: (lambda c: c and (c.get("streaming") or c.get("turn_open")))(root_of(place)), 30, what="world long turn")
            time.sleep(3)
            tmp = copy_dir / "arbos-kernel.new"; shutil.copy(kernel, tmp); os.replace(tmp, copy)
            control = self.wait(lambda s: bool(self.ids("status-bar-stranger-kernel")) or None, 75, every=2, what="stranger plate")
            ids = self.ids("status-bar-stranger-kernel")
            if not ids:
                # A kernel from before #385 says nothing about its file; the
                # bar cannot know. Said as a gap, not a fail.
                self.gap("world-deleted-build-plate", sc, "replace the kernel's file under a running turn", "no plate within 75 s — this kernel may predate `binary_gone` (#385); read its --version in the first row")
                self.stop_phase(sc, "no stranger plate for the replaced file (a kernel from before #385?)", ["world-deleted-build-click"], fault=False)
                return
            self.record("world-deleted-build-plate", sc, "replace the kernel's file under a running turn", "the bar's plate appears while the turn runs", "plate on the bar", "pass", self.still("world-plate"))
            before = kernel_pids(place)
            self.check("world-deleted-build-click", sc, "click the plate", "the kernel is restarted (new pid), the plate goes, the root chat says so",
                       lambda: self.app.click(ids[0]),
                       lambda a, b: (lambda c: (kernel_pids(place) != before and not self.ids("status-bar-stranger-kernel") and any("restarted on this build" in t for t in notices(c))) and f"pid {before} -> {kernel_pids(place)}; notices {[t[:40] for t in notices(c)[-3:]]}")(root_of(place)),
                       settle=8)
        finally:
            for pid in kernel_pids(place) + kernel_pids(moved):
                subprocess.run(["kill", pid])
            try:
                self.go_project()
            except Exception:
                pass

    def phase_permissions(self) -> None:
        """The permissions sheet from the menu (Arbos › Permissions…): a modal
        over the chat; rows re-read while it shows; Escape or Skip closes and
        never nags again (permissions.seen stays true)."""
        sc = "permissions"
        def perms(st):
            return st.get("permissions") or {}
        self.check("permissions-open", sc, "action arbos::ShowPermissions", "permissions.open true; rows listed",
                   lambda: self.app.call("action", name="arbos::ShowPermissions"),
                   lambda a, b: perms(b).get("open") is True and bool(perms(b).get("rows")) and f"{len(perms(b)['rows'])} rows", settle=1.2)
        st = self.state()
        if perms(st).get("open") and self.app.exists("permissions-enable-all"):
            self.check("permissions-enable-all", sc, "click Enable all", "rows walk Requesting → a settled phase; enabling_all ends false",
                       lambda: self.app.click("permissions-enable-all"),
                       lambda a, b: perms(b).get("enabling_all") is False and all(r["phase"] in ("idle", "needs_settings") for r in perms(b)["rows"]) and "settled", settle=5.0)
        if self.app.exists("mic-test"):
            self.check("mic-test", sc, "click Test in the sheet", "the probe starts (button reads Stop) or an error line says what is missing",
                       lambda: self.app.click("mic-test"), lambda a, b: self.seen("mic-test") and "probe toggled", settle=1.5)
            if self.app.exists("mic-test"):
                self.app.click("mic-test"); time.sleep(0.5)
        self.check("permissions-escape", sc, "Escape", "permissions.open false; seen stays true; composer focused",
                   lambda: self.app.key("escape"),
                   lambda a, b: perms(b).get("open") is False and perms(b).get("seen") is True and b["composer"]["focused"] and "closed", settle=1.2)

    def phase_menus(self) -> None:
        sc = "menus"
        for pat, action in (("chat-header-menu-*", "click header … menu"), ("chat-header-title-*", "right-click header title"),
                            ("chat-title-*", "right-click chat title")):
            f = self.first(pat)
            if not f:
                self.gap(pat, sc, action, "element not on screen"); continue
            do = (lambda f=f: self.app.click(f)) if "click " == action[:6] else (lambda f=f: self.app.right_click(f))
            self.check(pat, sc, action, "menu opens (menu_open true)", do, lambda a, b: b.get("menu_open") is True)
            if self.state().get("menu_open"):
                self.inv(f"menu:{pat}")
            self.escape()
        f = self.first("chat-header-title-name-*") or self.first("chat-title-name-*")
        if f:
            self.check("chat-title double-click", sc, "double-click title", "rename field (renaming true)", lambda: self.app.double_click(f), lambda a, b: b.get("renaming") is True)
            self.escape()
        self.check("conversation-drop", sc, "scroll transcript", "no error", lambda: self.app.scroll("conversation-drop", dy=-300), None)
        f = self.first("transcript-rail-*")
        if f:
            self.check("transcript-rail", sc, "click rail", "scroll position changes (no state field)", lambda: self.app.click(f), None)

    def phase_prs(self) -> None:
        """Parity row 4: the PRs pill, from a real `gh pr create` (the fake gh)."""
        sc = "prs-pill"
        self.go_project()
        ids = self.turn_ids()
        n0 = ((active(self.state()) or {}).get("pills") or {}).get("prs", 0)
        t0 = time.time()
        self.send(P_PR)
        s = self.wait(lambda s: ((active(s) or {}).get("pills") or {}).get("prs", 0) > n0 or (not busy(s) and time.time() - t0 > 20), 180, what="PRs pill")
        self.wait_idle(120)
        pills = (active(self.state()) or {}).get("pills") or {}
        recorded = (PROJ / ".arbos" / "prs.jsonl").read_text().splitlines() if (PROJ / ".arbos" / "prs.jsonl").exists() else []
        self.inv(sc)
        self.record("pill-prs", sc, "worker runs `gh pr create` (fake gh on PATH)", "pills.prs counts the subtree's PR; the pill-prs element shows \"PRs 1\"; .arbos/prs.jsonl has the record",
                    f"pills={json.dumps(pills)[:160]} prs.jsonl lines={len(recorded)} pill element={self.app.exists('pill-prs')}",
                    "pass" if pills.get("prs", 0) > n0 and self.seen("pill-prs") and recorded else ("not-reachable" if not recorded else "fail"), self.still("prs-pill"))
        if self.app.exists("pill-prs"):
            self.check("pill-prs", sc, "hover the pill", "tooltip lists the PR URLs; no state change", lambda: self.app.hover("pill-prs"), None)

    def phase_multitask(self, binary: str, kernel: str, xdg: Path) -> None:
        """Multitasking audit scenarios (internal/qa/inbox/2026-09-13-multitasking-audit.md)."""
        sc = "multitask"
        self.go_project()
        # First-spawn connect: every open chat is live, no 'connection failed' left behind.
        # A closed row (idle) is a chat that was archived or whose agent is
        # gone; it is not waiting on a socket. Only notices this connect
        # adds count: phase D before this one takes the kernel's file away
        # under a running chat, and the notice it earns stays on that
        # transcript (R28 — the row failed on phase D's notice, cycle 39).
        def failed_notices() -> list[tuple[int, str]]:
            return [(c.get("id", 0), it.get("text", "")) for c in project_sessions(self.state()) for it in c.get("items", []) if it.get("kind") == "notice" and "connection failed" in it.get("text", "").lower()]
        before = set(failed_notices())
        s = self.wait(lambda s: all(c["connection"] == "live" for c in sessions(s) if c["connection"] != "idle") and bool(sessions(s)), 25, what="all open sessions live")
        failed = [text for text in failed_notices() if text not in before]
        self.record("connect-first-spawn", sc, "launch with two tabs on one kernel", "every session live within 25 s; no 'connection failed' notice; reconnect_attempt back at 0",
                    f"live={[c['connection'] for c in sessions(self.state())]} failed_notices={len(failed)} attempts={[c.get('reconnect_attempt') for c in sessions(self.state())]}",
                    "pass" if s and not failed else "fail", self.still("connect"))
        # Scenario 1: typed while running lands as a steer within ~1 s.
        self.send(P_LONG); self.wait(lambda s: busy(s), 20, what="turn start"); time.sleep(3)
        t0 = time.time()
        self.app.click("composer-field"); self.app.type("Steer test line.\n")
        agent = (active(self.state()) or {}).get("agent_session") or "root"
        seen = None
        while time.time() - t0 < 2.5:
            if "steer" in inbox_kinds(agent) or transcript_has(agent, "Steer test line."):
                seen = round(time.time() - t0, 2); break
            time.sleep(0.1)
        act = active(self.state()) or {}
        landed = any("Steer test line." in it.get("text", "") for it in act.get("items", []) if it.get("kind") == "user")
        still_busy = busy(self.state())
        self.record("steer-within-1s", sc, "type a line at +3 s of a long turn", "the kernel has it (steer inbox file, or already in the running turn's transcript) within 1.5 s; user card on the transcript; queued == 0; turn still running",
                    f"kernel had it after {seen}s; landed={landed}; queued={act.get('queued')}; busy={still_busy}", "pass" if (seen is not None and seen <= 1.5 and landed and act.get("queued", 0) == 0) else "fail", self.still("steer"))
        # Scenario 4: a queued follow-up survives a window relaunch.
        self.app.click("composer-field"); self.app.type("RESTART-TEST reply with the word restart."); self.app.key("cmd-shift-enter"); time.sleep(1.5)
        held_before = (active(self.state()) or {}).get("held", 0)
        try:
            self.app.close()
        except Exception:
            pass
        time.sleep(2)
        app = self.drv.Arbos.launch(binary=binary, env={"ARBOS_KERNEL_BIN": kernel, "DISPLAY": DISPLAY, "XDG_CONFIG_HOME": str(xdg), "XDG_DATA_HOME": str(xdg / "data")},
                                    log=str(self.outdir / "app-relaunch.log"), timeout=90)
        self.app = app
        place_window(); time.sleep(1.5)
        self.tabs = self.app.exists("tab-bar"); self.go_project()
        s2 = self.wait(lambda s: not busy(s), 120, what="idle")
        # Not `wait_idle`: its recover() presses Stop, and the kernel drops a
        # queued prompt on Stop (F-105, filed) — the row would then measure
        # the stop, not the relaunch. A turn still running after two minutes
        # is its own finding here; the held row is what is checked.
        stopped_by_gate = False
        if s2 is None and busy(self.state()):
            stopped_by_gate = True
            self.recover()
        items = (active(self.state()) or {}).get("items", [])
        ran = any("RESTART-TEST" in it.get("text", "") for it in items if it.get("kind") == "user")
        self.record("queue-survives-relaunch", sc, "⇧⌘↩ a follow-up while busy, quit, relaunch, wait for idle",
                    "the follow-up ran (its user card is on the transcript) or is still held by the kernel",
                    f"held_before={held_before} ran={ran} held_now={(active(self.state()) or {}).get('held')}{' (turn still running at 120 s; Stop pressed after the read — F-105)' if stopped_by_gate else ''}",
                    "pass" if ran or (active(self.state()) or {}).get("held", 0) > 0 else ("not-reachable" if held_before == 0 else "fail"), self.still("relaunch"))
        # F-206: after a relaunch, a nested chat whose transcript ends on a
        # turn's end (with the kernel's after-lines behind it) reads done,
        # not a hollow ring for as long as the window lives.
        time.sleep(12)
        st = self.state()
        nested = [c for c in sessions(st) if c.get("parent") is not None and not c.get("closed") and c.get("connection") == "live" and not (c.get("streaming") or c.get("turn_open"))]
        ended = []
        for c in nested:
            sid = c.get("agent_session")
            if not sid:
                continue
            path = PROJ / ".arbos" / "agents" / sid / "transcript.jsonl"
            if not path.exists():
                continue
            kinds = [json.loads(l).get("kind") for l in path.read_text().splitlines() if l.strip()]
            while kinds and kinds[-1] in ("notice", "compaction", "fold", "nudge", "window_reset", "image_described"):
                kinds.pop()
            if kinds and kinds[-1] in ("turn_complete", "interrupted"):
                ended.append((c["id"], c.get("child_state")))
        if ended:
            self.record("relaunch-ended-reads-done", sc, "relaunch, wait 12 s, read nested chats whose file ends a turn",
                        "each reads done (or asking), none waiting", f"{ended}",
                        "pass" if all(state in ("done", "asking") for _, state in ended) else "fail")
        else:
            self.gap("relaunch-ended-reads-done", sc, "read", "no nested chat with an ended transcript to read")
        # Scenario 21: typed words while a question stands are never a skip.
        self.send(P_ASK)
        s = self.wait(lambda s: (active(s) or {}).get("questions"), 90, what="question card")
        if s:
            self.app.click("composer-field"); self.app.type("Unrelated thought: the answer is whichever you prefer.\n"); time.sleep(2)
            act = active(self.state()) or {}
            users = [it.get("text", "") for it in act.get("items", []) if it.get("kind") == "user"]
            notices = [it.get("text", "") for it in act.get("items", []) if it.get("kind") == "notice"]
            kept = any("Unrelated thought" in u for u in users)
            self.record("ask-typed-text", sc, "type a line with no option picked, Enter", "card resolved; the words are a user line on the transcript; nothing says skipped",
                        f"questions={bool(act.get('questions'))} kept={kept} skipped_notice={any('skip' in n.lower() for n in notices)}",
                        "pass" if kept and not act.get("questions") and not any("skip" in n.lower() for n in notices) else "fail", self.still("ask-typed"))
            self.wait_idle(60)
        else:
            self.gap("ask-typed-text", sc, "wait for ask", "model never asked")
        # Scenario 23: a child folder deleted under the live window is left alone.
        n0 = len(sessions(self.state()))
        self.send("Spawn one sub-agent whose only task is to reply with the word child, wait for it, then say done.")
        self.wait(lambda s: len(sessions(s)) > n0, 90, what="child session"); self.wait_idle(120)
        act_id = self.state().get("active_session")
        kids = [c for c in project_sessions(self.state()) if c.get("agent_session") and c["agent_session"] != "root" and c["id"] != act_id]
        kids.sort(key=lambda c: c.get("parent") is None)
        if kids:
            kid = kids[-1]
            folder = PROJ / ".arbos" / "agents" / kid["agent_session"]
            shutil.rmtree(folder, ignore_errors=True)
            a0 = attach_opens(); time.sleep(10); a1 = attach_opens()
            kid_now = next((c for c in sessions(self.state()) if c["id"] == kid["id"]), None)
            self.record("deleted-child", sc, "rm -rf the child's agent folder, wait 10 s", "no reconnect loop: attach_open grows by < 5 in 10 s; the child's reconnect_attempt does not climb; no ghost folder",
                        f"attach_open +{a1 - a0}; child={kid_now and kid_now.get('connection')} attempt={kid_now and kid_now.get('reconnect_attempt')} folder_back={folder.exists()}",
                        "pass" if (a1 - a0) < 5 and not folder.exists() and not (kid_now and (kid_now.get("reconnect_attempt") or 0) > 1) else "fail", self.still("deleted-child"))
        else:
            self.gap("deleted-child", sc, "spawn", "no child session to delete")

    def phase_provider_offer(self, binary: str, kernel: str, xdg: Path) -> None:
        """Second launch: the kernel has no key but the desktop does."""
        sc = "provider-offer"
        cfg = xdg / "arbos" / "config.toml"
        cfg.write_text(cfg.read_text().replace('api_key_env = "OPENROUTER_API_KEY"', 'api_key_env = "QA_NO_SUCH_KEY"'))
        # The place as the run left it, before this phase wipes it: the
        # evidence for anything the earlier phases found (F-105's lost
        # follow-up was undiagnosable twice because this reset ran first).
        keep = self.outdir / "arbos-before-nokey"
        shutil.rmtree(keep, ignore_errors=True)
        try:
            shutil.copytree(PROJ / ".arbos", keep, ignore=shutil.ignore_patterns("jobs", "pages", "trace", "*.png", "*.jpg"))
        except OSError as e:
            log(f"could not keep the place: {e}")
        shutil.rmtree(PROJ / ".arbos", ignore_errors=True)
        subprocess.run(["pkill", "-f", f"arbos-kernel serve {PROJ}$"]); time.sleep(1)
        app = self.drv.Arbos.launch(binary=binary, env={"ARBOS_KERNEL_BIN": kernel, "DISPLAY": DISPLAY, "XDG_CONFIG_HOME": str(xdg), "XDG_DATA_HOME": str(xdg / "data")},
                                    log=str(self.outdir / "app-nokey.log"), timeout=90)
        old = self.app; self.app = app
        try:
            place_window(); time.sleep(1)
            self.tabs = self.app.exists("tab-bar"); self.go_project()
            try:
                self.app.wait_element("composer-field", timeout=8, reachable=True)
            except Exception:
                self.app.key("cmd-n")
            self.send("Reply with one word: pong.")
            s = self.wait(lambda s: self.app.exists("provider-offer") or not busy(s), 40, what="provider offer")
            if self.app.exists("provider-offer"):
                self.inv(sc)
                btns = [i for i in self.ids() if self.app.find(i)["y"] >= self.app.find("provider-offer")["y"] and self.app.find(i)["y"] <= self.app.find("provider-offer")["y"] + self.app.find("provider-offer")["h"] and i.rsplit(".", 1)[-1] != "provider-offer"]
                self.record("provider-offer", sc, "strip shown when kernel has no key", "strip with Use-my-key buttons", f"buttons: {[b.rsplit('.', 1)[-1] for b in btns]}", "pass", self.still("provider-offer"))
                if btns:
                    self.check(btns[0].rsplit(".", 1)[-1], sc, "click first offer button", "strip goes away; a later prompt answers",
                               lambda: self.app.click(btns[0]), lambda a, b: not self.app.exists("provider-offer"), settle=2)
                    self.send("Reply with one word: pong."); s2 = self.wait_idle(60)
                    items = (active(s2) or {}).get("items", []) if s2 else []
                    self.record("provider-offer follow-through", sc, "prompt after lending the key", "answer arrives", f"items={len(items)} busy={busy(s2) if s2 else '?'}", "pass" if s2 and not busy(s2) and items else "fail", self.still("offer-followthrough"))
            else:
                self.gap("provider-offer", sc, "trigger", "strip never appeared: kernel found a key anyway or the desktop has none of its own to lend")
        finally:
            try:
                app.close()
            except Exception:
                pass
            self.app = old

    # -- output ------------------------------------------------------------

    def save(self) -> None:
        (self.outdir / "results.json").write_text(json.dumps(self.rows, indent=1))
        (self.outdir / "inventory.json").write_text(json.dumps(self.inventory, indent=1))
        for attempt in range(4):
            try:
                self.store_dir.mkdir(parents=True, exist_ok=True)
                for f in self.outdir.iterdir():
                    dst = self.store_dir / f.name
                    if not dst.exists() or dst.stat().st_size != f.stat().st_size or f.suffix == ".json":
                        shutil.copyfile(f, dst)
                break
            except OSError as err:
                log(f"store copy failed ({err}); retry {attempt + 1}")
                time.sleep(3)
        counts: dict[str, int] = {}
        for r in self.rows:
            counts[r["result"]] = counts.get(r["result"], 0) + 1
        log(f"results: {counts}")


def place_window() -> None:
    for _ in range(30):
        ids = subprocess.run(["xdotool", "search", "--name", "^Arbos$"], capture_output=True, text=True, env=ENV).stdout.split()
        if ids:
            wid = ids[0]
            subprocess.run(["xdotool", "windowactivate", "--sync", wid], env=ENV)
            subprocess.run(["xdotool", "windowsize", wid, "1600", "1000"], env=ENV)
            subprocess.run(["xdotool", "windowmove", wid, "100", "60"], env=ENV)
            # Read the geometry back rather than trust the move. (Added
            # after a false alarm: a crop of a still, cut at x=200 while the
            # window sits at x=100, read as "the window is off-screen"; the
            # raw still showed it whole. The guard is right anyway — a
            # coordinate the rig set is not one it verified — rig audit R13.)
            time.sleep(0.4)
            geo = subprocess.run(["xdotool", "getwindowgeometry", "--shell", wid], capture_output=True, text=True, env=ENV).stdout
            pos = {k: int(v) for k, v in (line.split("=") for line in geo.split() if "=" in line)}
            if pos.get("X", 0) < 0 or pos.get("Y", 0) < 0 or pos.get("X", 0) + pos.get("WIDTH", 0) > 1920:
                subprocess.run(["xdotool", "windowmove", wid, "100", "60"], env=ENV); time.sleep(0.4)
                geo = subprocess.run(["xdotool", "getwindowgeometry", "--shell", wid], capture_output=True, text=True, env=ENV).stdout
                pos = {k: int(v) for k, v in (line.split("=") for line in geo.split() if "=" in line)}
                if pos.get("X", 0) < 0 or pos.get("Y", 0) < 0:
                    raise DisplayHung(f"window sits off-screen after placement: {pos}")
            log(f"window at {pos.get('X')},{pos.get('Y')} {pos.get('WIDTH')}x{pos.get('HEIGHT')}")
            return
        time.sleep(0.5)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--branch", required=True)
    ap.add_argument("--binary", required=True)
    ap.add_argument("--kernel", required=True)
    ap.add_argument("--binary-from-elsewhere", action="store_true",
                    help="the desktop binary is not built from this checkout (a release, another branch); skip the tree check")
    ap.add_argument("--driver-py", default=None)
    # openai/* through this OpenRouter key is blocked (403 policy violation, 2026-09-16); Gemini answers
    ap.add_argument("--model", default=os.environ.get("QA_MODEL", "google/gemini-2.5-flash"))
    ap.add_argument("--phases", default="LCTQPSABRWMKGNDXO")
    args = ap.parse_args()
    if not os.environ.get("OPENROUTER_API_KEY"):
        raise SystemExit("OPENROUTER_API_KEY is not set")
    drv = load_driver(Path(args.driver_py) if args.driver_py else DRIVER_PY)

    store_dir = STORE / "media" / "qa-ui" / args.branch
    outdir = Path("/tmp/qa-ui") / args.branch
    shutil.rmtree(outdir, ignore_errors=True)
    outdir.mkdir(parents=True, exist_ok=True)
    xdg = Path(f"/tmp/qa-ui-xdg-{args.branch}")
    shutil.rmtree(xdg, ignore_errors=True)
    (xdg / "arbos").mkdir(parents=True)
    (xdg / "arbos" / "config.toml").write_text(f'model = "{args.model}"\napi_base = "https://openrouter.ai/api/v1"\napi_key_env = "OPENROUTER_API_KEY"\n')
    subprocess.run(["bash", str(PARITY / "seed-project.sh")], check=True)
    subprocess.run(["pkill", "-f", f"arbos-kernel serve {PROJ}$"]); subprocess.run(["pkill", "-f", "arbos-kernel serve .*/.arbos$"]); time.sleep(1)
    shutil.rmtree(PROJ / ".arbos", ignore_errors=True)
    drv.seed_state(xdg, [str(PROJ)])
    st = xdg / "arbos-desktop" / "state.toml"
    # Jacob's file: written by a build from before the bionic default flipped
    # — `bionic_reading = true` and no `version` key. The launch must read
    # that `true` as the old default, not a choice.
    st.write_text(st.read_text().replace('appearance = "dark"', 'appearance = "light"').replace("bionic_reading = false", "bionic_reading = true"))

    # The fake `gh` first on PATH: a worker's `gh pr create` answers with a
    # PR URL the kernel records, so the PRs pill has something to count. The
    # store mount holds no exec bits, so the script is copied out to /tmp.
    fake_gh = Path(f"/tmp/qa-ui-fake-gh-{args.branch}")
    fake_gh.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(PARITY / "fake-gh" / "gh.sh", fake_gh / "gh")
    os.chmod(fake_gh / "gh", 0o755)
    os.environ["PATH"] = f"{fake_gh}:{os.environ.get('PATH', '')}"
    os.environ["FAKE_GH_STATE"] = str(fake_gh / "counter")
    build = kernel_build(args.kernel)
    log(f"kernel under test: {build}")
    # The desktop's build too, from the binary, against the tree it should
    # have come from. A `cargo build` that failed leaves the previous binary
    # in target/, and every copy-then-launch step downstream runs it without
    # a word: a gate that only reads the kernel's build can pass a whole
    # cycle on a desktop that is not the PR's (rig audit R21, after the QA
    # loop's `-- desktop main: app build failed` one-liner cost it fifteen
    # scenarios). Unless told the binary is from elsewhere, a mismatch fails.
    app_build = desktop_build(args.binary)
    src_sha = tree_sha(PARITY.parents[1])
    log(f"desktop under test: {app_build} (tree {src_sha or '?'})")
    log(f"launching {args.binary}")
    app = drv.Arbos.launch(binary=args.binary, env={"ARBOS_KERNEL_BIN": args.kernel, "DISPLAY": DISPLAY, "XDG_CONFIG_HOME": str(xdg), "XDG_DATA_HOME": str(xdg / "data")},
                           log=str(outdir / "app.log"), timeout=90)
    p = Pass(drv, app, args.branch, outdir, store_dir)
    # The first row of every run names the kernel the run measured against.
    p.record("kernel", "rig", "arbos-kernel --version", "the build under test, from the binary", build, "info")
    stale = not args.binary_from_elsewhere and not binary_matches_tree(app_build, src_sha)
    p.record("desktop", "rig", "arbos-desktop --version", f"the binary under test is this tree's ({src_sha or '?'})", app_build,
             "fail" if stale else "info")
    if stale:
        log(f"FAULT: the desktop binary is {app_build}, not a build of tree {src_sha}: a failed build left the old one behind")
        p.stop_phase("rig", f"desktop binary {app_build} is not tree {src_sha}", list(args.phases), fault=True)
        p.save()
        try:
            app.close()
        except Exception:
            pass
        return 2
    phases = {"L": p.phase_launch, "C": p.phase_composer, "T": p.phase_turn, "Q": p.phase_question, "P": p.phase_plan,
              "S": p.phase_subagents, "A": p.phase_artifacts, "B": p.phase_tabs, "R": p.phase_panel, "W": p.phase_settings,
              "M": p.phase_menus, "G": p.phase_prs, "N": p.phase_permissions,
              "D": lambda: p.phase_world(args.kernel)}
    try:
        place_window(); time.sleep(1.5)
        for letter in args.phases:
            if letter in ("O", "X"):
                continue
            fn = phases.get(letter)
            if not fn:
                continue
            log(f"== phase {letter} {fn.__name__}")
            if getattr(p, "kernel_holds_stop", False) and busy(p.state()):
                p.stop_phase(fn.__name__, "the kernel held Stop over a waiting spawn twice this run (filed 2026-09-17-stop-waits-for-a-blocking-spawn); the phase would only repeat the cascade", [letter], fault=False)
                continue
            # The rig's own pulse before every phase: a display that has
            # stopped answering fails the run loudly (the ten-minute hang
            # of 2026-09-16 would otherwise have passed as quiet).
            display_pulse(DISPLAY)
            try:
                fn()
            except DisplayHung:
                raise
            except Exception as err:
                p.record(f"phase {letter}", fn.__name__, "-", "-", f"{type(err).__name__}: {err}\n{traceback.format_exc()[-600:]}", "fail", p.still(f"phase-{letter}-error"))
            p.save()
        if "X" in args.phases:
            log("== phase X multitasking audit")
            try:
                p.phase_multitask(args.binary, args.kernel, xdg)
            except Exception as err:
                p.record("phase X", "multitask", "-", "-", f"{type(err).__name__}: {err}\n{traceback.format_exc()[-600:]}", "fail", p.still("phase-X-error"))
            p.save()
            app = p.app
        try:
            app.close()
        except Exception:
            pass
        if "O" in args.phases:
            log("== phase O provider offer")
            try:
                p.phase_provider_offer(args.binary, args.kernel, xdg)
            except Exception as err:
                p.record("phase O", "provider-offer", "-", "-", f"{type(err).__name__}: {err}", "fail")
    except DisplayHung as err:
        p.record("display", "rig", "pulse", "the X display answers within 8 s", str(err), "fail")
        log(f"DISPLAY HUNG — run aborted: {err}")
        p.save()
        # The app is blocked on the same display; a polite close would
        # block with it.
        subprocess.run(["pkill", "-x", "arbos-desktop"], check=False)
        return 3
    finally:
        p.save()
        try:
            app.close()
        except Exception:
            pass
    print(store_dir)
    return 0


if __name__ == "__main__":
    sys.exit(main())
