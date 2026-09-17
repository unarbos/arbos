#!/usr/bin/env python3
"""The acceptance journey, end to end, scored step by step.

Jacob (2026-09-16): "The upgrade loops need to actually run full cycles of
creating a project, running a challenge, doing follow up etc." This is that
cycle on the desktop, as a person does it: make a project on a real folder
from nothing, give it a challenge that takes workers and can fail, watch it
work, follow up mid-flight and after, interrupt and continue, close and
reopen the window, and check the work is on disk. The element checks
(the one "N Working" line, the pills, the fold) sit inside the flow.

    python3 journey.py --bindir <dir with arbos-desktop + arbos-kernel> --label <build> \
        [--runs 1] [--model google/gemini-2.5-flash] [--record /path/journeys.jsonl]

Every step of every run is a row in the record (JSON lines), so a step's
pass rate over the last runs is one `--rates` away, and a step that fails
twice in a row on the same label prints a `BUG:` line — a named bug, not a
note. Stills per step under /tmp/journey/<label>/<run>/.

Step names follow QA's `docs/acceptance-journeys.md` once it lands; until
then the J-numbers below are the layout worker's draft of the same path.
"""
from __future__ import annotations

import argparse
import importlib.util
import json
import os
import shutil
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path

HERE = Path(__file__).resolve().parent
DRIVER_PY = Path(os.environ.get("ARBOS_DRIVER_PY", HERE.parent.parent / "driver" / "arbosdriver.py"))
DISPLAY = os.environ.get("DISPLAY", ":1")
ENV = dict(os.environ, DISPLAY=DISPLAY)

# The challenge: real work for two workers, tests that can fail, files on disk.
CHALLENGE = (
    "Build a small command-line to-do app in this folder: `todo.py` with `add <text>`, `list` and "
    "`done <n>` commands that keep items in `todo.json`, and `test_todo.py` with tests for all three "
    "commands using only the standard library. Use one worker for the app and one for the tests, in "
    "parallel; then run `python3 -m unittest test_todo.py` and report the result."
)
STEER = "Also add a `clear` command that removes every item."
FIRST_LINE = "Reply with the single word pong."
FOLLOW_UP = "Run `python3 -m unittest test_todo.py` once more and tell me the result in one line."
AFTER_REOPEN = "In one sentence: what did you build here, and does it pass its tests?"


def dunst_history() -> list[str] | None:
    """The notification daemon's own record (dunst on the rig), oldest
    first; None without dunstctl. The proof an alert reached a daemon."""
    if not shutil.which("dunstctl"):
        return None
    try:
        # History holds notifications once they are closed; a popup still
        # on screen is not in it yet. Close them first, then read.
        subprocess.run(["dunstctl", "close-all"], capture_output=True, timeout=5, env=ENV)
        raw = subprocess.run(["dunstctl", "history"], capture_output=True, text=True, timeout=5, env=ENV).stdout
        data = json.loads(raw).get("data", [[]])
        entries = data[0] if data else []
        return [f"{e.get('summary', {}).get('data', '')} | {e.get('body', {}).get('data', '')}" for e in reversed(entries)]
    except Exception:  # noqa: BLE001
        return None


def log(msg: str) -> None:
    print(f"[{datetime.now().strftime('%H:%M:%S')}] {msg}", flush=True)


def load_driver():
    spec = importlib.util.spec_from_file_location("arbosdriver", DRIVER_PY)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def sessions(state: dict) -> list[dict]:
    return [c for p in state.get("projects", []) for c in p.get("sessions", [])]


def busy(state: dict) -> bool:
    return any(c["streaming"] or c["turn_open"] for c in sessions(state))


class Journey:
    def __init__(self, drv, args, run: int):
        self.drv = drv
        self.args = args
        self.run = run
        self.label = args.label
        self.out = Path("/tmp/journey") / args.label / f"run-{run:03d}"
        shutil.rmtree(self.out, ignore_errors=True)
        self.out.mkdir(parents=True)
        stamp = datetime.now().strftime("%H%M%S")
        self.proj = Path(os.path.expanduser(f"~/journeys/{args.label}-{stamp}"))
        self.xdg = Path(f"/tmp/journey-xdg-{args.label}")
        self.app = None
        self.rows: list[dict] = []
        self.n = 0
        self.started = time.time()

    # -- scoring ------------------------------------------------------------

    def score(self, step: str, expected: str, ok: bool, observed: str, t0: float) -> bool:
        self.n += 1
        still = self.still(step)
        row = {
            "run": self.run, "label": self.label, "at": datetime.now(timezone.utc).isoformat(timespec="seconds"),
            "step": step, "expected": expected, "observed": observed[:400],
            "result": "pass" if ok else "fail", "secs": round(time.time() - t0, 1), "still": still,
        }
        self.rows.append(row)
        log(f"{'pass' if ok else 'FAIL':5} {step:28} {observed[:110]}")
        return ok

    def still(self, name: str) -> str:
        path = self.out / f"{self.n:02d}-{name}.png"
        subprocess.run(["scrot", "-o", str(path)], env=ENV, check=False)
        return str(path)

    # -- helpers ------------------------------------------------------------

    def state(self) -> dict:
        return self.app.state()

    def root(self) -> dict | None:
        st = self.state()
        for p in st["projects"]:
            if p["path"].rstrip("/") == str(self.proj).rstrip("/"):
                for c in p["sessions"]:
                    if c.get("parent") is None:
                        return c
        return None

    def project_ix(self) -> int | None:
        for p in self.state()["projects"]:
            if p["path"].rstrip("/") == str(self.proj).rstrip("/"):
                return p["index"]
        return None

    def wait(self, ok, timeout: float, every: float = 0.5):
        t0 = time.time()
        last = None
        while time.time() - t0 < timeout:
            try:
                last = ok()
            except Exception:  # noqa: BLE001 — the socket blinks during a relaunch
                last = None
            if last:
                return last
            time.sleep(every)
        return None

    def wait_root_idle(self, timeout: float) -> bool:
        def idle():
            c = self.root()
            return bool(c) and not c["streaming"] and not c["turn_open"] and not any(
                k["streaming"] or k["turn_open"] for k in sessions(self.state()) if k.get("parent") == c["id"]
            )
        return bool(self.wait(idle, timeout, 1.0))

    def send(self, text: str) -> None:
        if self.state()["composer"]["text"]:
            self.app.click("composer-field")
            self.app.key("cmd-a")
            self.app.key("backspace")
        self.app.click("composer-field")
        self.app.type(text + "\n")

    def launch(self, fresh: bool, keyless: bool = False) -> None:
        if fresh:
            shutil.rmtree(self.xdg, ignore_errors=True)
            (self.xdg / "arbos").mkdir(parents=True)
            key_env = "ARBOS_JOURNEY_NO_SUCH_KEY" if keyless else "OPENROUTER_API_KEY"
            (self.xdg / "arbos" / "config.toml").write_text(
                f'model = "{self.args.model}"\napi_base = "https://openrouter.ai/api/v1"\napi_key_env = "{key_env}"\n'
            )
            self.drv.seed_state(self.xdg, [])
        env = {"ARBOS_KERNEL_BIN": f"{self.args.bindir}/arbos-kernel", "DISPLAY": DISPLAY,
               "XDG_CONFIG_HOME": str(self.xdg), "XDG_DATA_HOME": str(self.xdg / "data")}
        if keyless:
            # The window must not lend the kernel a key either.
            env["OPENROUTER_API_KEY"] = ""
        self.app = self.drv.Arbos.launch(
            binary=f"{self.args.bindir}/arbos-desktop",
            env=env,
            log=str(self.out / ("app.log" if fresh else "app-relaunch.log")), timeout=90,
        )
        time.sleep(2)
        wid = subprocess.run(["xdotool", "search", "--name", "^Arbos$"], capture_output=True, text=True, env=ENV).stdout.split()
        if wid:
            subprocess.run(["xdotool", "windowactivate", "--sync", wid[0]], env=ENV)
            subprocess.run(["xdotool", "windowsize", wid[0], "1440", "900"], env=ENV)
            subprocess.run(["xdotool", "windowmove", wid[0], "200", "120"], env=ENV)
        time.sleep(1)
        # The first launch opens the Permissions sheet over the chat, as it
        # does for a new user; they skip it or enable rows. The journey
        # skips it — the sheet has its own checks in ui_pass.py.
        if self.app.exists("permissions-skip"):
            self.app.click("permissions-skip")
            time.sleep(0.6)

    # -- the journey --------------------------------------------------------

    def run_all(self) -> list[dict]:
        try:
            if self.args.keyless_first:
                self.k01_keyless_first_line()
                self.k02_key_runs_it()
            self.j01_launch()
            if self.j02_create_project():
                self.j03_kickoff()
                self.j04_challenge()
                self.j05_live_shape()
                self.j06_steer()
                self.j07_interrupt()
                self.j08_continue()
                self.j09_on_disk()
                self.j10_follow_up()
                self.j10b_notification_away()
                self.j11_close_reopen()
                self.j12_after_reopen()
        finally:
            try:
                if self.app:
                    self.app.close()
            except Exception:  # noqa: BLE001
                pass
            subprocess.run(["pkill", "-f", f"arbos-kernel serve {self.proj}"], check=False)
        return self.rows

    # -- stage zero: the first project with no key at all -----------------

    def k01_keyless_first_line(self) -> None:
        """A new user's very first screen: no model key anywhere. The line
        they type must be kept and shown as waiting, never queued behind a
        kickoff that cannot start (#312 kernel half, F-84 desktop half)."""
        t0 = time.time()
        self.launch(fresh=True, keyless=True)
        self.proj.parent.mkdir(parents=True, exist_ok=True)
        self.app.key("cmd-t"); time.sleep(0.8)
        self.app.type(str(self.proj)); time.sleep(1.2)
        self.app.key("enter"); time.sleep(2.5)
        if self.app.exists("tab-sheet-done"):
            self.app.key("escape"); time.sleep(0.6)
        time.sleep(4)
        self.send(FIRST_LINE)
        settled = self.wait(lambda: ((self.root() or {}).get("held") or 0) >= 1 or None, 20)
        time.sleep(1.5)
        c = self.root() or {}
        users = [i for i in c.get("items", []) if i.get("kind") == "user" and FIRST_LINE in (i.get("text") or "")]
        notices = [i for i in c.get("items", []) if i.get("kind") == "notice"]
        offer = self.app.exists("provider-offer-remember") or "no key" in json.dumps(self.state()).lower()
        ok = (len(users) == 1 and len(notices) == 1 and c.get("held") == 1 and not c.get("streaming")
              and not c.get("turn_open") and self.app.exists("followups-head"))
        self.score("K01-keyless-first-line", "the card lands once; one line says no key and that the words are kept; the follow-up row shows them waiting for a key; no shimmer, no second red line",
                   ok, f"cards={len(users)} notices={len(notices)} held={c.get('held')} streaming={c.get('streaming')} row={self.app.exists('followups-head')} offer_bar={offer} settled={bool(settled)}", t0)

    def k02_key_runs_it(self) -> None:
        """A key lands (here: written to the kernel's config and the kernel
        restarted): the kept line runs and its reply arrives."""
        t0 = time.time()
        (self.xdg / "arbos" / "config.toml").write_text(
            f'model = "{self.args.model}"\napi_base = "https://openrouter.ai/api/v1"\napi_key_env = "OPENROUTER_API_KEY"\n'
        )
        try:
            self.app.close()
        except Exception:  # noqa: BLE001
            pass
        subprocess.run(["pkill", "-f", f"arbos-kernel serve {self.proj}"], check=False)
        time.sleep(1.5)
        self.launch(fresh=False, keyless=False)
        ix = self.wait(lambda: self.project_ix(), 20)
        if ix is not None:
            self.app.click(f"tab-{ix}"); time.sleep(1.5)
        ran = self.wait(lambda: any(i.get("kind") == "agent" and (i.get("text") or "").strip() for i in (self.root() or {}).get("items", [])) or None, 150)
        c = self.root() or {}
        users = [i for i in c.get("items", []) if i.get("kind") == "user" and FIRST_LINE in (i.get("text") or "")]
        reply = next((i.get("text", "")[:80] for i in reversed(c.get("items", [])) if i.get("kind") == "agent"), "")
        self.score("K02-key-runs-kept-line", "once a key is in place the kept line runs by itself: one card, a reply, the follow-up row gone",
                   bool(ran) and len(users) == 1 and c.get("held", 0) == 0, f"reply={reply!r} cards={len(users)} held={c.get('held')}", t0)
        try:
            self.app.close()
        except Exception:  # noqa: BLE001
            pass
        subprocess.run(["pkill", "-f", f"arbos-kernel serve {self.proj}"], check=False)
        # The keyed journey starts on its own fresh folder.
        self.proj = Path(f"{self.proj}-keyed")

    def j01_launch(self) -> None:
        t0 = time.time()
        self.launch(fresh=True)
        st = self.state()
        self.score("J01-launch", "the window is up on the Home tab, idle",
                   bool(st.get("projects")) and not busy(st), f"projects={len(st.get('projects', []))} busy={busy(st)}", t0)

    def j02_create_project(self) -> bool:
        t0 = time.time()
        self.proj.parent.mkdir(parents=True, exist_ok=True)
        self.app.key("cmd-t"); time.sleep(0.8)
        if not self.state().get("opener_open"):
            self.app.click("new-tab"); time.sleep(0.8)
        self.app.type(str(self.proj)); time.sleep(1.2)
        rows = [i for i in self.app.ids() if i.rsplit(".", 1)[-1].startswith("opener-row-")]
        self.app.key("enter"); time.sleep(2.5)
        if self.app.exists("tab-sheet-done"):
            self.app.key("escape"); time.sleep(0.6)
        st = self.state()
        ix = self.project_ix()
        made = self.proj.is_dir()
        kickoff = self.app.exists("kickoff")
        # A fast kernel has the kickoff turn running by now and the
        # transcript has taken the view over: that is the same landing.
        c = self.root() or {}
        setting_up = kickoff or bool(c.get("streaming") or c.get("turn_open"))
        ok = made and ix is not None and st.get("pane") == "chat" and setting_up and not self.app.exists("pill-changes")
        self.score("J02-create-project", "⌘T, type a path that does not exist, Enter on Create: the folder is made, a tab opens on it on the kickoff view (or its turn already running); no Changes pill on a non-repo",
                   ok, f"rows={len(rows)} made={made} tab={ix} pane={st.get('pane')} kickoff_view={kickoff} kickoff_running={bool(c.get('streaming') or c.get('turn_open'))} changes_pill={self.app.exists('pill-changes')}", t0)
        return ix is not None

    def j03_kickoff(self) -> None:
        t0 = time.time()
        # The kickoff turn takes a few seconds to be filed and start: wait
        # for it to be live (or to have already greeted) before waiting for
        # it to end, or the challenge goes out under it (run 2 did).
        def begun():
            c = self.root() or {}
            return c.get("streaming") or c.get("turn_open") or any(i.get("kind") == "agent" for i in c.get("items", [])) or None
        self.wait(begun, 30)
        settled = self.wait_root_idle(150)
        c = self.root() or {}
        greeting = any(i.get("kind") == "agent" and (i.get("text") or "").strip() for i in c.get("items", []))
        failed = [i.get("text", "")[:80] for i in c.get("items", []) if i.get("kind") == "notice" and i.get("failed")]
        self.score("J03-kickoff-settles", "the kickoff turn ends within 150 s with a greeting and no failed notice",
                   settled and greeting and not failed, f"settled={settled} greeting={greeting} failed={failed} secs={round(time.time() - t0)}", t0)

    def j04_challenge(self) -> None:
        t0 = time.time()
        self.send(CHALLENGE)
        started = self.wait(lambda: busy(self.state()), 20)
        c = self.root() or {}
        children = self.wait(lambda: [k for k in sessions(self.state()) if k.get("parent") == c.get("id")] or None, 90) or []
        self.score("J04-challenge-spawns-workers", "the turn starts within 20 s and a worker appears within 90 s",
                   bool(started) and bool(children), f"started={bool(started)} workers={len(children)} pills={(self.root() or {}).get('pills')}", t0)

    def j05_live_shape(self) -> None:
        t0 = time.time()
        # The root either waits on its workers (one "N Working  <step>"
        # line while its turn runs) or has ended its turn and lets them run
        # on (a "1 Working  <step>" line per worker). Both are Cursor's.
        def lines():
            ids = [i.rsplit(".", 1)[-1] for i in self.app.ids()]
            found = [i for i in ids if i.startswith("child-line-")]
            return found or None
        line = self.wait(lines, 30) or []
        root_live = bool((self.root() or {}).get("turn_open") or (self.root() or {}).get("streaming"))
        card = self.app.exists("working-card")
        pill = self.app.exists("pill-working")
        one_line_while_live = (not root_live) or line == ["child-line-live"]
        self.score("J05-live-shape-one-line", "while workers run the root shows one 'N Working  <step>' line (or one per worker once its own turn ended) and the Working pill; no card until asked (F-82)",
                   bool(line) and pill and not card and one_line_while_live, f"lines={line} root_live={root_live} pill={pill} card_open={card}", t0)

    def j06_steer(self) -> None:
        t0 = time.time()
        if not busy(self.state()):
            self.score("J06-steer-mid-flight", "typed words while the turn runs land as a steer card within 2 s", False, "the turn had already ended before the steer", t0)
            return
        self.send(STEER)
        landed = self.wait(lambda: any(STEER in (i.get("text") or "") for i in (self.root() or {}).get("items", [])) or None, 3)
        c = self.root() or {}
        self.score("J06-steer-mid-flight", "typed words while the turn runs land as a steer card within 2 s, nothing queued in the window",
                   bool(landed) and c.get("queued", 0) == 0, f"landed={bool(landed)} queued={c.get('queued')} held={c.get('held')}", t0)

    def j07_interrupt(self) -> None:
        t0 = time.time()
        if not busy(self.state()):
            self.score("J07-interrupt", "Stop ends the turn within 10 s and says so", False, "nothing was running to stop", t0)
            return
        if self.app.exists("composer-stop"):
            self.app.click("composer-stop")
        stopped = self.wait(lambda: (not busy(self.state())) or None, 12)
        c = self.root() or {}
        said = any(i.get("kind") == "notice" and ("Stopped" in (i.get("text") or "") or "Interrupted" in (i.get("text") or "")) for i in c.get("items", [])[-6:])
        # With the root idle and only workers running, Stop stops the
        # workers: their transcripts carry the "Stopped by you", the root's
        # pill goes to zero.
        workers_stopped = (c.get("pills") or {}).get("working") == 0
        self.score("J07-interrupt", "Stop ends the turn (or the running workers) within 10 s; the transcript says 'Stopped by you', or the Working pill goes",
                   bool(stopped) and (said or workers_stopped), f"stopped={bool(stopped)} said={said} pills={c.get('pills')}", t0)

    def j08_continue(self) -> None:
        t0 = time.time()
        if self.app.exists("pill-continue"):
            self.app.click("pill-continue")
        else:
            self.send("Continue where you stopped and finish the to-do app and its tests.")
        started = self.wait(lambda: busy(self.state()) or None, 15)
        # The coordinator may end its own turn at once and let the workers
        # run on: "done" is the root idle with no worker left working.
        def all_done():
            c = self.root() or {}
            live = c.get("streaming") or c.get("turn_open")
            working = (c.get("pills") or {}).get("working", 0)
            return (not live and working == 0) or None
        time.sleep(3)
        done = bool(self.wait(all_done, 420, 2.0))
        c = self.root() or {}
        reply = next((i.get("text", "")[:100] for i in reversed(c.get("items", [])) if i.get("kind") == "agent" and (i.get("text") or "").strip()), "")
        self.score("J08-continue-to-done", "Continue Working restarts the turn and the work ends — root idle, no worker left working — within 7 min",
                   bool(started) and done and bool(reply), f"started={bool(started)} done={done} reply={reply!r}", t0)

    def j09_on_disk(self) -> None:
        t0 = time.time()
        found = {}
        for name in ("todo.py", "test_todo.py"):
            hits = [str(p.relative_to(self.proj)) for p in self.proj.rglob(name) if ".arbos/agents" not in str(p) or "worktree" in str(p)]
            found[name] = hits[:3]
        ok = bool(found["todo.py"]) and bool(found["test_todo.py"])
        tests = ""
        if found["todo.py"] and found["test_todo.py"]:
            where = (self.proj / found["todo.py"][0]).parent
            r = subprocess.run([sys.executable, "-m", "unittest", "test_todo.py"], cwd=where, capture_output=True, text=True, timeout=60)
            tests = (r.stderr.strip().splitlines() or [""])[-1][:80]
            ok = ok and r.returncode == 0
        self.score("J09-work-on-disk", "todo.py and test_todo.py exist under the project and the tests pass when run here",
                   ok, f"found={found} tests={tests!r}", t0)

    def j10_follow_up(self) -> None:
        t0 = time.time()
        before = len((self.root() or {}).get("items", []))
        self.send(FOLLOW_UP)
        started = self.wait(lambda: busy(self.state()) or None, 15)
        done = self.wait_root_idle(240)
        c = self.root() or {}
        new_agent = [i for i in c.get("items", [])[before:] if i.get("kind") == "agent" and (i.get("text") or "").strip()]
        self.score("J10-follow-up-after", "a follow-up on the finished project gets a reply within 4 min",
                   bool(started) and done and bool(new_agent), f"started={bool(started)} done={done} reply={(new_agent[-1].get('text', '')[:100] if new_agent else '')!r}", t0)

    def j10b_notification_away(self) -> None:
        """Leave for another tab while the project answers: the reply must
        reach you — a dot on the tab, an OS notification the daemon really
        got — and looking at it must clear the dot (#293/#297)."""
        t0 = time.time()
        ix = self.project_ix()
        st = self.state()
        posted_before = len(st.get("notifications", {}).get("posted", []))
        seen_before = (self.root() or {}).get("seen_through") or 0
        nonce = f"jn{int(time.time()) % 100000}"
        self.send(f"Run the shell command `sleep 6` and then reply with exactly: journey notification check {nonce}.")
        time.sleep(0.3)
        self.app.click("tab-0"); time.sleep(0.5)
        self.wait_root_idle(120); time.sleep(3)
        st = self.state()
        pr = next((p for p in st["projects"] if p["index"] == ix), {})
        c = self.root() or {}
        posted = st.get("notifications", {}).get("posted", [])[posted_before:]
        # The post's body is the reply's first line; a model may put words
        # before the nonce, so any new post from this chat counts and the
        # daemon is checked for that post's own words.
        hit = next((n for n in posted if nonce in (n.get("body") or "").lower()), None) or (posted[-1] if posted else None)
        key = ((hit or {}).get("body") or nonce).strip()[:40].lower()
        after, daemon = None, False
        for _ in range(10):
            after = dunst_history()
            daemon = after is not None and any(key in e.lower() or nonce in e.lower() for e in after)
            if daemon or after is None:
                break
            time.sleep(0.5)
        unseen_ok = (c.get("unseen") or 0) >= 1 and bool(pr.get("tab_dot"))
        os_ok = bool(hit) and not hit.get("error") and (daemon or after is None)
        self.app.click(f"tab-{ix}"); time.sleep(2.0)
        pr2 = next((p for p in self.state()["projects"] if p["index"] == ix), {})
        c2 = self.root() or {}
        seen_ok = c2.get("unseen") == 0 and not pr2.get("tab_dot") and (c2.get("seen_through") or 0) > seen_before
        self.score("J10b-notification-away", "a reply that lands while another tab is in front: unseen 1+, the tab's dot, an OS notification the daemon's history confirms; opening the chat clears it and sends seen",
                   unseen_ok and os_ok and seen_ok,
                   f"unseen={c.get('unseen')} tab_dot={pr.get('tab_dot')} posted={bool(hit)} body={(hit or {}).get('body', '')[:50]!r} daemon={'n/a' if after is None else daemon} err={hit.get('error') if hit else None} | after open: unseen={c2.get('unseen')} tab_dot={pr2.get('tab_dot')} seen_through {seen_before}->{c2.get('seen_through')}", t0)

    def j11_close_reopen(self) -> None:
        t0 = time.time()
        before = self.root() or {}
        n_before = len(before.get("items", []))
        last_agent = next((i.get("text", "") for i in reversed(before.get("items", [])) if i.get("kind") == "agent"), "")
        self.app.close()
        time.sleep(2)
        self.launch(fresh=False)
        ix = self.wait(lambda: self.project_ix(), 20)
        # The tab that was in front when the window closed is in front
        # again; Home only when that place is gone. Then click it as a user
        # would, whatever came up, so the rest of the run does not depend on
        # the restore.
        front = next((p.get("active") for p in self.state()["projects"] if p["index"] == ix), None) if ix is not None else None
        self.score("J11a-front-tab-restored", "the relaunch lands on the tab that was in front, not Home",
                   bool(front), f"tab={ix} in_front={front} active_tab={[p['index'] for p in self.state()['projects'] if p.get('active')]}", t0)
        t0 = time.time()
        if ix is not None:
            self.app.click(f"tab-{ix}"); time.sleep(1.5)
        after = self.wait(lambda: (self.root() if len((self.root() or {}).get("items", [])) >= 1 else None), 30) or {}
        n_after = len(after.get("items", []))
        kept = last_agent and any(last_agent.strip() == (i.get("text") or "").strip() for i in after.get("items", []) if i.get("kind") == "agent")
        users_after = [i.get("text", "")[:40] for i in after.get("items", []) if i.get("kind") == "user"]
        doubled = sorted({u for u in users_after if users_after.count(u) > 1})
        # Workers the last turn started may still be running through the
        # relaunch — the kernel outlives the window — so busy is allowed.
        self.score("J11-close-reopen", "quit and relaunch: the tab is back, the transcript has every item (no loss, no doubled prompt), the last reply is there",
                   ix is not None and n_after >= n_before - 2 and bool(kept) and not doubled,
                   f"tab={ix} items {n_before}->{n_after} last_reply_kept={bool(kept)} doubled_prompts={doubled} pane={self.state().get('pane')}", t0)

    def j12_after_reopen(self) -> None:
        t0 = time.time()
        before = len((self.root() or {}).get("items", []))
        self.send(AFTER_REOPEN)
        started = self.wait(lambda: busy(self.state()) or None, 15)
        done = self.wait_root_idle(180)
        c = self.root() or {}
        new_agent = [i for i in c.get("items", [])[before:] if i.get("kind") == "agent" and (i.get("text") or "").strip()]
        self.score("J12-follow-up-after-reopen", "the reopened project answers a follow-up that needs its history within 3 min",
                   bool(started) and done and bool(new_agent), f"started={bool(started)} done={done} reply={(new_agent[-1].get('text', '')[:120] if new_agent else '')!r}", t0)


# -- the record ------------------------------------------------------------

def rates(record: Path, label: str | None, last: int = 10) -> None:
    rows = [json.loads(l) for l in record.read_text().splitlines() if l.strip()] if record.exists() else []
    if label:
        rows = [r for r in rows if r["label"] == label]
    if not rows:
        print("no runs on record")
        return
    runs = sorted({(r["label"], r["run"]) for r in rows})[-last:]
    keep = {k for k in runs}
    rows = [r for r in rows if (r["label"], r["run"]) in keep]
    steps = []
    for r in rows:
        if r["step"] not in steps:
            steps.append(r["step"])
    print(f"{'step':30} {'pass':>5} {'runs':>5}  rate   last results (oldest → newest)")
    for step in steps:
        got = [r for r in rows if r["step"] == step]
        got.sort(key=lambda r: (r["label"], r["run"]))
        passed = sum(r["result"] == "pass" for r in got)
        trail = "".join("✓" if r["result"] == "pass" else "✗" for r in got)
        print(f"{step:30} {passed:5} {len(got):5}  {passed / len(got):5.0%}  {trail}")
        if len(got) >= 2 and got[-1]["result"] == "fail" and got[-2]["result"] == "fail":
            print(f"BUG: {step} failed twice in a row ({got[-2]['label']} run {got[-2]['run']}, {got[-1]['label']} run {got[-1]['run']}): {got[-1]['observed'][:160]}")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--bindir")
    ap.add_argument("--label")
    ap.add_argument("--runs", type=int, default=1)
    ap.add_argument("--model", default=os.environ.get("QA_MODEL", "google/gemini-2.5-flash"))
    ap.add_argument("--record", default=os.environ.get("JOURNEY_RECORD", str(Path.home() / "journeys" / "journeys.jsonl")))
    ap.add_argument("--rates", action="store_true", help="print the pass rate per step from the record and exit")
    ap.add_argument("--keyless-first", action="store_true", help="start with stage zero: a first project on a kernel with no model key")
    args = ap.parse_args()
    record = Path(args.record)
    if args.rates:
        rates(record, args.label)
        return 0
    if not args.bindir or not args.label:
        ap.error("--bindir and --label are required to run")
    if not os.environ.get("OPENROUTER_API_KEY"):
        raise SystemExit("OPENROUTER_API_KEY is not set")
    drv = load_driver()
    # The gate's stand-in `gh` first on PATH: a worker's `gh pr create` gets
    # a PR URL instead of a login prompt — run 4's coordinator ran
    # `gh auth login` and hung its turn on the interactive prompt (F-91).
    fake_gh = Path("/tmp/journey-fake-gh")
    fake_gh.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(HERE / "fake-gh" / "gh.sh", fake_gh / "gh")
    os.chmod(fake_gh / "gh", 0o755)
    os.environ["PATH"] = f"{fake_gh}:{os.environ.get('PATH', '')}"
    os.environ["FAKE_GH_STATE"] = str(fake_gh / "counter")
    record.parent.mkdir(parents=True, exist_ok=True)
    prior = [json.loads(l) for l in record.read_text().splitlines() if l.strip()] if record.exists() else []
    next_run = max([r["run"] for r in prior if r["label"] == args.label], default=0) + 1
    for k in range(args.runs):
        run = next_run + k
        log(f"== journey run {run} on {args.label}")
        rows = Journey(drv, args, run).run_all()
        with record.open("a") as f:
            for r in rows:
                f.write(json.dumps(r) + "\n")
        passed = sum(r["result"] == "pass" for r in rows)
        log(f"== run {run}: {passed}/{len(rows)} steps passed; stills under /tmp/journey/{args.label}/run-{run:03d}/")
    rates(record, args.label)
    return 0


if __name__ == "__main__":
    sys.exit(main())
