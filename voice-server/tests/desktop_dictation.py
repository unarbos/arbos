"""Fn dictation through the gateway's streaming recogniser, on the desktop under Xvfb.

    ARBOS_DESKTOP_BIN=... python -m tests.desktop_dictation

The gateway runs `--asr mock` with a scripted sentence (dictation mode is the ASR pipeline on any
engine, with partials as the words come and a final on release). The app's mic is `tests/mic.py`.
The test presses the composer's mic, drops the utterance, watches the composer fill with partial
words, releases, and checks the final text. Measures first-partial latency from the start of the
utterance and word error rate of the final against the scripted sentence. Real-model numbers come
from `tests.live_dictation` against the pod.
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
from tests.mock_duplex import MockDuplex
from tests.mock_kernel import MockKernel
from tests.run import Gateway, TOKEN

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
REPO = ROOT.parent
OUT = HERE / "out" / "desktop-dictation"
DRIVER_DIR = Path(os.environ.get("ARBOS_DESKTOP_DRIVER", REPO / "desktop" / "driver"))
SENTENCE = "the quick brown fox jumps over the lazy dog near the river bank"


def wer(ref: str, hyp: str) -> float:
    r, h = ref.lower().split(), hyp.lower().replace(".", "").replace(",", "").split()
    d = [[0] * (len(h) + 1) for _ in range(len(r) + 1)]
    for i in range(len(r) + 1):
        d[i][0] = i
    for j in range(len(h) + 1):
        d[0][j] = j
    for i in range(1, len(r) + 1):
        for j in range(1, len(h) + 1):
            d[i][j] = min(d[i - 1][j] + 1, d[i][j - 1] + 1, d[i - 1][j - 1] + (r[i - 1] != h[j - 1]))
    return d[len(r)][len(h)] / max(1, len(r))


async def main_async() -> int:
    binary = os.environ.get("ARBOS_DESKTOP_BIN") or str(REPO / "desktop" / "target" / "debug" / "arbos-desktop")
    if not Path(binary).exists() or not shutil.which("Xvfb"):
        print("needs a built desktop and Xvfb", file=sys.stderr)
        return 2
    sys.path.insert(0, str(DRIVER_DIR))
    import arbosdriver  # noqa: E402

    if OUT.exists():
        shutil.rmtree(OUT)
    OUT.mkdir(parents=True)
    place = OUT / "place"
    place.mkdir()
    mic_dir = OUT / "mic"
    mic_dir.mkdir()
    script = OUT / "asr-script.txt"
    script.write_text(SENTENCE + "\n")
    checks: list[tuple[bool, str]] = []

    def check(ok: bool, what: str) -> None:
        checks.append((bool(ok), what))
        print(f"   {'ok  ' if ok else 'FAIL'} {what}", flush=True)

    display = f":{9100 + os.getpid() % 800}"
    xvfb = subprocess.Popen(["Xvfb", display, "-screen", "0", "1500x950x24", "-nolisten", "tcp"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    duplex, kernel = MockDuplex(), MockKernel(place)
    gateway = app = None
    try:
        duplex_url = await duplex.start()
        kernel_url = await kernel.start()
        os.environ["VOICE_MOCK_ASR_SCRIPT"] = str(script)
        gateway = Gateway(duplex_url=duplex_url, kernel_url=kernel_url, log=OUT / "gateway.log", extra=["--asr", "mock", "--end-silence-ms", "500"])
        await gateway.start()
        xdg = OUT / "xdg"
        (xdg / "arbos").mkdir(parents=True)
        (xdg / "arbos" / "config.toml").write_text(f'voice_url = "{gateway.url}"\nvoice_token = "{TOKEN}"\nvoice_mirror = false\n')
        env = {
            "DISPLAY": display,
            "ARBOS_VOICE_MIC_CMD": f"{sys.executable} -m tests.mic {mic_dir}",
            "ARBOS_VOICE_PLAYER_CMD": f"cat >> {OUT / 'speaker.raw'}",
            "PYTHONPATH": str(ROOT),
            "PATH": f"{Path(binary).parent}:{os.environ.get('PATH', '')}",
        }
        app = await asyncio.to_thread(arbosdriver.Arbos.launch, binary=binary, env=env, log=OUT / "app.log", xdg=xdg, projects=[str(place)], timeout=120)
        state = await asyncio.to_thread(app.wait_state, lambda s: len(s["projects"]) >= 2 and s.get("active_session") is not None, 60, 0.5, "both tabs")
        ix = next(p["index"] for p in state["projects"] if Path(p["path"]).resolve() == place.resolve())
        if not state["projects"][ix]["active"]:
            await asyncio.to_thread(app.click, f"tab-{ix}")
            await asyncio.to_thread(app.wait_state, lambda s: s["projects"][ix]["active"], 10, 0.2, "tab")

        # Press the mic: the app opens a dictation session on the gateway.
        await asyncio.to_thread(app.click, "composer-voice")
        state = await asyncio.to_thread(app.wait_state, lambda s: s["composer"]["recording"], 20, 0.2, "recording")
        check(state["composer"]["recording"], "the mic button opens a take (composer.recording)")
        glog = lambda: (OUT / "gateway.log").read_text(errors="replace")  # noqa: E731
        await asyncio.to_thread(app.wait_for, glog, lambda t: "connected (pipeline)" in t, 10, 0.2, "a dictation session on the ASR pipeline")
        check("connected (pipeline)" in glog(), "the gateway opened the dictation session on the ASR pipeline (not the speech model)")

        pcm = speech.utterance(SENTENCE, prefer="synthetic")
        t0 = time.monotonic()
        (mic_dir / f"{time.time_ns()}.raw").write_bytes(pcm)
        first_partial = None

        def has_words(s: dict) -> bool:
            nonlocal first_partial
            text = (s["composer"].get("preview") or "").strip()
            if text and first_partial is None:
                first_partial = time.monotonic() - t0
            return bool(text)

        try:
            await asyncio.to_thread(app.wait_state, has_words, speech.seconds(pcm) + 5, 0.1, "partial words in the composer")
            check(True, f"partial words reached the composer while talking (first after {first_partial:.2f} s of a {speech.seconds(pcm):.1f} s utterance)")
        except Exception as exc:
            check(False, f"partial words reached the composer: {exc}")
        await asyncio.sleep(max(0.0, speech.seconds(pcm) - (time.monotonic() - t0)) + 0.9)
        # Release: the final replaces the partials and is sent as the prompt (the app's
        # push-to-talk shape: the take goes to the agent on release).
        t_release = time.monotonic()
        await asyncio.to_thread(app.click, "composer-voice")

        def sent(s: dict) -> bool:
            chat = next((c for p in s["projects"] for c in p["sessions"] if c["id"] == s.get("active_session")), None)
            return bool(chat) and any(i.get("kind") == "user" for i in chat["items"])

        state = await asyncio.to_thread(app.wait_state, sent, 15, 0.1, "the take sent as a prompt")
        final_at = time.monotonic() - t_release
        chat = next(c for p in state["projects"] for c in p["sessions"] if c["id"] == state["active_session"])
        final = next(i["text"] for i in chat["items"] if i.get("kind") == "user").strip()
        rate = wer(SENTENCE, final)
        check(rate == 0.0, f"the final text is the sentence, sent as the prompt {final_at:.2f} s after release (WER {rate:.0%}): {final!r}")
        check(not state["composer"]["recording"], "the take closed on release")
        subprocess.run(["ffmpeg", "-y", "-loglevel", "error", "-f", "x11grab", "-video_size", "1500x950", "-i", display, "-frames:v", "1", str(OUT / "dictated.png")], check=False, timeout=20)
        print(f"   measured: first partial {first_partial if first_partial is not None else float('nan'):.2f} s after the utterance began; WER {rate:.0%} (mock recogniser: timing of the pipeline, not of a model)")
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
    print(f"\ndesktop-fn-dictation: {'PASS' if ok else 'FAIL'}  {sum(1 for o, _ in checks if o)}/{len(checks)} checks; artifacts in {OUT}")
    return 0 if ok else 1


def main() -> None:
    raise SystemExit(asyncio.run(main_async()))


if __name__ == "__main__":
    main()
