"""The desktop's in-call UI, driven end to end with mocked speech: the app under Xvfb, attached to
the mock kernel's place, calling through a real gateway with the mock speech model.

    ARBOS_DESKTOP_BIN=../desktop/target/debug/arbos-desktop python -m tests.desktop_call [scenario]

Needs the desktop built (`cargo build` in desktop/), `Xvfb`, and ffmpeg for screenshots. The
desktop's microphone is `tests/mic.py` (utterance files dropped into a folder); its speaker is a
file. Checks, through the driver socket: the Call button starts a call, `state.call` reports the
phase, the caller's partial words and the narrator's last line, `voice ·` lines land in the chat,
Mute flips `muted`, End clears the call. Screenshots and the app log go to tests/out/desktop/.
"""

from __future__ import annotations

import asyncio
import json
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
OUT = HERE / "out" / "desktop"
DRIVER_DIR = Path(os.environ.get("ARBOS_DESKTOP_DRIVER", REPO / "desktop" / "driver"))


def screenshot(display: str, path: Path) -> None:
    subprocess.run(
        ["ffmpeg", "-y", "-loglevel", "error", "-f", "x11grab", "-video_size", "1500x950", "-i", display, "-frames:v", "1", str(path)],
        check=False, timeout=20,
    )


async def main_async(name: str) -> int:
    binary = os.environ.get("ARBOS_DESKTOP_BIN") or str(REPO / "desktop" / "target" / "debug" / "arbos-desktop")
    if not Path(binary).exists():
        print(f"no desktop binary at {binary}; build desktop/ or set ARBOS_DESKTOP_BIN", file=sys.stderr)
        return 2
    if not shutil.which("Xvfb"):
        print("Xvfb is not installed", file=sys.stderr)
        return 2
    sys.path.insert(0, str(DRIVER_DIR))
    import arbosdriver  # noqa: E402  (lives in desktop/driver, found at run time)

    sc = load_scenarios([name])[0]
    if OUT.exists():
        shutil.rmtree(OUT)
    OUT.mkdir(parents=True)
    place = OUT / "place"
    place.mkdir()
    mic_dir = OUT / "mic"
    mic_dir.mkdir()
    spoken_raw = OUT / "speaker.raw"

    steps = sc.get("steps", [])
    duplex, kernel = MockDuplex(), MockKernel(place)
    for step in steps:
        if "say" in step:
            duplex.script.append(Utterance(text=step["say"], response=model_response(step)))
        if "kernel" in step:
            kernel.script.append(kernel_behaviour(step["kernel"]))

    checks: list[tuple[bool, str]] = []

    def check(ok: bool, what: str) -> None:
        checks.append((bool(ok), what))
        print(f"   {'ok  ' if ok else 'FAIL'} {what}", flush=True)

    display = f":{9000 + os.getpid() % 900}"
    xvfb = subprocess.Popen(["Xvfb", display, "-screen", "0", "1500x950x24", "-nolisten", "tcp"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    gateway = app = None
    try:
        duplex_url = await duplex.start()
        kernel_url = await kernel.start()
        # The gateway is started for the folder, as on a real machine: a call naming that
        # folder is its own; any other bare name it refuses (project_not_on_hub).
        gateway = Gateway(duplex_url=duplex_url, kernel_url=kernel_url, log=OUT / "gateway.log", extra=["--kernel-place", str(place)])
        await gateway.start()

        xdg = OUT / "xdg"
        (xdg / "arbos").mkdir(parents=True)
        (xdg / "arbos" / "config.toml").write_text(
            f'voice_url = "{gateway.url}"\nvoice_token = "{TOKEN}"\nvoice_mirror = false\n'
        )
        env = {
            "DISPLAY": display,
            "ARBOS_VOICE_MIC_CMD": f"{sys.executable} -m tests.mic {mic_dir}",
            "ARBOS_VOICE_PLAYER_CMD": f"cat >> {spoken_raw}",
            "PYTHONPATH": str(ROOT),
            "PATH": f"{Path(binary).parent}:{os.environ.get('PATH', '')}",
        }
        app = await asyncio.to_thread(
            arbosdriver.Arbos.launch, binary=binary, env=env, log=OUT / "app.log", xdg=xdg, projects=[str(place)], timeout=120,
        )
        state = await asyncio.to_thread(app.wait_state, lambda s: s.get("active_session") is not None, 60, 0.5, "a chat")
        # The app lands on its Home tab; the call is for the mock kernel's place, so bring that
        # tab to the front first (a tab's element is `tab-<index>` in the strip).
        print("   projects:", [(p["index"], p["path"], p["active"]) for p in state["projects"]])
        ix = next((p["index"] for p in state["projects"] if Path(p["path"]).resolve() == place.resolve()), None)
        if ix is not None and not state["projects"][ix]["active"]:
            tabs = [e["path"] for e in app.elements("tab*")]
            print("   tab elements:", tabs[:8])
            target = next((t for t in tabs if t.split(".")[-1] in (f"tab-{ix}", f"tab/{ix}", f"tab:{ix}", f"tab_{ix}")), None)
            if target:
                await asyncio.to_thread(app.click, target)
            else:
                await asyncio.to_thread(app.key, "cmd-shift-]")
            state = await asyncio.to_thread(app.wait_state, lambda s: s["projects"][ix]["active"], 10, 0.2, "the place tab in front")
        check(state.get("call") is None, "no call before the button is pressed")
        # The handset lives in the side panel's head; the panel is closed until asked (cmd-b).
        if not (state.get("panel") or {}).get("open"):
            await asyncio.to_thread(app.key, "cmd-b")
            state = await asyncio.to_thread(app.wait_state, lambda s: bool((s.get("panel") or {}).get("open")), 10, 0.2, "the panel open")
        await asyncio.to_thread(app.wait_state, lambda s: True, 1, 0.5, "a frame")
        await asyncio.sleep(1.0)
        screenshot(display, OUT / "01-before-call.png")

        # The handset in the panel head.
        await asyncio.to_thread(app.click, "panel-call")
        state = await asyncio.to_thread(app.wait_state, lambda s: (s.get("call") or {}).get("connecting") is False, 30, 0.25, "call connected")
        call = state["call"]
        check(call["active"] and call["phase"] in ("listening", "ready"), f"call is live and listening ({call['phase']})")
        check(bool(call.get("mic_device")) and not call.get("mic_error"), f"the strip names the mic and reports no mic error (mic={call.get('mic_device')!r}, error={call.get('mic_error')!r})")
        speaker_seen = {"name": ""}
        await asyncio.sleep(0.8)
        screenshot(display, OUT / "02-in-call.png")

        for step in steps:
            if "say" in step:
                pcm = speech.utterance(step["say"], prefer="synthetic")
                (mic_dir / f"{time.time_ns()}.raw").write_bytes(pcm)
                await asyncio.sleep(speech.seconds(pcm) + 0.6)
            elif "wait_spoken" in step:
                needle = step["wait_spoken"].lower()

                def said(s: dict, needle=needle) -> bool:
                    chat = next((c for p in s["projects"] for c in p["sessions"] if c["id"] == s.get("active_session")), None)
                    items = chat["items"] if chat else []
                    lines = [i.get("text", "") for i in items if i.get("kind") == "notice"]
                    return any(needle in l.lower() for l in lines) or needle in str((s.get("call") or {}).get("last_said", "")).lower()

                try:
                    state = await asyncio.to_thread(app.wait_state, said, float(step.get("timeout", 30)), 0.3, f"spoken {needle!r}")
                    check(True, f"chat or strip shows {step['wait_spoken']!r}")
                except Exception as exc:
                    check(False, f"chat or strip shows {step['wait_spoken']!r}: {exc}")
                screenshot(display, OUT / f"03-{needle[:20].replace(' ', '-')}.png")
            elif "pause" in step:
                await asyncio.sleep(float(step["pause"]))
            elif "text" in step:
                await asyncio.to_thread(app.fill, "composer-field", step["text"])
                await asyncio.to_thread(app.key, "enter")
            elif "wait_quiet" in step:
                await asyncio.sleep(float(step["wait_quiet"]))

        state = await asyncio.to_thread(app.state)
        chat = next((c for p in state["projects"] for c in p["sessions"] if c["id"] == state.get("active_session")), None)
        notices = [i.get("text", "") for i in (chat["items"] if chat else []) if i.get("kind") == "notice"]
        voice_lines = [n for n in notices if n.startswith("voice ·")]
        check(any("call started" in n for n in voice_lines), "chat has the `voice · call started` line")
        check(len([n for n in voice_lines if "call started" not in n]) >= 1, f"chat has narrator lines as `voice ·` notices ({len(voice_lines)})")
        check(bool((state.get("call") or {}).get("last_said")), "state.call.last_said holds the narrator's last line")
        check(spoken_raw.exists() and spoken_raw.stat().st_size > 20000, f"reply audio reached the speaker ({spoken_raw.stat().st_size if spoken_raw.exists() else 0} bytes)")
        # Downlink: the app requested and played the audio itself (a speaker was opened, the byte
        # counter moved) and told the gateway when it was playing (client.speaking around playback).
        call = state.get("call") or {}
        check(bool(call.get("speaker_device")), f"the strip names the speaker ({call.get('speaker_device')!r})")
        check(int(call.get("played_bytes") or 0) > 20000, f"the app counted reply audio it played ({call.get('played_bytes')} bytes)")
        glog = (OUT / "gateway.log").read_text(errors="replace")
        check("client.speaking True" in glog and "client.speaking False" in glog, "the gateway received client.speaking true and false around playback")
        users = [i for i in (chat["items"] if chat else []) if i.get("kind") == "user"]
        spoken_items = [u for u in users if any(step.get("say", "") and step["say"] in u.get("text", "") for step in steps)]
        check(bool(spoken_items), f"the caller's spoken words appear in the chat as user cards ({len(users)} user cards)")
        check(all(u.get("channel") == "voice" for u in spoken_items) and bool(spoken_items), "spoken user cards carry channel = voice (the microphone mark)")
        # One card per utterance: the app shows the caller's words as they land and the kernel's
        # record of the forwarded message is that card's echo, not a second card (and never a
        # second turn: the app sends nothing itself for a spoken line).
        for step in steps:
            if step.get("say"):
                n = sum(1 for u in users if step["say"] in u.get("text", ""))
                check(n == 1, f"exactly one user card for the utterance {step['say'][:40]!r} ({n})")
        check(len(kernel.users) <= sum(1 for step in steps if step.get("say")) + sum(1 for step in steps if step.get("text")),
              f"the kernel got no more user frames than lines were spoken or typed ({len(kernel.users)})")
        print("   kernel user frames:", [(u.get("channel"), u.get("kind"), str(u.get("text") or u.get("body") or "")[:50]) for u in kernel.users])
        # The speech model's own words (small talk it answered itself) land in the chat too.
        model_lines = [n for n in voice_lines if "call started" not in n and "call ended" not in n]
        check(bool(model_lines), f"the chat has voice lines for what was said back ({len(model_lines)})")
        # The call is bound to this tab's folder: the gateway either says which folder
        # (session.ready.project_info.place) and it is this one, or says nothing (own kernel by
        # address). Another folder would have hung up before this point.
        bound = str(call.get("project_path") or "")
        check(not bound or Path(bound).resolve() == place.resolve(), f"state.call.project_path is this tab's folder or unsaid ({bound!r})")
        glog = (OUT / "gateway.log").read_text(errors="replace")
        check("project_not_on_hub" not in glog and "refused" not in glog, "the gateway did not refuse the call")

        await asyncio.to_thread(app.click, "call-mute")
        state = await asyncio.to_thread(app.wait_state, lambda s: (s.get("call") or {}).get("muted") is True, 10, 0.2, "muted")
        check(state["call"]["muted"], "Mute flips state.call.muted")
        screenshot(display, OUT / "04-muted.png")
        await asyncio.to_thread(app.click, "call-mute")
        await asyncio.to_thread(app.wait_state, lambda s: (s.get("call") or {}).get("muted") is False, 10, 0.2, "unmuted")

        await asyncio.to_thread(app.click, "call-end")
        state = await asyncio.to_thread(app.wait_state, lambda s: s.get("call") is None, 10, 0.2, "call ended")
        check(state.get("call") is None, "End clears the call")
        await asyncio.sleep(0.6)
        state = await asyncio.to_thread(app.state)
        chat = next((c for p in state["projects"] for c in p["sessions"] if c["id"] == state.get("active_session")), None)
        notices = [i.get("text", "") for i in (chat["items"] if chat else []) if i.get("kind") == "notice"]
        check(any("call ended" in n for n in notices), "chat has the `voice · call ended` line")
        screenshot(display, OUT / "05-after-call.png")
        (OUT / "state.json").write_text(json.dumps(state, indent=1)[:200000])
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
    print(f"\ndesktop-call ({name}): {'PASS' if ok else 'FAIL'}  {sum(1 for o, _ in checks if o)}/{len(checks)} checks; screenshots in {OUT}")
    return 0 if ok else 1


def main() -> None:
    name = sys.argv[1] if len(sys.argv) > 1 else "dispatch-highlight-drilldown"
    raise SystemExit(asyncio.run(main_async(name)))


if __name__ == "__main__":
    main()
