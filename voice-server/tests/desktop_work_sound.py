"""The sound of work on a desktop call, under Xvfb: plays only while the gateway's agent.activity
says the agent is working, ducks under a reply, and stops when the turn ends.

    ARBOS_DESKTOP_BIN=... python -m tests.desktop_work_sound

Uses the work-sound-activity scenario (a 6.5 s tool call) with the speaker as a file: the file
must grow while work runs and nothing is spoken, and stop growing once the agent is idle.
"""

from __future__ import annotations

import asyncio
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

from tests import speech
from tests.mock_duplex import MockDuplex, Utterance
from tests.mock_kernel import MockKernel
from tests.run import Gateway, TOKEN, kernel_behaviour, load_scenarios, model_response

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
REPO = ROOT.parent
OUT = HERE / "out" / "desktop-work-sound"
DRIVER_DIR = Path(os.environ.get("ARBOS_DESKTOP_DRIVER", REPO / "desktop" / "driver"))


async def main_async() -> int:
    binary = os.environ.get("ARBOS_DESKTOP_BIN") or str(REPO / "desktop" / "target" / "debug" / "arbos-desktop")
    if not Path(binary).exists() or not shutil.which("Xvfb"):
        print("needs a built desktop and Xvfb", file=sys.stderr)
        return 2
    sys.path.insert(0, str(DRIVER_DIR))
    import arbosdriver  # noqa: E402

    sc = load_scenarios(["work-sound-activity"])[0]
    if OUT.exists():
        shutil.rmtree(OUT)
    OUT.mkdir(parents=True)
    place = OUT / "place"
    place.mkdir()
    mic_dir = OUT / "mic"
    mic_dir.mkdir()
    spoken_raw = OUT / "speaker.raw"
    duplex, kernel = MockDuplex(), MockKernel(place)
    for step in sc["steps"]:
        if "say" in step:
            duplex.script.append(Utterance(text=step["say"], response=model_response(step)))
        if "kernel" in step:
            kernel.script.append(kernel_behaviour(step["kernel"]))
    checks: list[tuple[bool, str]] = []

    def check(ok: bool, what: str) -> None:
        checks.append((bool(ok), what))
        print(f"   {'ok  ' if ok else 'FAIL'} {what}", flush=True)

    def size() -> int:
        return spoken_raw.stat().st_size if spoken_raw.exists() else 0

    display = f":{9300 + os.getpid() % 600}"
    xvfb = subprocess.Popen(["Xvfb", display, "-screen", "0", "1500x950x24", "-nolisten", "tcp"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    gateway = app = None
    try:
        duplex_url = await duplex.start()
        kernel_url = await kernel.start()
        gateway = Gateway(duplex_url=duplex_url, kernel_url=kernel_url, log=OUT / "gateway.log", extra=[])
        await gateway.start()
        xdg = OUT / "xdg"
        (xdg / "arbos").mkdir(parents=True)
        sound = os.environ.get("WORK_SOUND", "bed")  # bed | ticks: which to exercise (and sample)
        (xdg / "arbos" / "config.toml").write_text(f'voice_url = "{gateway.url}"\nvoice_token = "{TOKEN}"\nvoice_mirror = false\nvoice_work_sound = "{sound}"\n')
        env = {
            "DISPLAY": display,
            "ARBOS_VOICE_MIC_CMD": f"{sys.executable} -m tests.mic {mic_dir}",
            "ARBOS_VOICE_PLAYER_CMD": f"cat >> {spoken_raw}",
            "PYTHONPATH": str(ROOT),
            "PATH": f"{Path(binary).parent}:{os.environ.get('PATH', '')}",
        }
        app = await asyncio.to_thread(arbosdriver.Arbos.launch, binary=binary, env=env, log=OUT / "app.log", xdg=xdg, projects=[str(place)], timeout=120)
        state = await asyncio.to_thread(app.wait_state, lambda s: len(s["projects"]) >= 2 and s.get("active_session") is not None, 60, 0.5, "both tabs")
        ix = next(p["index"] for p in state["projects"] if Path(p["path"]).resolve() == place.resolve())
        if not state["projects"][ix]["active"]:
            await asyncio.to_thread(app.click, f"tab-{ix}")
            await asyncio.to_thread(app.wait_state, lambda s: s["projects"][ix]["active"], 10, 0.2, "tab")
        await asyncio.to_thread(app.click, "panel-call")
        state = await asyncio.to_thread(app.wait_state, lambda s: (s.get("call") or {}).get("connecting") is False, 30, 0.25, "call connected")
        work = state["call"]["work"]
        check(work["sound"] == sound and not work["active"], f"before any work: sound={work['sound']}, active={work['active']}")
        await asyncio.sleep(1.5)
        quiet_before = size()
        await asyncio.sleep(1.5)
        check(size() == quiet_before, f"nothing plays while the agent is idle ({size() - quiet_before} bytes in 1.5 s)")

        say = next(step["say"] for step in sc["steps"] if "say" in step)
        pcm = speech.utterance(say, prefer="synthetic")
        (mic_dir / f"{time.time_ns()}.raw").write_bytes(pcm)
        state = await asyncio.to_thread(app.wait_state, lambda s: (s.get("call") or {}).get("work", {}).get("active") is True, speech.seconds(pcm) + 15, 0.2, "work active")
        check(True, f"work turns active when the agent's turn starts (agents {state['call']['work']['agents']})")
        # The bed: the speaker file grows while the tool runs and nothing is being said.
        t0 = time.monotonic()
        s0 = size()
        await asyncio.sleep(2.0)
        state = await asyncio.to_thread(app.state)
        grew = size() - s0
        phase = (state.get("call") or {}).get("phase")
        # A bed streams the whole time; ticks come every 2.5 s, so any growth at all is the check there.
        enough = grew > 24_000 * 2 if sound == "bed" else grew > 0
        check(enough and phase != "speaking", f"the {sound} plays while the tool runs and nobody speaks ({grew} bytes in 2 s, phase {phase})")
        saw_tool = any(a.startswith("root:tool") for a in (state.get("call") or {}).get("work", {}).get("agents", []))
        check(saw_tool, f"the strip knows the main agent is in a tool ({state['call']['work']['agents']})")
        state = await asyncio.to_thread(app.wait_state, lambda s: (s.get("call") or {}).get("work", {}).get("active") is False, 40, 0.2, "work idle")
        check(True, "work goes idle when the turn ends")
        # Let the reply ("All 30 tests pass.") finish, then the file must not grow.
        await asyncio.to_thread(app.wait_state, lambda s: (s.get("call") or {}).get("phase") != "speaking", 20, 0.2, "reply over")
        await asyncio.sleep(1.0)
        s1 = size()
        await asyncio.sleep(1.5)
        check(size() == s1, f"nothing plays once the agent is idle again ({size() - s1} bytes in 1.5 s)")
        subprocess.run(["ffmpeg", "-y", "-loglevel", "error", "-f", "x11grab", "-video_size", "1500x950", "-i", display, "-frames:v", "1", str(OUT / "after.png")], check=False, timeout=20)
        await asyncio.to_thread(app.click, "call-end")
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
        await kernel.stop()
        xvfb.terminate()
    ok = all(o for o, _ in checks)
    print(f"\ndesktop-work-sound: {'PASS' if ok else 'FAIL'}  {sum(1 for o, _ in checks if o)}/{len(checks)} checks; artifacts in {OUT}")
    return 0 if ok else 1


def main() -> None:
    raise SystemExit(asyncio.run(main_async()))


if __name__ == "__main__":
    main()
