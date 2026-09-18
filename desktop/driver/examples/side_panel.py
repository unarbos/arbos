"""Check the side panel's shape: default closed, its own tabs, its own chords.

Every fact is waited for on the state the assertion reads, never slept for, and
each check says what it proves rather than which field it looked at.

Run from the repo root::

    source .venv/bin/activate
    xvfb-run -s "-screen 0 1728x1080x24" python desktop/driver/examples/side_panel.py

The chords are spelled the way the window binds them: `cmd-shift-[` and
`cmd-shift-]` are the physical `⌘⇧{` and `⌘⇧}`.
"""

from __future__ import annotations

import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from arbosdriver import Arbos  # noqa: E402

PASSED: list[str] = []
FAILED: list[str] = []


def check(what: str, ok: bool, detail: str = "") -> None:
    if ok:
        PASSED.append(what)
        print(f"  ok   {what}")
    else:
        FAILED.append(f"{what} — {detail}")
        print(f"  FAIL {what} — {detail}")


def kinds(state: dict) -> list[str]:
    return [tab["kind"] for tab in state["panel"]["tabs"]]


def seed_place(root: Path) -> Path:
    """A folder with a store the panel has something to list: a page, a
    context document, and one more file to click. Written here rather than
    borrowed from the machine, so the check reads the same everywhere."""
    place = root / "place"
    docs = place / ".arbos" / "docs"
    docs.mkdir(parents=True, exist_ok=True)
    (place / ".arbos" / "notes.md").write_text("# Place\n\n- [ ] nothing yet\n")
    (docs / "project-context.md").write_text("# Context\n\nA folder for the check.\n")
    (docs / "note.md").write_text("# Note\n\nOne file to open in the drawer.\n")
    return place


def quiet_launch(xdg: Path, place: Path) -> None:
    """Take the first-launch sheet out of the way before the app starts.

    It opens about 600 ms in and takes the focus, so a keystroke sent while it
    is arriving lands in a window whose focus is about to move — which is a
    race in the check, not a fault in the app. Marking it seen up front is the
    fix at the source; every other value in the file is left to the app's own
    defaults.
    """
    folder = xdg / "arbos-desktop"
    folder.mkdir(parents=True, exist_ok=True)
    state = folder / "state.toml"
    if not state.exists():
        state.write_text(
            "permissions_seen = true\n"
            'appearance = "dark"\n'
            f'projects = ["{place}"]\n'
            "active = 0\n"
        )


def main() -> int:
    home = Path(tempfile.mkdtemp(prefix="side-panel-"))
    log = home / "app.log"
    # `xdg=` would seed a fresh `state.toml` on every launch, and what this
    # checks is the file the first launch wrote. So the environment is set
    # directly and the second launch reads what the first one left.
    xdg = home / "xdg"
    env = {"XDG_CONFIG_HOME": str(xdg), "XDG_DATA_HOME": str(xdg / "data")}
    place = seed_place(home)
    quiet_launch(xdg, place)
    with Arbos.launch(log=log, env=env) as app:
        state = app.wait_state(
            lambda s: s.get("panel") is not None,
            what="a project whose panel the window can read",
        )
        check("the drawer starts closed", state["panel"]["open"] is False)
        check("holding the project tab alone", kinds(state) == ["project"])
        check(
            "and `panel_open` still answers under its old name",
            state["panel_open"] is False,
        )

        app.key("cmd-b")
        state = app.wait_state(lambda s: s["panel"]["open"], what="the drawer open")
        check(
            "opening it gives it the focus, so the lit tab row is its row",
            state["panel"]["focused"] is True,
        )
        check(
            "the Project tab opens as wide as a Terminal tab",
            state["panel"]["width"] == 592,
            f"width={state['panel']['width']}",
        )

        projects = len(state["projects"])
        app.key("cmd-t")
        state = app.wait_state(
            lambda s: len(s["panel"]["tabs"]) == 2, what="a second panel tab"
        )
        check("⌘T in the panel opens a panel tab", kinds(state) == ["project", "new"])
        check(
            "and no project tab, and no folder picker",
            len(state["projects"]) == projects and state["opener_open"] is False,
            f"projects={len(state['projects'])} opener={state['opener_open']}",
        )
        check("the new tab is the one in front", state["panel"]["active"] == 1)

        app.key("cmd-shift-[")
        state = app.wait_state(
            lambda s: s["panel"]["active"] == 0, what="the step back"
        )
        check("⌘⇧{ steps the panel's own tabs", True)
        app.key("cmd-shift-]")
        state = app.wait_state(
            lambda s: s["panel"]["active"] == 1, what="the step on"
        )
        check("⌘⇧} steps them the other way", True)

        app.key("cmd-1")
        state = app.wait_state(
            lambda s: s["panel"]["focused"] is False, what="the focus leaving the panel"
        )
        check("⌘1 takes the focus off the panel", True)
        check("and leaves the drawer open", state["panel"]["open"] is True)

        tabs = len(state["panel"]["tabs"])
        app.key("cmd-t")
        state = app.wait_state(
            lambda s: s["opener_open"] or len(s["panel"]["tabs"]) != tabs,
            what="⌘T with the focus outside the panel",
        )
        check(
            "the same chord is a project tab again once the focus has left",
            state["opener_open"] is True and len(state["panel"]["tabs"]) == tabs,
            f"opener={state['opener_open']} tabs={len(state['panel']['tabs'])}",
        )
        app.key("escape")
        app.wait_state(lambda s: s["opener_open"] is False, what="the picker dismissed")

        # A click on a store file is the person's own route into a tab: it
        # opens one, fills it, and brings it to the front.
        app.click("panel-tab-0")
        app.wait_state(lambda s: s["panel"]["active"] == 0, what="the project tab")
        # `ids()` gives full dotted paths; a row is named by its tail.
        if any(name.endswith("panel-file-0") for name in app.ids()):
            app.click("panel-file-0")
            state = app.wait_state(
                lambda s: any(t["kind"] == "surface" for t in s["panel"]["tabs"]),
                what="a surface tab from the click",
            )
            front = state["panel"]["tabs"][state["panel"]["active"]]
            check(
                "a file the person opens fills a tab and comes to the front",
                front["kind"] == "surface" and bool(front["title"]),
                f"front={front}",
            )
            # ⌘\ grows the drawer. It must not clone the tab into the chat
            # column — that was the four-box bug.
            before = state["panel"]["width"]
            app.key("cmd-\\")
            state = app.wait_state(
                lambda s: s["panel"]["width"] != before, what="the drawer grown"
            )
            check(
                "⌘\\ expands the drawer and leaves the chat in the column",
                state["showing"] == "chat" and state["panel"]["width"] > before,
                f"showing={state['showing']} width={state['panel']['width']}",
            )
            app.key("cmd-\\")
            state = app.wait_state(
                lambda s: s["panel"]["width"] == before, what="the drawer restored"
            )
            check("and the same key gives the default width back", True)

            at = state["panel"]["active"]
            app.click(f"panel-tab-close-{at}")
            state = app.wait_state(
                lambda s: all(t["kind"] != "surface" for t in s["panel"]["tabs"]),
                what="the tab closed",
            )
            check("closing its tab takes the row with it", True)
        else:
            check(
                "a file the person opens fills a tab",
                False,
                "no store file row to click in this place",
            )

        # The drawer belongs to the project, so the window's tabs switch it.
        # The focus is on the chat here, which is what makes the same chord
        # move the project strip.
        app.key("cmd-1")
        app.wait_state(lambda s: s["panel"]["focused"] is False, what="the chat's focus")
        active = [p["name"] for p in app.state()["projects"] if p["active"]]
        app.key("cmd-shift-[")
        state = app.wait_state(
            lambda s: [p["name"] for p in s["projects"] if p["active"]] != active,
            what="the other project in front",
        )
        check(
            "the other project's drawer is its own, and was never opened",
            state["panel"]["open"] is False,
            f"panel={state['panel']}",
        )
        app.key("cmd-shift-]")
        state = app.wait_state(
            lambda s: [p["name"] for p in s["projects"] if p["active"]] == active,
            what="the first project back in front",
        )
        check("and coming back brings this one's back open", state["panel"]["open"] is True)

        app.key("cmd-b")
        app.wait_state(lambda s: s["panel"]["open"] is False, what="⌘B closing it")
        app.key("cmd-b")
        app.wait_state(lambda s: s["panel"]["open"], what="⌘B opening it again")
        app.key("escape")
        app.wait_state(lambda s: s["panel"]["open"] is False, what="Escape closing it")
        check("Escape closes the drawer", True)

        app.key("cmd-b")
        app.wait_state(lambda s: s["panel"]["open"], what="the drawer open to be filed")
        app.quit()

    filed = (xdg / "arbos-desktop" / "state.toml").read_text()
    check(
        "the drawer is filed under its place, open",
        "[panels." in filed and "open = true" in filed,
        "state.toml has no panels table",
    )

    # The same place again: the drawer comes back as it was left, and holds no
    # row the window cannot account for.
    with Arbos.launch(log=log, env=env) as app:
        state = app.wait_state(
            lambda s: s.get("panel") is not None, what="the panel after a relaunch"
        )
        check("a relaunch brings the drawer back open", state["panel"]["open"] is True)
        check(
            "and no tab claims to be something the kernel has no record of",
            all(
                tab["kind"] != "surface" or tab["state"] != "gone"
                for tab in state["panel"]["tabs"]
            ),
            f"tabs={state['panel']['tabs']}",
        )
        app.quit()

    print()
    print(f"{len(PASSED)} passed, {len(FAILED)} failed")
    for line in FAILED:
        print(f"  {line}")
    return 1 if FAILED else 0


if __name__ == "__main__":
    sys.exit(main())
