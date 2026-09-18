"""A message must never cross projects (2026-09-16, after #306 on the phone). The failure class is
pending work and identity: a kernel that dies rather than a link that drops, a queued line when a
place is closed and reopened, a hub attach whose kernel goes away. `xp-*`.

Desktop probes need a built app (ARBOS_DESKTOP_BIN, ARBOS_DESKTOP_DRIVER, Xvfb); they run without
a model key — a line still lands as a `user` transcript line, which is all these checks read.
"""

import json
import os
import shutil
import signal
import socket
import subprocess
import sys
import time
from pathlib import Path

import desktop_scenarios

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


def desktop_available():
    return bool(DESKTOP_BIN and Path(DESKTOP_BIN).exists() and DRIVER_DIR and (Path(DRIVER_DIR) / "arbosdriver.py").exists() and shutil.which("Xvfb"))


def user_lines(place, agent="root"):
    out = []
    for base in (Path(place) / ".arbos" / "agents" / agent, Path(place) / ".arbos" / "archive" / "agents" / agent):
        tr = base / "transcript.jsonl"
        if tr.exists():
            for l in tr.read_text(errors="replace").splitlines():
                try:
                    e = json.loads(l)
                except Exception:  # noqa: BLE001
                    continue
                if e.get("kind") == "user":
                    out.append(e.get("text", ""))
    return out


def all_text(place):
    """Every byte under a place's .arbos that could carry a user's words (transcripts, inbox, logs)."""
    chunks = []
    for p in (Path(place) / ".arbos").rglob("*"):
        if p.is_file() and p.suffix in (".jsonl", ".md", ".log", ".json", ".toml"):
            try:
                chunks.append(p.read_text(errors="replace"))
            except OSError:
                pass
    return "\n".join(chunks)


def kernel_pid(place):
    for p in (Path(place) / ".arbos" / "runtime" / "kernel.json", Path(place) / ".arbos" / "kernel.json"):
        if p.exists():
            try:
                return int(json.loads(p.read_text())["pid"])
            except Exception:  # noqa: BLE001
                return None
    return None


class TwoProjects:
    """Xvfb + the app opened on two places at once."""

    def __init__(self, cx, a, b, tag="desktop"):
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
        self.log = cx.rec.dir / f"{tag}.app.log"
        self.app = arbosdriver.Arbos.launch(binary=hidden_store_binary(cx.scratch), env=env, log=self.log, xdg=cx.scratch / "xdg", projects=[str(a), str(b)], timeout=90)
        cx.rec.log(f"{tag}: app pid {self.app.hello().get('pid')} on {self.display}")

    def project_index(self, place):
        for i, p in enumerate(self.app.state()["projects"]):
            if str(p.get("path", "")).rstrip("/") == str(place).rstrip("/"):
                return i
        return None

    def focus(self, place):
        """Make `place`'s chat the active one by clicking its tab; returns True when the state agrees."""
        ix = self.project_index(place)
        if ix is None:
            return False
        for cand in (f"tab-{ix}",):
            try:
                self.app.click(cand)
            except Exception:  # noqa: BLE001
                continue
        st = self.app.wait_state(lambda s: s.get("active_project") == ix, timeout=10, what=f"project {ix} active")
        return st.get("active_project") == ix

    def send(self, text):
        """Type a line and say what the app did with it.

        `xp-01` reported `first-line-lost` — a data-loss rule — in every cycle from 2026-09-17 18:18
        to 2026-09-18 11:45, and nobody could act on it, because typing without looking cannot tell
        three failures apart: the click never focused the field (this rig's fault), the line sits in
        the composer unsent (a refusal the person can see), or the app took the line and it reached no
        transcript (real loss). The app's own state carries `composer.text` and `composer.focused`
        (`desktop/src/driver.rs:1273`), so all three are separable for two reads. It was the first.
        """
        self.app.wait_element("composer-field", reachable=True)
        got = desktop_scenarios.focus_composer(self.app)
        self.app.type(text + "\n")
        left, deadline = "", time.time() + 5
        while time.time() < deadline:
            left = str((self.app.state().get("composer") or {}).get("text") or "")
            if not left:
                break
            time.sleep(0.25)
        return {"focused_after_click": got["focused"], "clicks_to_focus": got["clicks"], "composer_left_holding": left[:120], "accepted": got["focused"] and not left}

    def close(self, places=()):
        try:
            self.app.close()
        except Exception:  # noqa: BLE001
            pass
        time.sleep(1)
        # The app's kernels outlive it; stop them so the place checks see a quiet folder.
        for place in list(places) + [self.cx.scratch / "home" / ".arbos"]:
            pid = kernel_pid(place)
            if pid:
                try:
                    os.kill(pid, signal.SIGINT)
                except ProcessLookupError:
                    pass
        time.sleep(2)
        self.xvfb.terminate()


def register(scenario, registry, transcript, now_ms, branch):
    def reg(name, tags=(), desktop=False):
        def deco(fn):
            scenario(name, needs_model=False, tags=("cross-project",) + (("desktop",) if desktop else ()) + tuple(tags))(fn)
            registry[name]["branch"] = branch
            return fn

        return deco

    @reg("xp-01-dead-kernel-line-stays-in-its-project", desktop=True)
    def s01(cx):
        """Two places open. Kill A's kernel, type into A, switch to B and type there. A's line must reach only A (after its kernel respawns); B's only B; neither place's files may carry the other's words."""
        if not desktop_available():
            cx.rec.notes["skipped"] = "desktop binary/driver/Xvfb missing"
            return
        a = cx.scratch / "alpha"
        b = cx.scratch / "beta"
        a.mkdir()
        b.mkdir()
        tag = f"XP{now_ms() % 100000}"
        d = TwoProjects(cx, a, b)
        try:
            cx.rec.expect(d.focus(a), "focus-a", "could not make A the active project")
            sent_first = d.send(f"A-first-{tag}: reply with the word ALPHA.")
            cx.rec.notes["first_line_send"] = sent_first
            # Let the first line land and its (keyless) turn end before the kernel dies.
            end = time.time() + 60
            while time.time() < end and not any(f"A-first-{tag}" in u for u in user_lines(a)):
                time.sleep(0.5)
            first_landed_at = round(time.time() - (end - 60), 1)
            pid_a = kernel_pid(a)
            cx.rec.notes["first_line_landed_after_s"] = first_landed_at if any(f"A-first-{tag}" in u for u in user_lines(a)) else None
            cx.rec.expect(pid_a is not None, "no-kernel-a", "A never got a kernel")
            time.sleep(3)
            if pid_a:
                os.kill(pid_a, signal.SIGKILL)
            time.sleep(1.5)
            # The user keeps typing into A while its kernel is dead, then moves to B.
            d.send(f"A-after-death-{tag}: reply with ALPHA again.")
            time.sleep(1)
            cx.rec.expect(d.focus(b), "focus-b", "could not make B the active project")
            d.send(f"B-only-{tag}: reply with the word BETA.")
            time.sleep(25)
            home = cx.scratch / "home" / ".arbos"
            ua, ub, uh = user_lines(a), user_lines(b), user_lines(home)
            ta, tb = all_text(a), all_text(b)
            cx.rec.notes.update({"a_user_lines": ua, "b_user_lines": ub, "home_store_user_lines": uh, "a_kernel_before": pid_a, "a_kernel_after": kernel_pid(a)})
            cx.rec.expect(not any(tag in u for u in uh), "xp-01-line-in-home-store", f"a line typed into A or B ran in the home store (~/.arbos), the project that was active at launch: {[u for u in uh if tag in u]}", "desktop: the composer's target must be the chat the words were typed into")
            # Three failures wore one name until 2026-09-18; each gets its own, and only the last is loss.
            cx.rec.expect(
                sent_first["focused_after_click"],
                "probe-composer-never-focused",
                f"clicking `composer-field` never focused it, so the first line was typed at nothing and this run says nothing about the app: {sent_first}",
            )
            if sent_first["focused_after_click"] and sent_first["composer_left_holding"]:
                cx.rec.expect(
                    False,
                    "xp-01-first-line-stuck-in-the-composer",
                    f"the first line is still in the composer 5 s after Enter ({sent_first['composer_left_holding']!r}); the app did not take it. The person can see it, so not silent loss — but the line does not go",
                    "desktop composer: Enter on a project whose kernel is not yet up must either send or say why",
                )
            elif sent_first["accepted"]:
                cx.rec.expect(
                    any(f"A-first-{tag}" in u for u in ua),
                    "xp-01-first-line-lost",
                    f"the app took A's first line — composer focused, emptied on Enter — and it is on no transcript and in no store: A={ua} home={uh} B={ub}. A line a person typed, gone with nothing said",
                    "desktop session: a send while the project's kernel is still coming up must be held and delivered, not dropped",
                )
            dup = [u for u in set(ua) if ua.count(u) > 1]
            cx.rec.expect(not dup, "xp-01-line-duplicated", f"a line reached A twice after its kernel was replaced: {dup}", "desktop session: a send to a dead connection must not be replayed on the new one as well")
            cx.rec.expect(not any("A-" in u for u in ub), "xp-01-a-line-in-b", f"A's words ran in B: {[u for u in ub if 'A-' in u]}", "desktop session: a line typed into a project whose kernel is down must stay with that project")
            cx.rec.expect(not any("B-only" in u for u in ua), "xp-01-b-line-in-a", f"B's words ran in A: {[u for u in ua if 'B-only' in u]}")
            cx.rec.expect(f"A-after-death-{tag}" not in tb and f"B-only-{tag}" not in ta, "xp-01-words-cross-in-files", "the other project's words appear somewhere under this project's .arbos")
            cx.rec.expect(any(f"A-after-death-{tag}" in u for u in ua), "xp-01-a-line-lost", f"the line typed into A while its kernel was dead never reached A: {ua}", "desktop kernel respawn on send")
        finally:
            d.close(places=[a, b])
            for name, place in (("alpha", a), ("beta", b), ("home-store", cx.scratch / "home" / ".arbos")):
                try:
                    cx.rec.snapshot(place, f"{name}-after")
                except Exception:  # noqa: BLE001
                    pass
        cx.check(place=a)

    @reg("xp-02-queued-line-survives-close-and-reopen-in-place", desktop=True)
    def s02(cx):
        """Type a follow-up while A's turn is queued behind a dead kernel, close A's tab, work in B, reopen A. The follow-up runs in A or is dropped with a notice; it never runs in B."""
        if not desktop_available():
            cx.rec.notes["skipped"] = "desktop binary/driver/Xvfb missing"
            return
        a = cx.scratch / "alpha"
        b = cx.scratch / "beta"
        a.mkdir()
        b.mkdir()
        tag = f"XQ{now_ms() % 100000}"
        d = TwoProjects(cx, a, b)
        try:
            cx.rec.expect(d.focus(a), "focus-a", "could not make A the active project")
            d.send(f"A-first-{tag}")
            end = time.time() + 40
            while time.time() < end and kernel_pid(a) is None:
                time.sleep(0.5)
            pid_a = kernel_pid(a)
            if pid_a:
                os.kill(pid_a, signal.SIGKILL)
            time.sleep(1.5)
            d.send(f"A-queued-{tag}: this must run in A")
            time.sleep(0.5)
            ix = d.project_index(a)
            try:
                d.app.click(f"tab-close-{ix}")
            except Exception as e:  # noqa: BLE001
                cx.rec.notes["close_error"] = str(e)[:120]
            time.sleep(1.5)
            cx.rec.expect(d.focus(b), "focus-b", "could not make B the active project")
            d.send(f"B-work-{tag}: reply with BETA.")
            time.sleep(8)
            # Reopen A through the opener.
            reopened = False
            try:
                d.app.click("new-tab")
                time.sleep(1)
                rows = [e for e in d.app.elements("*") if str(e.get("path", "")).split(".")[-1].startswith("opener-row-")]
                for r in rows:
                    if "alpha" in json.dumps(r):
                        d.app.click(r["id"])
                        reopened = True
                        break
                if not reopened and rows:
                    d.app.click(rows[0]["id"])
                    reopened = d.project_index(a) is not None
            except Exception as e:  # noqa: BLE001
                cx.rec.notes["reopen_error"] = str(e)[:160]
            time.sleep(20)
            home = cx.scratch / "home" / ".arbos"
            ua, ub, uh = user_lines(a), user_lines(b), user_lines(home)
            cx.rec.notes.update({"reopened": reopened, "a_user_lines": ua, "b_user_lines": ub, "home_store_user_lines": uh})
            cx.rec.expect(not any(tag in u for u in uh), "xp-02-line-in-home-store", f"a line typed into A or B ran in the home store: {[u for u in uh if tag in u]}")
            dup = [u for u in set(ua) if ua.count(u) > 1]
            cx.rec.expect(not dup, "xp-02-line-duplicated", f"a queued line reached A twice: {dup}")
            cx.rec.expect(not any(f"A-queued-{tag}" in u for u in ub), "xp-02-queued-line-in-b", "the follow-up queued in A ran in B after A was closed", "desktop follow-up queue: identity must travel with the line")
            cx.rec.expect(f"A-queued-{tag}" not in all_text(b), "xp-02-words-in-b-files", "A's queued words appear under B's .arbos")
            landed = any(f"A-queued-{tag}" in u for u in ua)
            cx.rec.notes["queued_landed_in_a"] = landed
            if not landed:
                cx.rec.log("the queued line did not run in A after reopen (dropped); acceptable if the user was told — check the UI items")
        finally:
            d.close(places=[a, b])
            for name, place in (("alpha", a), ("beta", b), ("home-store", cx.scratch / "home" / ".arbos")):
                try:
                    cx.rec.snapshot(place, f"{name}-after")
                except Exception:  # noqa: BLE001
                    pass
        cx.check(place=a)

    @reg("xp-03-hub-attach-does-not-follow-to-another-project")
    def s03(cx):
        """Through a local hub: attach to qa-b/alpha, kill alpha's kernel, send a user line. It must not reach beta (the other project on the same machine); the client must be told the kernel is gone."""
        hub_bin = os.environ.get("ARBOS_QA_HUB_BIN", str(Path(cx.binary).parent / "arbos-hub"))
        if not Path(hub_bin).exists():
            cx.rec.notes["skipped"] = f"no arbos-hub at {hub_bin}"
            return
        try:
            import websockets  # noqa: F401
        except Exception:  # noqa: BLE001
            cx.rec.notes["skipped"] = "websockets module missing in this python"
            return
        s = socket.socket()
        s.bind(("127.0.0.1", 0))
        port = s.getsockname()[1]
        s.close()
        hub_cfg = cx.scratch / "hub-server.toml"
        hub_cfg.write_text(f'bind = "127.0.0.1:{port}"\n\n[[machine]]\nname = "qa-b"\ntoken = "machine-b-secret-qa-loopback-only"\n\n[[client]]\nname = "qa-client"\ntoken = "client-secret-1-qa-loopback-only"\nrole = "owner"\n')
        hub = subprocess.Popen([hub_bin, "--config", str(hub_cfg), "--bind", f"127.0.0.1:{port}"], stdout=open(cx.rec.dir / "hub.log", "ab"), stderr=subprocess.STDOUT)
        time.sleep(1.0)
        cfg_b = cx.scratch / "xdg-b" / "arbos"
        cfg_b.mkdir(parents=True)
        cfg_b.joinpath("config.toml").write_text((cx.scratch / "xdg" / "arbos" / "config.toml").read_text())
        cfg_b.joinpath("hub.toml").write_text(f'url = "ws://127.0.0.1:{port}"\nmachine = "qa-b"\ntoken = "machine-b-secret-qa-loopback-only"\n')
        env_b = dict(cx.env)
        env_b["XDG_CONFIG_HOME"] = str(cx.scratch / "xdg-b")
        alpha = cx.scratch / "alpha"
        beta = cx.scratch / "beta"
        for p in (alpha, beta):
            (p / ".arbos").mkdir(parents=True)
        ka = cx.kernel(tag="kernel-alpha", place=alpha)
        ka.env = env_b
        kb = cx.kernel(tag="kernel-beta", place=beta)
        kb.env = env_b
        cx.rec.expect(ka.start() and kb.start(), "kernels-start", "alpha/beta kernels did not come up")
        time.sleep(2.5)
        import asyncio
        import websockets

        tag = f"XH{now_ms() % 100000}"
        result = {}

        async def go():
            url = f"ws://127.0.0.1:{port}/attach/qa-b/alpha?token=client-secret-1-qa-loopback-only"
            async with websockets.connect(url, open_timeout=10) as ws:
                first = await asyncio.wait_for(ws.recv(), 10)
                result["first"] = str(first)[:160]
                await ws.send(json.dumps({"type": "user", "agent": "root", "text": f"alpha-alive-{tag}", "steer": False, "attachments": []}))
                await asyncio.sleep(2)
                ka.kill()
                await asyncio.sleep(2)
                try:
                    await ws.send(json.dumps({"type": "user", "agent": "root", "text": f"after-alpha-died-{tag}", "steer": False, "attachments": []}))
                    got = []
                    for _ in range(6):
                        try:
                            got.append(str(await asyncio.wait_for(ws.recv(), 3))[:200])
                        except asyncio.TimeoutError:
                            break
                    result["after_kill"] = got
                except Exception as e:  # noqa: BLE001
                    result["after_kill_error"] = f"{type(e).__name__}: {e}"[:160]

        try:
            asyncio.run(go())
        except Exception as e:  # noqa: BLE001
            result["error"] = f"{type(e).__name__}: {e}"[:200]
        time.sleep(2)
        ua, ub = user_lines(alpha), user_lines(beta)
        cx.rec.notes.update(result)
        cx.rec.notes.update({"alpha_user_lines": ua, "beta_user_lines": ub})
        cx.rec.expect(any(f"alpha-alive-{tag}" in u for u in ua), "xp-03-hub-attach-broken", f"a line through the hub to alpha did not reach alpha while it lived: {result}")
        cx.rec.expect(not any(tag in u for u in ub), "xp-03-line-crossed-to-beta", f"a line sent to alpha reached beta: {ub}", "arbos-hub: a client's channel is bound to one kernel; a dead kernel's client must not be re-routed")
        told = any("error" in g or "closed" in g.lower() or "gone" in g.lower() for g in result.get("after_kill", [])) or "after_kill_error" in result
        cx.rec.expect(told, "xp-03-client-not-told", f"after alpha's kernel died the client was not told (no error/close): {result.get('after_kill')}")
        kb.stop()
        hub.terminate()
        cx.check(place=beta)
