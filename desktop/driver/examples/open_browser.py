"""Open Arbos, start a new chat, ask it to open a browser, and check it did.

Run from the repo root::

    source .venv/bin/activate
    python desktop/driver/examples/open_browser.py

Exit code 0 means every check passed. The screenshot is left in /tmp so a
person can look at it too.
"""

from __future__ import annotations

import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from arbosdriver import Arbos, Png, element_box_in_png  # noqa: E402

SHOTS = Path("/tmp/arbos-driver")


def main() -> int:
    SHOTS.mkdir(exist_ok=True)
    stamp = time.strftime("%H%M%S")
    log = SHOTS / f"open-browser-{stamp}.log"

    with Arbos.launch(log=log) as app:
        hello = app.hello()
        scale = hello["window"]["scale"]
        print(f"app pid {hello['pid']} window {hello['window']['width']}x{hello['window']['height']} @{scale}x")

        state = app.state()
        if not state["projects"]:
            print("FAIL: no project is open; open one in the app first so a chat has somewhere to run")
            return 2
        ix = state["active_project"] or 0

        # 1. New chat. The "+" on a project heading mounts only while the
        #    heading is hovered, so hover first, then click it.
        before = {c["id"] for c in app.sessions()}
        app.hover(f"project-{ix}")
        app.wait_element(f"project-add-{ix}", reachable=True)
        app.click(f"project-add-{ix}")
        new = app.wait_state(
            lambda s: {c["id"] for p in s["projects"] for c in p["sessions"]} - before,
            timeout=15,
            what="a new session",
        )
        session_id = (({c["id"] for p in new["projects"] for c in p["sessions"]}) - before).pop()
        print(f"new session {session_id}")

        # 2. Click the text box and type the request; "\n" is Enter, which sends.
        app.wait_element("composer-field", reachable=True)
        app.click("composer-field")
        app.type("open browser")
        typed = app.state()["composer"]["text"]
        assert typed == "open browser", f"composer holds {typed!r}"
        app.screenshot(SHOTS / f"open-browser-{stamp}-typed.png")
        app.type("\n")

        # 3. Wait for the agent to open a browser surface.
        try:
            app.wait_state(
                lambda s: any(f["kind"] == "browser" for p in s["projects"] for f in p["surfaces"]),
                timeout=180,
                every=0.5,
                what="a browser surface",
            )
        except TimeoutError as err:
            shot = app.screenshot(SHOTS / f"open-browser-{stamp}-timeout.png")
            chat = app.active_session() or {}
            tail = [f"{i['kind']}: {i.get('text') or i.get('label')}" for i in chat.get("items", [])[-4:]]
            print(f"FAIL: {err}\n  transcript tail: {tail}\n  screenshot: {shot}")
            return 1

        # 4. Screenshot, and check what is on screen.
        shot = app.screenshot(SHOTS / f"open-browser-{stamp}-browser.png")
        snap = app.snapshot()
        showing = snap["state"]["showing"]
        panel = next((el for el in snap["elements"] if el["id"] in ("surface-browser-shot", "surface-browser-open")), None)
        print(f"showing pane: {showing}; browser panel element: {panel and panel['path']}")
        print(f"screenshot: {shot}")

        failures = []
        if showing != "surface":
            failures.append(f"detail column shows {showing!r}, not the surface pane")
        if panel is None:
            failures.append("no surface-browser-* element on screen")
        elif not panel["visible"]:
            failures.append("browser panel element is clipped out of view")
        else:
            # The panel's box in the PNG should not be one flat colour.
            png = Png(shot)
            x, y, w, h = element_box_in_png(panel, scale)
            spread = png.spread(x, y, w, h)
            print(f"browser panel pixel spread: {spread:.1f}")
            if spread < 2.0:
                failures.append(f"browser panel region is flat (spread {spread:.1f}); nothing drawn?")

        if failures:
            print("FAIL:\n  " + "\n  ".join(failures))
            return 1
        print("PASS: the browser panel opened and is on screen")
        return 0


if __name__ == "__main__":
    sys.exit(main())
