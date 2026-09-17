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


def available():
    return bool(DESKTOP_BIN and Path(DESKTOP_BIN).exists() and DRIVER_DIR and (Path(DRIVER_DIR) / "arbosdriver.py").exists() and shutil.which("Xvfb"))


class Desktop:
    """Xvfb + the app + the driver, all scoped to one scenario."""

    def __init__(self, cx, tag="desktop"):
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
        self.log = self.rec.dir / f"{tag}.app.log"
        self.app = arbosdriver.Arbos.launch(binary=hidden_store_binary(cx.scratch), env=env, log=self.log, xdg=cx.scratch / "xdg", projects=[str(cx.place)], timeout=90)
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
        t = time.time()
        out = fn()
        dt = time.time() - t
        if dt > limit:
            self.rec.broke("ui-stall", f"{what} took {dt:.1f}s (limit {limit}s)", "desktop")
        return out, dt

    def sessions(self):
        return [c for p in self.app.state()["projects"] for c in p["sessions"]]

    def new_chat(self, ix=0, timeout=20):
        """A new chat in the open project: `new-subchat` (symmetry cycles 11+; `new-tab` opens the
        project opener instead), else the old sidebar's `project-add-<ix>`. Returns the new session id."""
        before = {c["id"] for c in self.sessions()}
        leaves = {str(e.get("path", "")).split(".")[-1] for e in self.app.elements("*")}
        if "new-subchat" in leaves:
            self.app.click("new-subchat")
        else:
            self.app.hover(f"project-{ix}")
            self.app.wait_element(f"project-add-{ix}", reachable=True)
            self.app.click(f"project-add-{ix}")
        st = self.app.wait_state(lambda s: {c["id"] for p in s["projects"] for c in p["sessions"]} - before, timeout=timeout, what="a new session")
        return (({c["id"] for p in st["projects"] for c in p["sessions"]}) - before).pop()

    def session_element(self, session_id):
        """The clickable element for a session: its tab (`tab-<index>`) in the tab bar, else the
        old sidebar row that names the id."""
        sid = str(session_id)
        ids = [c["id"] for c in self.sessions()]
        if session_id in ids:
            cand = f"tab-{ids.index(session_id)}"
            for el in self.app.elements("*"):
                if str(el.get("path", "")).endswith(cand):
                    return el["id"]
        for el in self.app.elements():
            if sid in str(el.get("id", "")) or sid in str(el.get("path", "")):
                return el["id"]
        return None

    def send(self, text):
        self.app.wait_element("composer-field", reachable=True)
        self.app.click("composer-field")
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
            # Sidebar rows are `session-<index>` in list order.
            rows = [i for i in d.app.ids() if i.rsplit(".", 1)[-1].startswith("session-") and "dots" not in i]
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
            d.new_chat()
            d.send("Run bash: `echo https://github.com/unarbos/arbos/pull/21`. Then run bash again: `echo https://github.com/unarbos/arbos/pull/21`. Then reply done.")
            time.sleep(25)
            pills = elements_matching(d, "pill")
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
        t0 = time.time()
        d = Desktop(cx)
        cx.rec.notes["launch_s"] = round(time.time() - t0, 1)
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
