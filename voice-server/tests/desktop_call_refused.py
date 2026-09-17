"""The desktop surfaces a refused call, two ways, and never talks to another project's kernel.

    ARBOS_DESKTOP_BIN=../desktop/target/debug/arbos-desktop python -m tests.desktop_call_refused

A. Off the hub, speech server elsewhere: the tab has no machine name and `voice_url` is not this
   computer, so the desktop refuses before dialing — a red `voice · call refused: … not on the hub …
   hub.toml …` line in the chat (the gateway would answer `project_not_on_hub`; nothing is dialed).
B. The gateway refuses: a loopback gateway started for another folder, and the tab's folder has no
   reachable kernel (its kernel.json is gone), so the gateway answers `error {code: project_offline}`
   and closes 4404 — the chat shows `voice · call refused: project_offline: <the gateway's words>`.
In both, no call is live afterwards and the gateway's own kernel got no user frames.
"""

from __future__ import annotations

import asyncio
import os
import shutil
import subprocess
import sys
from pathlib import Path

from tests.mock_duplex import MockDuplex
from tests.mock_kernel import MockKernel
from tests.run import Gateway, TOKEN

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
REPO = ROOT.parent
OUT = HERE / "out" / "desktop-refused"
DRIVER_DIR = Path(os.environ.get("ARBOS_DESKTOP_DRIVER", REPO / "desktop" / "driver"))


def screenshot(display: str, path: Path) -> None:
    subprocess.run(
        ["ffmpeg", "-y", "-loglevel", "error", "-f", "x11grab", "-video_size", "1500x950", "-i", display, "-frames:v", "1", str(path)],
        check=False, timeout=20,
    )


async def main_async() -> int:
    binary = os.environ.get("ARBOS_DESKTOP_BIN") or str(REPO / "desktop" / "target" / "debug" / "arbos-desktop")
    if not Path(binary).exists():
        print(f"no desktop binary at {binary}; build desktop/ or set ARBOS_DESKTOP_BIN", file=sys.stderr)
        return 2
    if not shutil.which("Xvfb"):
        print("Xvfb is not installed", file=sys.stderr)
        return 2
    sys.path.insert(0, str(DRIVER_DIR))
    import arbosdriver  # noqa: E402

    if OUT.exists():
        shutil.rmtree(OUT)
    OUT.mkdir(parents=True)
    tab_place = OUT / "discord_backups"  # the tab's folder: served by its own kernel
    tab_place.mkdir()
    other_place = OUT / "arboslife-demo"  # the gateway's own kernel: another project
    other_place.mkdir()
    mic_dir = OUT / "mic"
    mic_dir.mkdir()

    checks: list[tuple[bool, str]] = []

    def check(ok: bool, what: str) -> None:
        checks.append((bool(ok), what))
        print(f"   {'ok  ' if ok else 'FAIL'} {what}", flush=True)

    duplex, tab_kernel, other_kernel = MockDuplex(), MockKernel(tab_place), MockKernel(other_place)
    display = f":{9000 + os.getpid() % 900}"
    xvfb = subprocess.Popen(["Xvfb", display, "-screen", "0", "1500x950x24", "-nolisten", "tcp"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    gateway = app = None
    try:
        duplex_url = await duplex.start()
        await tab_kernel.start()
        other_url = await other_kernel.start()
        gateway = Gateway(duplex_url=duplex_url, kernel_url=other_url, log=OUT / "gateway.log", extra=["--kernel-place", str(other_place)])
        await gateway.start()

        xdg = OUT / "xdg"
        (xdg / "arbos").mkdir(parents=True)
        # A: the speech server is "elsewhere" — a non-loopback address nothing listens on. The
        # desktop must not dial it at all.
        far_url = gateway.url.replace("127.0.0.1", "10.255.255.1")
        (xdg / "arbos" / "config.toml").write_text(f'voice_url = "{far_url}"\nvoice_token = "{TOKEN}"\nvoice_mirror = false\n')
        env = {
            "DISPLAY": display,
            "ARBOS_VOICE_MIC_CMD": f"{sys.executable} -m tests.mic {mic_dir}",
            "ARBOS_VOICE_PLAYER_CMD": "cat > /dev/null",
            "PYTHONPATH": str(ROOT),
            "PATH": f"{Path(binary).parent}:{os.environ.get('PATH', '')}",
        }
        app = await asyncio.to_thread(
            arbosdriver.Arbos.launch, binary=binary, env=env, log=OUT / "app.log", xdg=xdg, projects=[str(tab_place)], timeout=120,
        )
        state = await asyncio.to_thread(app.wait_state, lambda s: s.get("active_session") is not None, 60, 0.5, "a chat")
        ix = next((p["index"] for p in state["projects"] if Path(p["path"]).resolve() == tab_place.resolve()), None)
        if ix is not None and not state["projects"][ix]["active"]:
            tabs = [e["path"] for e in app.elements("tab*")]
            target = next((t for t in tabs if t.split(".")[-1] in (f"tab-{ix}", f"tab/{ix}", f"tab:{ix}", f"tab_{ix}")), None)
            if target:
                await asyncio.to_thread(app.click, target)
            else:
                await asyncio.to_thread(app.key, "cmd-shift-]")
            state = await asyncio.to_thread(app.wait_state, lambda s: s["projects"][ix]["active"], 10, 0.2, "the tab in front")


        # The handset lives in the side panel's head; the panel is closed until asked (cmd-b).
        if not (state.get("panel") or {}).get("open"):
            await asyncio.to_thread(app.key, "cmd-b")
            state = await asyncio.to_thread(app.wait_state, lambda s: bool((s.get("panel") or {}).get("open")), 10, 0.2, "the panel open")
        await asyncio.to_thread(app.wait_state, lambda s: True, 1, 0.5, "a frame")
        def refusals(s: dict) -> list[dict]:
            chat = next((c for p in s["projects"] for c in p["sessions"] if c["id"] == s.get("active_session")), None)
            return [i for i in (chat["items"] if chat else []) if i.get("kind") == "notice" and "call refused" in i.get("text", "")]

        async def press_and_wait(n: int, label: str) -> tuple[dict, str]:
            await asyncio.to_thread(app.click, "panel-call")
            try:
                st = await asyncio.to_thread(app.wait_state, lambda s: s.get("call") is None and len(refusals(s)) >= n, 30, 0.25, f"refusal {label}")
                check(True, f"{label}: the call ends with no call state and a `voice · call refused` line in the chat")
            except Exception as exc:
                st = await asyncio.to_thread(app.state)
                check(False, f"{label}: the refusal reached the chat: {exc}")
            lines = refusals(st)
            line = lines[-1] if len(lines) >= n else {}
            text = line.get("text", "")
            print(f"   chat says ({label}):", text[:200])
            check(bool(line) and line.get("failed") is True, f"{label}: the refusal is drawn as a failed notice (red)")
            check(st.get("call") is None, f"{label}: no call is live")
            return st, text

        # A: off the hub, speech server elsewhere — refused before dialing.
        state, text = await press_and_wait(1, "A off-hub")
        check("not on the hub" in text and "hub.toml" in text, "A: the line says the computer is not on the hub and what to do (hub.toml)")
        check(not (OUT / "gateway.log").exists() or "session.start" not in (OUT / "gateway.log").read_text(errors="replace"), "A: nothing was dialed (the gateway saw no session.start)")
        await asyncio.sleep(0.4)
        screenshot(display, OUT / "01-refused-off-hub.png")

        # B: a loopback gateway that serves another folder, and the tab's kernel is unreachable
        # (its kernel.json gone): the gateway refuses with a code and words; the chat shows them.
        (xdg / "arbos" / "config.toml").write_text(f'voice_url = "{gateway.url}"\nvoice_token = "{TOKEN}"\nvoice_mirror = false\n')
        for kj in (tab_place / ".arbos" / "kernel.json", tab_place / ".arbos" / "runtime" / "kernel.json"):
            if kj.exists():
                kj.unlink()
        await asyncio.sleep(0.5)
        state, text = await press_and_wait(2, "B gateway")
        check("project_offline" in text or "project_not_on_hub" in text, "B: the line names the gateway's code (project_offline | project_not_on_hub)")
        check(":" in text and len(text) > 60, "B: the line carries the gateway's message")
        glog = (OUT / "gateway.log").read_text(errors="replace") if (OUT / "gateway.log").exists() else ""
        check("refused" in glog, "B: the gateway logged the refusal")
        check(len(other_kernel.users) == 0, f"the gateway's own kernel (another project) got no user frames ({len(other_kernel.users)})")
        await asyncio.sleep(0.4)
        screenshot(display, OUT / "02-refused-by-gateway.png")
    except Exception as exc:
        import traceback

        traceback.print_exc()
        check(False, f"{type(exc).__name__}: {exc}")
    finally:
        if app is not None:
            try:
                app.quit()
            except Exception:
                pass
            if app.process and app.process.poll() is None:
                app.process.kill()
        if gateway:
            gateway.stop()
        await duplex.stop()
        await tab_kernel.stop()
        await other_kernel.stop()
        xvfb.terminate()
    ok = all(o for o, _ in checks)
    print(f"\ndesktop-call-refused: {'PASS' if ok else 'FAIL'}  {sum(1 for o, _ in checks if o)}/{len(checks)} checks; screenshot in {OUT}")
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(asyncio.run(main_async()))
