"""Desktop attacks: the gpui app under Xvfb, driven through its JSON driver.

Needs:
  ARBOS_DESKTOP_BIN     path to a built `arbos-desktop`
  ARBOS_DESKTOP_DRIVER  folder holding `arbosdriver.py` (desktop/driver in the repo)
  a kernel binary on PATH or next to the desktop binary (the app spawns
  `arbos-kernel serve` per folder), and `Xvfb`.

Every scenario: start Xvfb, launch the app on a private xdg with one project
(the scratch place), do the attack, keep screenshots and the app log in the
rollout, then check the place's state like every other scenario.
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


def hidden_store_binary(scratch):
    """The app binary behind deploy/ns-wrap.sh: the desktop and every kernel it spawns cannot
    see /cursor/stores (2026-09-17: an agent's `cd / && rm -rf *` deleted the Project Agent
    Store seven times, qal-j15). run.py exports ARBOS_QA_NS_WRAP; without it we refuse to launch."""
    if os.environ.get("ARBOS_QA_STORE_VISIBLE") == "1":
        return DESKTOP_BIN
    wrap = os.environ.get("ARBOS_QA_NS_WRAP", "")
    if not wrap:
        raise RuntimeError("ARBOS_QA_NS_WRAP unset: refusing to launch a desktop that can reach the Project Agent Store")
    script = Path(scratch) / "desktop-hidden-store.sh"
    script.write_text(f'#!/bin/sh\nexec bash "{wrap}" "{DESKTOP_BIN}" "$@"\n')
    script.chmod(0o755)
    return str(script)


def focus_composer(app, timeout=15):
    """Click `composer-field` until the app says it is focused. Returns what it took.

    `wait_element("composer-field", reachable=True)` says the element is there, not that the window
    is taking input. Measured 2026-09-18 on the app at `2301abd291c0`: the first send after launch
    leaves `composer.focused` false two seconds after a single click, and the keystrokes go nowhere —
    which is how `xp-01` reported `first-line-lost`, a data-loss rule, in five consecutive cycles
    with nothing wrong in the product. A second click takes focus. Every other send in the library
    happens later in a run and gets away with one click, so the fault was latent everywhere and
    visible only where the first send is the assertion.

    `composer.focused` is the app's own account (`desktop/src/driver.rs:1273`), so this waits on the
    thing it needs rather than on a sleep.
    """
    clicks, focused, deadline = 0, False, time.time() + timeout
    while time.time() < deadline and not focused:
        app.click("composer-field")
        clicks += 1
        settle = time.time() + 2
        while time.time() < settle:
            focused = bool((app.state().get("composer") or {}).get("focused"))
            if focused:
                break
            time.sleep(0.1)
    return {"focused": focused, "clicks": clicks}


def available():
    return bool(DESKTOP_BIN and Path(DESKTOP_BIN).exists() and DRIVER_DIR and (Path(DRIVER_DIR) / "arbosdriver.py").exists() and shutil.which("Xvfb"))


class Desktop:
    """Xvfb + the app + the driver, all scoped to one scenario."""

    def __init__(self, cx, tag="desktop", reseed=True):
        """`reseed=False` for a **relaunch**: keep the state the last window wrote.

        `arbosdriver.Arbos.launch()` calls `seed_state()` whenever it is given an `xdg`, and
        `seed_state` **overwrites** `<xdg>/arbos-desktop/state.toml` with a minimal file holding
        `projects`, `appearance` and nothing else — no `[last]`. That is right for a first window
        and wrong for a second one: it erases what the first window persisted before the app starts.

        Measured 2026-09-18 (qal-j35): the first window's final save wrote 646 bytes containing
        `[last."…/place"]`, confirmed on disk immediately after the rename; the relaunched window's
        very first read saw 234 bytes with no `[last.` at all, and no save ran in between. The
        difference was `seed_state`. So `mt-24-relaunch-restores-active-tab` could not pass on any
        build, and three product fixes (#675, #679, #682) were written against a red it produced.

        With `reseed=False` the driver is given no `xdg`, so it skips the re-seed, and this sets the
        two variables `launch()` would have set — `XDG_CONFIG_HOME` is already on `cx.env`, so only
        `XDG_DATA_HOME` needs adding — leaving the state file exactly as the last window left it.
        """
        self.cx, self.rec = cx, cx.rec
        self.tag = tag
        self.display = f":{9000 + os.getpid() % 900}"
        self.xvfb = subprocess.Popen(["Xvfb", self.display, "-screen", "0", "1600x1000x24", "-nolisten", "tcp"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        time.sleep(0.8)
        sys.path.insert(0, DRIVER_DIR)
        import arbosdriver  # noqa: E402

        self.mod = arbosdriver
        env = dict(cx.env)
        env["DISPLAY"] = self.display
        env["PATH"] = f"{Path(cx.binary).parent}:{env.get('PATH', '')}"
        env["ARBOS_DRIVER"] = "1"
        # The kernel agent the scenario is typing into: the project's main chat until `new_chat()`
        # mints another and sets it.
        self.agent = "root"
        self.log = self.rec.dir / f"{tag}.app.log"
        if reseed:
            self.app = arbosdriver.Arbos.launch(binary=hidden_store_binary(cx.scratch), env=env, log=self.log, xdg=cx.scratch / "xdg", projects=[str(cx.place)], timeout=90)
        else:
            env["XDG_CONFIG_HOME"] = str(cx.scratch / "xdg")
            env["XDG_DATA_HOME"] = str(cx.scratch / "xdg" / "data")
            self.app = arbosdriver.Arbos.launch(binary=hidden_store_binary(cx.scratch), env=env, log=self.log, timeout=90)
        self.rec.log(f"{tag}: app pid {self.app.hello().get('pid')} on {self.display}")
        self.shots = 0

    def shot(self, name):
        self.shots += 1
        path = self.rec.dir / f"{self.tag}-{self.shots:02d}-{name}.png"
        try:
            self.app.screenshot(path)
        except Exception as e:
            # The driver's screenshot is AppKit-only today (qa-015). Grab the
            # Xvfb root instead so the rollout still has a picture.
            self.rec.notes.setdefault("driver_screenshot_error", str(e)[:120])
            xwd = subprocess.run(["xwd", "-root", "-silent", "-display", self.display], capture_output=True)
            if xwd.returncode == 0 and shutil.which("convert"):
                conv = subprocess.run(["convert", "xwd:-", str(path)], input=xwd.stdout, capture_output=True)
                if conv.returncode != 0:
                    self.rec.log(f"screenshot {name}: convert failed: {conv.stderr[:120]!r}")
            else:
                self.rec.log(f"screenshot {name}: no xwd/convert; driver said {e}")
        return path

    def alive(self):
        return self.app.process is not None and self.app.process.poll() is None

    def timed(self, what, fn, limit=5.0):
        # `time.monotonic()`, never the wall clock: this VM is **paused** while the agent driving it is
        # idle, and only the wall clock absorbs the pause. Measured 2026-09-18: one click reported
        # `ui-stall: switch to panel-agent-7 took 1528.7s (limit 3.0s)` and the stall ended at the exact
        # second the agent resumed, while the guest's own uptime advanced 3.65 h across 6.28 h of wall
        # time. A monotonic clock stops with the machine, so it measures the work and not the pause
        # (qal-j26).
        t = time.monotonic()
        out = fn()
        dt = time.monotonic() - t
        if dt > limit:
            self.rec.broke("ui-stall", f"{what} took {dt:.1f}s (limit {limit}s)", "desktop")
        return out, dt

    def sessions(self):
        return [c for p in self.app.state()["projects"] for c in p["sessions"]]

    def new_chat(self, ix=0, timeout=20):
        """A new chat in the open project. `new-subchat` lives in the right-hand panel
        (`desktop/src/view/panel.rs`), and the panel is **closed** in a fresh window, so the leaf is
        absent until `toggle-panel` is clicked. Measured 2026-09-17 on `b1c8e82a62b1`: fresh window 33
        elements with `new-subchat` absent; after dismissing the first-run permissions sheet, 26 and still
        absent; after `toggle-panel`, 42 with `new-subchat` present and reachable.

        The old sidebar fallback (`hover project-<ix>` then `project-add-<ix>`) is gone: the sidebar was
        removed by the 2026-09-13 layout decision, so that branch could only ever fail — and because it
        was tried second, its `move: no element matches 'project-0'` became the error 16 desktop
        scenarios a cycle reported from 15 September, naming the fallback instead of the cause. A
        fallback that cannot succeed is worse than none (qal-j24)."""
        before = {c["id"] for c in self.sessions()}

        def leaves():
            return {str(e.get("path", "")).split(".")[-1] for e in self.app.elements("*")}

        # ⌘N first, the button second. The button has moved twice: out of the removed sidebar
        # (qal-j24), then out of reach entirely with `8d6cb643` ("the panel's tabs sit on the window
        # strip"), after which `new-subchat` renders in neither the fresh window nor the opened panel
        # and every desktop scenario in cycle 9 died on `timed out waiting for element new-subchat`.
        # Measured 2026-09-18 on the app at `1beec0a1fd98`: 30 leaves at launch and 37 with the panel
        # open, `new-subchat` absent from both; `new-tab` and `panel-new-tab` mint nothing even after
        # nine clicks; `app.key("cmd-n")` mints a chat with its own kernel agent first press. ⌘N is
        # the app's own documented shortcut for this control (panel.rs:1210, "New sub-chat ⌘N"), so it
        # is the affordance least likely to move next time.
        if "new-subchat" in leaves():
            self.app.click("new-subchat")
        else:
            self.app.key("cmd-n")
        st = self.app.wait_state(lambda s: {c["id"] for p in s["projects"] for c in p["sessions"]} - before, timeout=timeout, what="a new session")
        sid = (({c["id"] for p in st["projects"] for c in p["sessions"]}) - before).pop()
        # The kernel agent this chat belongs to. `new-subchat` **mints a new agent** (`chat-<ms>`, via
        # `desktop/src/kernel.rs::mint_chat` → `arbos_core::create_chat`); the project's main chat is
        # `root` and this one is not. A scenario that types here and then reads `root` is reading an
        # agent nobody spoke to — which is what made `qal-j27` look like lost words when the line was on
        # the minted agent's transcript all along (the features agent's read, 2026-09-18 05:55).
        self.agent = next(
            (c.get("agent_session") for p in st["projects"] for c in p["sessions"] if c["id"] == sid),
            None,
        )
        return sid

    def session_element(self, session_id):
        """The clickable element for a session. A sub-chat's row is `panel-agent-<session id>` in the
        right-hand panel, which must be open to exist; a project's own chat is `tab-<index>` in the
        window's strip. Measured 2026-09-17 on the app built from `b1c8e82a62b1`: two sub-chats made in
        a fresh window give `panel-agent-1`, `panel-agent-2`, `panel-agent-3` beside `panel-tab-0`,
        `panel-tabs` and `tab-0`/`tab-1`.

        The old version looked only in the tab bar and then for any element whose id merely *contained*
        the session number, so it reported `no clickable session row` for every sub-chat (qal-j24) — and
        the loose second pass could have returned an unrelated element whose id happened to share a
        digit."""
        sid = str(session_id)

        def paths():
            return {str(e.get("path", "")): e for e in self.app.elements("*")}

        def find(leaf):
            for path, el in paths().items():
                if path.split(".")[-1] == leaf:
                    return el["id"]
            return None

        row = find(f"panel-agent-{sid}")
        if row is None and find("toggle-panel") is not None and find("panel") is None:
            # The rows do not exist while the panel is shut, so open it before concluding.
            self.app.click("toggle-panel")
            row = find(f"panel-agent-{sid}")
        if row is not None:
            return row
        ids = [c["id"] for c in self.sessions()]
        if session_id in ids:
            return find(f"tab-{ids.index(session_id)}")
        return None

    def send(self, text):
        self.app.wait_element("composer-field", reachable=True)
        focus_composer(self.app)
        self.app.type(text + "\n")

    def close(self):
        try:
            if self.alive():
                self.app.close()
        except Exception:
            pass
        try:
            if self.app.process and self.app.process.poll() is None:
                self.app.process.kill()
        except Exception:
            pass
        # The desktop's kernel is a detached process by design; the scenario
        # ends it so the state check sees an idle place.
        pid = kernel_pid(self.cx.place)
        if pid:
            try:
                os.kill(pid, signal.SIGINT)
                for _ in range(50):
                    os.kill(pid, 0)
                    time.sleep(0.1)
                os.kill(pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        self.xvfb.terminate()


def kernel_pid(place):
    try:
        return int(json.loads((Path(place) / ".arbos" / "kernel.json").read_text())["pid"])
    except Exception:
        return None


def register(scenario, transcript, kinds, now_ms):
    @scenario("desktop-rapid-session-switch", tags=("desktop", "adversarial"))
    def s_switch(cx):
        """Create six chats and switch between them as fast as the driver allows. The app must stay up and responsive; each chat must keep its own folder."""
        if not available():
            cx.rec.notes["skipped"] = "desktop binary/driver/Xvfb missing"
            return
        d = Desktop(cx)
        try:
            ids = []
            for i in range(6):
                sid, dt = d.timed(f"new chat {i}", lambda: d.new_chat())
                ids.append(sid)
            d.shot("six-chats")
            cx.rec.notes["sessions"] = ids
            # Ask the helper for each chat's row rather than guessing at a naming: this line used to
            # look for `session-<index>`, the old sidebar's, and found nothing on any build since the
            # 2026-09-13 layout change — a copy of `session_element`'s job that rotted on its own while
            # the helper was there to be called (qal-j24).
            rows = [r for r in (d.session_element(sid) for sid in ids) if r]
            cx.rec.notes["session_rows"] = len(rows)
            switches = failures = 0
            for r in range(5):
                for row in rows:
                    name = row.rsplit(".", 1)[-1]
                    try:
                        _, dt = d.timed(f"switch to {name}", lambda: d.app.click(name), limit=3.0)
                        switches += 1
                    except Exception as e:
                        failures += 1
                        cx.rec.log(f"click {name}: {e}")
            cx.rec.notes["switches"] = switches
            cx.rec.notes["switch_failures"] = failures
            cx.rec.expect(switches > 0, "driver-gap", "no clickable session row; ids seen: " + ", ".join(d.app.ids()[:40]))
            cx.rec.expect(failures == 0, "ui-click-failed", f"{failures} session clicks were refused during rapid switching", "desktop sidebar")
            d.send("Reply with the single word SWITCHED.")
            time.sleep(3)
            d.shot("after-switching")
            cx.rec.expect(d.alive(), "app-died", f"desktop exited during rapid switching; log tail: {d.log.read_text(errors='replace')[-400:]!r}")
            folders = sorted(p.name for p in (cx.place / ".arbos" / "agents").iterdir() if p.is_dir())
            cx.rec.notes["agent_folders"] = folders
            cx.rec.expect(len([f for f in folders if f != "root"]) >= len(ids), "state:chat-folders", f"{len(ids)} chats created but folders are {folders}")
        finally:
            d.close()
        time.sleep(1)
        cx.check()

    @scenario("desktop-kill-kernel-under-ui", tags=("desktop", "adversarial"))
    def s_kill(cx):
        """SIGKILL the kernel while the window is attached, then send a prompt. The app must notice, respawn the kernel, and run the prompt."""
        if not available():
            cx.rec.notes["skipped"] = "desktop binary/driver/Xvfb missing"
            return
        d = Desktop(cx)
        try:
            d.new_chat()
            d.send("Reply with the single word FIRST.")
            time.sleep(4)
            pid = kernel_pid(cx.place)
            cx.rec.notes["kernel_pid_before"] = pid
            cx.rec.expect(pid is not None, "no-kernel", "the app never wrote a kernel for its project")
            if pid:
                os.kill(pid, signal.SIGKILL)
                time.sleep(1)
            d.shot("after-kill")
            d.send("Reply with the single word SECOND.")
            end = time.time() + 60
            new_pid = None
            while time.time() < end:
                new_pid = kernel_pid(cx.place)
                if new_pid and new_pid != pid:
                    try:
                        os.kill(new_pid, 0)
                        break
                    except ProcessLookupError:
                        pass
                time.sleep(0.5)
            cx.rec.notes["kernel_pid_after"] = new_pid
            cx.rec.expect(new_pid and new_pid != pid, "kernel-not-respawned", "after SIGKILL of the kernel and a new prompt, no new kernel appeared within 60s", "desktop/src/kernel.rs attach_or_spawn")
            d.shot("after-respawn")
            cx.rec.expect(d.alive(), "app-died", f"desktop exited after the kernel was killed; log tail: {d.log.read_text(errors='replace')[-400:]!r}")
            time.sleep(3)
            snap = d.app.state()
            items = [i for p in snap["projects"] for c in p["sessions"] for i in c.get("items", [])]
            cx.rec.notes["items_in_ui"] = len(items)
        finally:
            d.close()
        time.sleep(1)
        cx.check()


    def box(el):
        """(x, y, w, h) of a snapshot element, whatever the key names."""
        b = el.get("box") or el.get("bounds") or el
        if isinstance(b, dict):
            x = b.get("x", b.get("left", 0)); y = b.get("y", b.get("top", 0))
            w = b.get("w", b.get("width", 0)); h = b.get("h", b.get("height", 0))
            return float(x), float(y), float(w), float(h)
        if isinstance(b, (list, tuple)) and len(b) >= 4:
            return tuple(float(v) for v in b[:4])
        return 0.0, 0.0, 0.0, 0.0

    def elements_matching(d, needle):
        return [e for e in d.app.snapshot()["elements"] if needle in str(e.get("id", "")) or needle in str(e.get("path", ""))]

    @scenario("desktop-composer-pills", needs_model=True, tags=("desktop", "inbox", "composer-pills"))
    def s_pills(cx):
        """Feature composer-pills: a PR URL printed twice by bash counts once; a URL merely read from a file is recorded; the strip above the composer must not overflow a 900 px window."""
        if not available():
            cx.rec.notes["skipped"] = "desktop binary/driver/Xvfb missing"
            return
        cx.rec.notes["feature"] = "composer-pills"
        (cx.place / "notes.md").write_text("See https://github.com/unarbos/arbos/pull/7 for context.\n")
        d = Desktop(cx)
        try:
            # NOT a new sub-chat: `detail.rs:1924` returns no pills at all when
            # `chat.parent.is_some()` — "a subagent's chat in Cursor carries no pills; they are the
            # project's". This scenario used to open a sub-chat and then assert the project's pills in
            # it, so `pill-missing` was the app doing exactly what it says it does. The project's own
            # chat is the one that has them.
            pass  # the project's main chat is already open
            d.send("Run bash: `echo https://github.com/unarbos/arbos/pull/21`. Then run bash again: `echo https://github.com/unarbos/arbos/pull/21`. Then reply done.")
            # Wait for the fact, and record it: the pill is a response to a PR URL reaching the chat, so
            # a run where the model never ran bash proves nothing about pills. The old version slept 25 s
            # and asserted regardless — "no PRs pill after two bash outputs" with no evidence there were
            # two bash outputs, which is the `checkpoint_refs` defect (an assertion that never proved its
            # own setup) and a fixed sleep used to wait for a result.
            # The marker must be one only the system can produce. The first version of this wait
            # matched `pull/21` anywhere in the chat state and "landed" in 0.1 s — because the URL is in
            # the *prompt this scenario typed*, echoed back as the user's own line. That is `sb-01`'s
            # lesson (a marker the scenario wrote into the input proves nothing) and it was reproduced
            # here in the same file that records it. So: the URL must appear in an item that is not the
            # user's — a tool result or the agent's words.
            deadline = time.time() + 90
            trigger = None
            # A chat item is a flat dict with `kind` and `text` (as the journey reads them: `kind ==
            # "tool"`, `"agent"`, `"notice"`, `"user"`). The marker must come from a `tool` item — the
            # bash result — because the URL is in the user's own line and in `notes.md` too. Two earlier
            # versions of this wait matched the user's echo and "landed" in 0.1 s.
            while time.time() < deadline and trigger is None:
                for proj in (d.app.state().get("projects") or []):
                    for chat in (proj.get("sessions") or []):
                        for item in (chat.get("items") or []):
                            if item.get("kind") == "tool" and "pull/21" in json.dumps(item):
                                trigger = round(90 - (deadline - time.time()), 1)
                                cx.rec.notes["trigger_item_kind"] = item.get("kind")
                                break
                if trigger is None:
                    time.sleep(1.0)
            cx.rec.notes["trigger_landed_after_s"] = trigger
            if trigger is None:
                cx.rec.notes["skipped"] = "self: probe-trigger-never-landed — the PR URL never reached the chat within 90 s, so the model did not run the bash this scenario asks for and nothing can be concluded about the pills"
                return
            # The trigger landing is not the pill appearing: wait for the pill too, bounded, so
            # "absent" means absent after a fair chance rather than checked too early. (The version
            # before this one slept 25 s and never proved the trigger; the one before *that* checked the
            # instant the trigger landed, at 2.3 s, which is the opposite error.)
            pill_deadline = time.time() + 25
            pills = []
            while time.time() < pill_deadline:
                pills = elements_matching(d, "pill")
                if any("pill-prs" in str(e.get("path") or e.get("id")) for e in pills):
                    break
                time.sleep(1.0)
            cx.rec.notes["pill_wait_s"] = round(25 - (pill_deadline - time.time()), 1)
            cx.rec.notes["pill_ids"] = sorted({e["id"] for e in pills})[:10]
            cx.rec.notes["pill_elements"] = [{k: v for k, v in e.items() if k in ("id", "path", "text", "label", "title")} for e in pills][:6]
            prs = [e for e in pills if "pill-prs" in str(e.get("path") or e.get("id"))]
            cx.rec.expect(prs, "pill-missing", "no PRs pill after two bash outputs with a PR URL; pills seen: " + ", ".join(sorted({e['id'] for e in pills})[:8]), "desktop composer pills")
            count_text = " ".join(str(e.get(k, "")) for e in prs for k in ("text", "label", "title"))
            m = re.search(r"(\d+)", count_text)
            cx.rec.notes["prs_pill_text"] = count_text.strip() or None
            if m:
                cx.rec.expect(m.group(1) == "1", "pill-dedupe", f"the same URL printed twice counts as {m.group(1)}, not 1")
            d.shot("prs-pill")
            d.send("Read notes.md with the read tool and reply with its first word.")
            time.sleep(15)
            prs2 = [e for e in elements_matching(d, "pill-prs")]
            cx.rec.notes["prs_after_read_of_markdown"] = " ".join(str(e.get(k, "")) for e in prs2 for k in ("text", "label", "title")).strip() or ("pill present" if prs2 else None)
            d.shot("after-read")
            # Narrow window: nothing above the composer may run past the window edge.
            d.app.resize(900, 700)
            time.sleep(1.5)
            win = d.app.hello()["window"]
            over = []
            for e in d.app.snapshot()["elements"]:
                x, y, w, h = box(e)
                if w and x + w > float(win["width"]) + 1 and ("pill" in e["id"] or "composer" in e["id"] or "plan" in e["id"]):
                    over.append((e["id"], round(x + w)))
            cx.rec.notes["overflow_at_900px"] = over[:5]
            cx.rec.expect(not over, "layout-overflow", f"elements past the 900 px window edge: {over[:3]}", "desktop composer strip")
            d.shot("narrow-900")
            cx.rec.expect(d.alive(), "app-died", f"desktop exited; log tail: {d.log.read_text(errors='replace')[-300:]!r}")
        finally:
            d.close()
        time.sleep(1)
        cx.check()

    @scenario("desktop-user-message-card", tags=("desktop", "inbox", "user-message-card"))
    def s_card(cx):
        """Feature user-message-card: a short prompt's card hugs its text and sits right; a 200-word prompt wraps within the cap; nothing overflows the reading column."""
        if not available():
            cx.rec.notes["skipped"] = "desktop binary/driver/Xvfb missing"
            return
        cx.rec.notes["feature"] = "user-message-card"
        d = Desktop(cx)
        try:
            d.new_chat()
            d.send("hi")
            time.sleep(3)
            d.send(("Please consider this longer request carefully. " * 25).strip())
            time.sleep(4)
            snap = d.app.snapshot()
            def pth(e):
                return str(e.get("path") or e.get("id", ""))
            transcript = next((e for e in snap["elements"] if re.search(r"(^|\.)transcript-\d+$", pth(e))), None)
            prompts = [e for e in snap["elements"] if re.search(r"(^|\.)prompt-\d+-\d+$", pth(e))]
            cx.rec.notes["prompt_cards"] = len(prompts)
            cx.rec.expect(transcript is not None and len(prompts) >= 2, "driver-gap", f"transcript/prompt elements not found: {len(prompts)} prompt cards; paths: {[pth(e) for e in snap['elements'] if 'transcript' in pth(e)][:12]}")
            if transcript and len(prompts) >= 2:
                tx, ty, tw, th = box(transcript)
                widths = []
                for e in prompts[:2]:
                    x, y, w, h = box(e)
                    widths.append(round(w / tw, 2) if tw else None)
                    cx.rec.expect(x + w <= tx + tw + 1 and x >= tx - 1, "layout-overflow", f"{e['id']} spans {x:.0f}-{x + w:.0f} outside the column {tx:.0f}-{tx + tw:.0f}")
                    cx.rec.expect((x + w) >= tx + tw * 0.9, "card-not-right-aligned", f"{e['id']} ends at {x + w:.0f}, column ends at {tx + tw:.0f}")
                cx.rec.notes["card_width_ratio_short_long"] = widths
                short, long_ = widths
                cx.rec.expect(short is not None and short < 0.5, "card-short-too-wide", f"the 'hi' card takes {short} of the column; it should hug its text")
                cx.rec.expect(long_ is not None and long_ <= 0.85, "card-cap", f"the long card takes {long_} of the column; cap is ~0.8")
            d.shot("cards")
            cx.rec.expect(d.alive(), "app-died", f"desktop exited; log tail: {d.log.read_text(errors='replace')[-300:]!r}")
        finally:
            d.close()
        time.sleep(1)
        cx.check()

    @scenario("desktop-fresh-place-no-notice", tags=("desktop", "regression"))
    def s_fresh(cx):
        """Open a fresh place (no .arbos yet). The chat, board and terminal all attach at once; exactly one kernel must be spawned and no failed notice ("arbos-kernel exited") may appear in the chat or the transcript (qa-030)."""
        if not available():
            cx.rec.notes["skipped"] = "desktop binary/driver/Xvfb missing"
            return
        d = Desktop(cx)
        try:
            end = time.time() + 60
            while time.time() < end and kernel_pid(cx.place) is None:
                time.sleep(0.5)
            cx.rec.expect(kernel_pid(cx.place) is not None, "no-kernel", "the app never started a kernel for the fresh place")
            d.new_chat()
            time.sleep(6)
            d.shot("fresh-open")
            snap = d.app.state()
            items = [i for p in snap["projects"] for c in p["sessions"] for i in c.get("items", [])]
            failed = [i for i in items if i.get("kind") == "notice" and (i.get("failed") or "exited" in str(i.get("text", "")))]
            cx.rec.notes["ui_failed_notices"] = [str(i.get("text", ""))[:160] for i in failed]
            cx.rec.expect(not failed, "stale-spawn-notice-in-ui", f"failed notice(s) in a fresh chat: {cx.rec.notes['ui_failed_notices']}", "desktop/src/kernel.rs attach_or_spawn / wait_ready")
            agents = cx.place / ".arbos" / "agents"
            bad = []
            for tr in agents.glob("*/transcript.jsonl") if agents.exists() else []:
                for line in tr.read_text(errors="replace").splitlines():
                    if '"notice"' in line and ("arbos-kernel exited" in line or "already served" in line):
                        bad.append(line[:200])
            cx.rec.notes["transcript_spawn_notices"] = bad
            cx.rec.expect(not bad, "stale-spawn-notice-in-transcript", f"{len(bad)} transcript notice(s) about a kernel exit on a fresh place")
            klog = cx.place / ".arbos" / "kernel.log"
            text = klog.read_text(errors="replace") if klog.exists() else ""
            cx.rec.notes["lock_race_in_kernel_log"] = text.count("already served")
            cx.rec.expect("already served" not in text, "kernel-lock-race", "a second kernel was spawned for the same place and lost the lock", "desktop/src/kernel.rs attach_or_spawn: chat, board and terminal each spawn")
        finally:
            d.close()
        time.sleep(1)
        cx.check()

    @scenario("desktop-huge-transcript-scroll", tags=("desktop", "adversarial"))
    def s_scroll(cx):
        """A chat with 3,000 transcript lines is opened and scrolled hard. Each driver call must answer within seconds; the app must not die."""
        if not available():
            cx.rec.notes["skipped"] = "desktop binary/driver/Xvfb missing"
            return
        root = cx.place / ".arbos" / "agents" / "root"
        root.mkdir(parents=True, exist_ok=True)
        (root / "agent.md").write_text("name: root\nparent: \npaused: false\nmodel: inherit\nallowlist: ls, read, write, bash\nreadonly: false\ncwd: " + str(cx.place) + "\n")
        lines = []
        for i in range(1000):
            t = now_ms() - (1000 - i) * 60_000
            lines.append(json.dumps({"ts": t, "kind": "wake", "wake": "user", "text": f"q {i}"}))
            lines.append(json.dumps({"ts": t, "kind": "user", "text": f"Question {i}: what is {i} squared? " + "context " * 20}))
            lines.append(json.dumps({"ts": t + 1, "kind": "assistant", "text": f"{i} squared is {i * i}. " + "detail " * 30}))
            lines.append(json.dumps({"ts": t + 2, "kind": "turn_complete"}))
        (root / "transcript.jsonl").write_text("\n".join(lines) + "\n")
        t0 = time.monotonic()
        d = Desktop(cx)
        cx.rec.notes["launch_s"] = round(time.monotonic() - t0, 1)
        try:
            state, dt = d.timed("first state() with a 4000-line transcript", d.app.state, limit=10.0)
            d.shot("opened")
            cands = [i.rsplit(".", 1)[-1] for i in d.app.ids() if re.search(r"\.transcript-\d+$", i)]
            target = cands[0] if cands else None
            cx.rec.notes["scroll_target"] = target
            stalls = 0
            for i in range(60):
                dy = -800 if i % 20 < 10 else 800
                try:
                    _, dt = d.timed(f"scroll {i}", lambda: d.app.scroll(target, dy=dy) if target else d.app.scroll(x=800, y=500, dy=dy), limit=3.0)
                    if dt > 1.0:
                        stalls += 1
                except Exception as e:
                    cx.rec.broke("driver-error", f"scroll {i}: {e}", "desktop")
                    break
            cx.rec.notes["slow_scrolls_over_1s"] = stalls
            d.shot("after-scroll")
            cx.rec.expect(d.alive(), "app-died", f"desktop exited while scrolling a huge transcript; log tail: {d.log.read_text(errors='replace')[-400:]!r}")
            _, dt = d.timed("state() after scrolling", d.app.state, limit=5.0)
        finally:
            d.close()
        time.sleep(1)
        cx.check()
