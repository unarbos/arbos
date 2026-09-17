#!/usr/bin/env python3
"""Cursor | Arbos side by side: the same prompt into both, stills at the same
moments, pairs written to the store.

  sidebyside.py --cycle 1 --side cursor|arbos|pair --prompt p1 ...

Cursor is driven with xdotool (Agents window, local project /tmp/parity-proj,
dark theme, sidebar hidden). Arbos is driven through its driver socket.
Both windows are placed at the same 1440x900 rectangle so crops align.
"""
import os, argparse, importlib.util, json, os, shutil, subprocess, sys, time
from pathlib import Path
from PIL import Image, ImageChops

STORE = Path("/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983")
OUT = Path("/tmp/polish/sbs")
DISPLAY = os.environ.get("DISPLAY", ":1")
ENV = {**os.environ, "DISPLAY": DISPLAY}
W, H, X, Y = 1440, 900, 200, 120

PROMPTS = {
    "p1": "Reply with exactly: The quick brown fox.",
    "p2": "hello write bubble sort",
    "p3": "what are we doing right now?",
    "p4": ("Use parallel sub-agents: one reviews math_utils.py for edge cases, one writes docstrings for every function, "
           "one drafts a CHANGELOG.md. Then merge their results."),
    "p5": "Add a mul(a, b) function to math_utils.py and call it from main.py with mul(4, 5).",
    "p6": 'Run this exact shell command and show me its output as it arrives: `for i in 1 2 3 4 5 6; do echo "step $i"; sleep 2; done`',
    "p7": "Write tests/test_math_utils.py with a failing test asserting add(2, 2) == 5, run it, show the failure, then fix the test so it passes and run it again.",
    "p8": "Explain the difference between lists, tuples, sets and dicts in Python. Use headings for each, one comparison table, and finish with three bullet recommendations.",
    "p9": 'Run this exact shell command and show me its output as it arrives: `for i in 1 2 3 4 5 6; do echo "step $i"; sleep 2; done`',
}
PROMPTS.update({
    "p10": "Before doing anything, ask me one multiple-choice question with two options, alpha and beta, about which name to use for a new module. Wait for my answer.",
    "p11": "Make a plan, do not write code yet: how would you add a command-line interface to this project? List the steps.",
    "p12": "Rename the function `sub` to `subtract` everywhere in this project, keeping behaviour, and run main.py to check.",
    "p13": 'Run this exact shell command and show me its output as it arrives: `for i in 1 2 3 4 5 6; do echo "step $i"; sleep 2; done`',
    "p14": "",  # no prompt: restore the checkpoint before the previous prompt
    "p15": "Commit all current changes with a clear message. Do not push.",
    "p16": "Search the web for the current stable Python version and tell me the number with a source link.",
    "p17": "Open a terminal and run `python3 main.py`; show me the output.",
    "p19": "Think carefully, step by step, about the fastest sorting algorithm for 10 nearly-sorted integers, then answer in one line.",
    "p21": "Run `python3 does_not_exist.py` and tell me what happened.",
    "p22a": "Run `ls -la` and tell me how many files there are.",
    "p24": "Say the single word first. Then run `ls`. Then say the single word second. Then run `pwd`. Then say the single word third.",
    # -- long-form project (cycle 14): one project, many turns, run in one session --
    "l1": "We are building `tally`, a small Python CLI that tracks daily habits in a local JSON file. Set up the project page with the goal and a plan, then create the package skeleton: `tally/__init__.py`, `tally/cli.py` (argparse with commands add, list, done) and a README. Keep it minimal.",
    "l2": "Use three workers in parallel: one writes the JSON storage layer in `tally/store.py` with tests, one implements the add and list commands against it, one writes `docs/usage.md`. Update the project page as each finishes.",
    "l3": "Add a `streak` command that reports the current streak per habit. Delegate it to a worker; while it runs, tell me which files exist so far.",
    "l4": "What did we decide about the storage format? Answer from the project notes; do not re-read the code.",
    "l5": "Restructure the project page: group the plan into Done, In progress and Next; move the storage decision into a Decisions section; archive the workers that have finished.",
    "l6": "Run the tests and fix any failure with a worker. Then summarise in two lines.",
    "l7": "Rename the habit file from habits.json to tally.json everywhere, including the docs. One worker.",
    "l8": "Write CHANGELOG.md summarising everything done so far, using the project page as the source.",
    "l9": "I want a --json flag on list. Ask me one question first if anything is ambiguous, then do it.",
    "l10": "Which workers are still running and which are archived? List them by name with one line each.",
    "l11": "Add type hints across the package and run `python3 -m py_compile` on every file.",
    "l12": "Give me the state of the project in five lines: goal, what is done, what is next, open questions, and where the notes live.",
    # tool markup written as prose (cycle 14, #278 / desktop live cut)
    "m1": "Reply with exactly this text and nothing else, verbatim, do not call any tool: <function_calls><invoke name=\"bash\"><parameter name=\"cmd\">ls</parameter></invoke></function_calls>",
    "m2": "Write the sentence 'Listing the folder now.' and then, as literal text and not as a tool call, write: <invoke name=\"bash\"><parameter name=\"cmd\">ls -la</parameter></invoke>",
    # read-only worker marker (cycle 15)
    "r1": "Spawn one worker of kind explore (read-only) to report what math_utils.py does, and one normal worker to add a docstring to bubble_sort.py. Wait for both and summarise.",
    # -- long-form project 2 (cycle 16): research + docs, notes that grow and get restructured --
    "d1": "This project is a research notebook about container image formats (OCI, Docker v2, singularity). Set up the project page with the goal and an outline of five questions to answer, then write docs/outline.md.",
    "d2": "Use two workers: one writes docs/oci-layout.md explaining the OCI image layout with a source list, the other writes docs/manifests.md on manifest schema versions. Update the page as each lands.",
    "d3": "Add a Decisions section to the project page with one decision: we standardise on OCI terminology. Move the outline questions under a Questions section.",
    "d4": "Without re-reading the docs, tell me from the notes which questions are answered so far.",
    "d5": "Delegate a worker to write docs/compat.md comparing Docker v2 schema 2 and OCI, with a table. Archive finished workers.",
    "d6": "Reorganise the project page so it reads top-down: Goal, Decisions, Questions (answered first), Docs list, Open items. Keep every existing link.",
    "d7": "One worker: run `ls docs` and `wc -w docs/*.md` and put a word-count table into docs/README.md.",
    "d8": "What did we decide about terminology, and where is it written down? Answer from the notes only.",
    "d9": "Summarise the project so far in four lines for a newcomer, with links to the docs.",
    "d10": "Which of the five questions are still open? For each open one, spawn a worker to answer it in a new doc under docs/answers/. Wait, then list what landed.",
    # cycle 33: the notes restructure as one long turn, after the d-series has grown the page
    "d12": "The project page has grown in pieces. Restructure notes.md yourself, no workers: sections Goal, Decisions, Done, In flight, Open questions, Next — keep every fact and every link, drop nothing, merge duplicates. Then tell me in the chat, as a short list, what moved where and what you merged.",
    # after a relaunch, on the same project
    "l13": "Continue: what were we doing? Pick up the next item from the plan and do it.",
    "p23": "In one sentence, what does main.py do? Do not run anything.",
    "p22b": "Run the test suite with `python3 -m pytest -q` and report the result.",
    "p20": "",  # no prompt: open the project page / files panel after the session
    "pp1": "Reply with exactly: The quick brown fox.",
    "pp2": "Use three sub-agents in parallel: one writes one sentence about Python lists, one about tuples, one about sets. Then combine their three sentences into your answer.",
    "pp5": "what is in this repo",
    "pp3": "First call the todo tool with op set and three items: greet, count to three, say goodbye. Then do the three steps yourself, one short line each, calling todo check n after each step. No sub-agents.",
    "pp4": "Spawn one sub-agent with the brief: 'Wait for my message and do exactly what it says, in one line.' Then use say with mode steer, title 'Count to three', and text 'Count to five: before each number run the shell command sleep 3, then say the number.' to that sub-agent. When its reply arrives, repeat it to me.",
})
# prompts that get a recording of the working state, and how long
RECORD = {"pp5": 20, "pp4": 20, "p6": 14, "p4": 14, "p9": 10, "p13": 14, "p10": 14, "p19": 14, "p16": 14, "pp2": 20}
# a follow-up typed while the turn runs: seconds after send, text
STEER = {"p13": (3.0, "Also print the date at the end.")}
# a typed answer to the question card: seconds after send, text
ANSWER = {"p10": (12.0, "alpha")}
# prompts cancelled mid-turn: seconds after send to press Stop
CANCEL = {"p9": 5}
STAGES = [("t2", 2.0), ("t8", 6.0), ("t20", 12.0)]
# long-form turns after which the Project page is captured too, and the chat scrolled to its top
PAGE_AFTER = {"l1", "l4", "l5", "l8", "l12", "l13", "d1", "d3", "d6", "d10", "d12"}
SCROLL_TOP = {"l12", "l13", "d10"}


def log(m):
    print(time.strftime("[%H:%M:%S] ") + m, file=sys.stderr, flush=True)


def run(*cmd, **kw):
    return subprocess.run(list(cmd), env=ENV, capture_output=True, text=True, **kw)


def find_window(pattern):
    out = run("wmctrl", "-l").stdout
    for line in out.splitlines():
        parts = line.split(None, 3)
        if len(parts) == 4 and pattern in parts[3]:
            return parts[0]
    return None


def place(wid):
    run("wmctrl", "-i", "-a", wid)
    for _ in range(8):
        run("wmctrl", "-i", "-r", wid, "-e", f"0,{X},{Y},{W},{H}")
        time.sleep(0.8)
        g = dict(l.split("=") for l in run("xdotool", "getwindowgeometry", "--shell", wid).stdout.strip().splitlines())
        if int(g["WIDTH"]) == W and int(g["HEIGHT"]) == H:
            return (int(g["X"]), int(g["Y"]), W, H)
    return (int(g["X"]), int(g["Y"]), int(g["WIDTH"]), int(g["HEIGHT"]))


def shot(path, geom):
    x, y, w, h = geom
    run("scrot", "-o", "-a", f"{x},{y},{w},{h}", str(path))
    return path


def cursor_to_bottom(geom):
    """Cursor keeps the scroll where the reader left it and shows an
    'N New Messages ↓' pill; the frames want the newest turn. Wheel down
    over the chat column."""
    x, y, w, h = geom
    run("xdotool", "mousemove", str(x + w // 2), str(y + h // 2))
    for _ in range(4):
        run("xdotool", "click", "--repeat", "15", "--delay", "10", "5")
        time.sleep(0.15)


def record(path, geom, seconds):
    x, y, w, h = geom
    return subprocess.Popen(["ffmpeg", "-loglevel", "error", "-y", "-f", "x11grab", "-framerate", "15", "-video_size", f"{w}x{h}",
                             "-i", f"{DISPLAY}+{x},{y}", "-t", str(seconds), "-c:v", "libx264", "-preset", "veryfast", "-pix_fmt", "yuv420p", str(path)],
                            env=ENV, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def stable(geom, take, seconds=6.0, every=1.0):
    """Wait until the window stops changing for `seconds` (or 150 s pass)."""
    t0 = time.time(); last = None; still_since = None
    while time.time() - t0 < float(os.environ.get("SBS_STABLE_MAX", "150")):
        p = OUT / "_probe.png"; shot(p, geom)
        im = Image.open(p).convert("L")
        # the bottom band holds Cursor's always-turning spinner and the
        # composer caret; stillness is judged on the transcript above it
        im = im.crop((0, 0, im.size[0], max(1, im.size[1] - 90)))
        if last is not None:
            diff = ImageChops.difference(im, last).getbbox()
            if diff is None:
                if still_since is None: still_since = time.time()
                if time.time() - still_since >= seconds:
                    return True
            else:
                still_since = None
        last = im
        time.sleep(every)
    return False


# -- Cursor --------------------------------------------------------------------

def cursor_send(geom, text):
    x, y, w, h = geom
    # the composer sits at the bottom of the chat column once a chat exists,
    # and mid-window on the empty screen; click both spots' text area.
    run("xdotool", "mousemove", str(x + w // 2), str(y + h - 70), "click", "1")
    time.sleep(0.4)
    subprocess.run(["xdotool", "type", "--delay", "12", "--file", "-"], input=text, text=True, env=ENV)
    time.sleep(0.3)
    run("xdotool", "key", "Return")


def cursor_new_chat(geom):
    run("xdotool", "key", "ctrl+n"); time.sleep(2.0)
    x, y, w, h = geom
    # empty screen: the composer is centred
    run("xdotool", "mousemove", str(x + w // 2), str(y + h // 2 - 40), "click", "1")
    time.sleep(0.4)


def side_cursor(ids, tag):
    wid = find_window("Cursor Agents")
    if not wid:
        raise SystemExit("no Cursor Agents window")
    geom = place(wid)
    for n, pid in enumerate(ids):
        text = PROMPTS[pid]
        if n == 0 and not os.environ.get("SBS_CURSOR_FOLLOWUP"):
            # type into whatever composer is showing (a fresh chat was set up by hand)
            x, y, w, h = geom
            run("xdotool", "mousemove", str(x + w // 2), str(y + h // 2 - 40), "click", "1"); time.sleep(0.3)
            subprocess.run(["xdotool", "type", "--delay", "12", "--file", "-"], input=text, text=True, env=ENV)
            time.sleep(0.3); run("xdotool", "key", "Return")
        else:
            cursor_send(geom, text)
        if pid == "p20":
            x, y, w, h = geom
            # Cursor's right panel: "Files" row under "On parity-proj"
            run("xdotool", "mousemove", str(x + 1220), str(y + 198), "click", "1"); time.sleep(2.0)
            cursor_to_bottom(geom); shot(OUT / f"{tag}-cursor-{pid}-end.png", geom); log("cursor p20 files panel")
            continue
        if pid == "p14":
            # Cursor: the restore icon sits at the right end of the last prompt card; hover it first
            x, y, w, h = geom
            run("xdotool", "mousemove", str(x + 600), str(y + 95)); time.sleep(0.8)
            cursor_to_bottom(geom); shot(OUT / f"{tag}-cursor-{pid}-t2.png", geom)
            run("xdotool", "mousemove", str(x + 946), str(y + 98), "click", "1"); time.sleep(2.5)
            cursor_to_bottom(geom); shot(OUT / f"{tag}-cursor-{pid}-t8.png", geom); log("cursor p14 restore clicked")
            time.sleep(4); shot(OUT / f"{tag}-cursor-{pid}-end.png", geom)
            continue
        t0 = time.time()
        rec = record(OUT / f"{tag}-cursor-{pid}-working.mp4", geom, RECORD[pid]) if pid in RECORD else None
        extra_done = False
        for name, at in STAGES:
            while time.time() - t0 < at:
                time.sleep(0.1)
                if not extra_done and pid in STEER and time.time() - t0 >= STEER[pid][0]:
                    cursor_send(geom, STEER[pid][1]); extra_done = True; log(f"cursor {pid} steer sent")
                if not extra_done and pid in ANSWER and time.time() - t0 >= ANSWER[pid][0]:
                    cursor_to_bottom(geom); shot(OUT / f"{tag}-cursor-{pid}-card.png", geom)
                    cursor_send(geom, ANSWER[pid][1]); extra_done = True; log(f"cursor {pid} answered")
            if pid in CANCEL and at >= CANCEL[pid] and not getattr(rec, "_cancelled", False):
                # Cursor's stop square sits where the mic was, bottom right of the composer
                x, y, w, h = geom
                run("xdotool", "mousemove", str(x + 948), str(y + 841), "click", "1")
                log(f"cursor {pid} stop pressed")
                if rec is not None: rec._cancelled = True
                else: CANCEL[pid] = 10_000
            cursor_to_bottom(geom); shot(OUT / f"{tag}-cursor-{pid}-{name}.png", geom); log(f"cursor {pid} {name}")
        if rec is not None: rec.wait()
        stable(geom, pid, seconds=float(os.environ.get("SBS_STILL", "6")))
        cursor_to_bottom(geom); shot(OUT / f"{tag}-cursor-{pid}-end.png", geom); log(f"cursor {pid} end")
        if pid in PAGE_AFTER:
            x, y, w, h = geom
            run("xdotool", "mousemove", str(x + 1220), str(y + 198), "click", "1"); time.sleep(2.0)
            shot(OUT / f"{tag}-cursor-{pid}-page.png", geom); log(f"cursor {pid} project page")
            run("xdotool", "key", "Escape"); time.sleep(1.0)
        if pid in SCROLL_TOP:
            x, y, w, h = geom
            run("xdotool", "mousemove", str(x + w // 2), str(y + h // 2))
            for _ in range(40): run("xdotool", "click", "4")
            time.sleep(0.8); shot(OUT / f"{tag}-cursor-{pid}-top.png", geom); log(f"cursor {pid} scrolled to top")
        time.sleep(1.5)


# -- Arbos ---------------------------------------------------------------------

def side_arbos(ids, tag, binary, kernel):
    spec = importlib.util.spec_from_file_location("arbosdriver", str(Path(__file__).resolve().parent.parent.parent / "driver" / "arbosdriver.py"))
    drv = importlib.util.module_from_spec(spec); spec.loader.exec_module(drv)
    xdg = Path("/tmp/polish-xdg-sbs")
    if not os.environ.get("SBS_KEEP"):
        shutil.rmtree(xdg, ignore_errors=True)
    (xdg / "arbos").mkdir(parents=True, exist_ok=True)
    model = os.environ.get("SBS_MODEL", "openai/gpt-5.4-mini")
    effort = os.environ.get("SBS_EFFORT", "")
    (xdg / "arbos" / "config.toml").write_text(f'model = "{model}"\napi_base = "https://openrouter.ai/api/v1"\napi_key_env = "OPENROUTER_API_KEY"\n' + (f'reasoning_effort = "{effort}"\n' if effort else ""))
    where = os.environ.get("SBS_PLACE", "/tmp/parity-proj")
    subprocess.run(["pkill", "-f", f"arbos-kernel serve {where}$"]); time.sleep(1)
    if os.environ.get("SBS_COLD"):
        # The cold track: a fresh place, no store, no history; a repo when asked.
        shutil.rmtree(where, ignore_errors=True); Path(where).mkdir(parents=True)
        if os.environ.get("SBS_COLD") == "repo":
            subprocess.run(["git", "init", "-q", where]); (Path(where) / "main.py").write_text("print('hello')\n")
            subprocess.run(["git", "-C", where, "add", "-A"]); subprocess.run(["git", "-C", where, "-c", "user.email=qa@arbos", "-c", "user.name=qa", "commit", "-q", "-m", "init"])
    if not os.environ.get("SBS_KEEP"):
        shutil.rmtree(f"{where}/.arbos", ignore_errors=True)
        drv.seed_state(xdg, [where])
    app = drv.Arbos.launch(binary=binary, env={"ARBOS_KERNEL_BIN": kernel, "DISPLAY": DISPLAY, "XDG_CONFIG_HOME": str(xdg), "XDG_DATA_HOME": str(xdg / "data")},
                           log=str(OUT / f"{tag}-arbos-app.log"), timeout=90)
    time.sleep(2.5)
    wid = find_window("Arbos")
    geom = place(wid)
    st = app.state()
    if st.get("permissions", {}).get("open"): app.key("escape"); time.sleep(0.8)
    if app.exists("tab-sheet-done"): app.key("escape"); time.sleep(0.8)
    for p in st["projects"]:
        if p["path"].rstrip("/") == where: app.click(f"tab-{p['index']}"); break
    time.sleep(1.0)
    def busy(s): return any(c["streaming"] or c["turn_open"] for pr in s["projects"] for c in pr["sessions"])
    for pid in ids:
        if pid in PAGE_AFTER:
            pass
        if pid == "p20":
            if app.exists("panel-project-head"):
                app.click("panel-project-head"); time.sleep(1.5)
            shot(OUT / f"{tag}-arbos-{pid}-end.png", geom); log("arbos p20 project page")
            rows = [e["path"] for e in app.snapshot()["elements"] if "panel-agent-" in e["path"]]
            if rows: app.click(rows[0]); time.sleep(0.8)
            continue
        if pid == "p14":
            # Arbos: the checkpoint restore shows on the last answer's footer while the pointer is over it
            els = [e["path"] for e in app.snapshot()["elements"] if "rewind-turn-" in e["path"]]
            if els:
                el = app.find(els[-1]); app.hover(els[-1]); time.sleep(0.8)
                shot(OUT / f"{tag}-arbos-{pid}-t2.png", geom)
                app.click(els[-1]); time.sleep(2.5); log("arbos p14 rewind clicked")
            shot(OUT / f"{tag}-arbos-{pid}-t8.png", geom); time.sleep(4); shot(OUT / f"{tag}-arbos-{pid}-end.png", geom)
            continue
        app.click("composer-field")
        # a rewind leaves the old prompt in the composer: clear it first
        if app.state()["composer"]["text"]:
            for chord in ("cmd-a", "ctrl-a"):
                app.key(chord); app.key("backspace")
                if not app.state()["composer"]["text"]: break
        app.type(PROMPTS[pid] + "\n")
        t0 = time.time()
        rec = record(OUT / f"{tag}-arbos-{pid}-working.mp4", geom, RECORD[pid]) if pid in RECORD else None
        cancelled = False
        extra_done = False
        for name, at in STAGES:
            while time.time() - t0 < at:
                time.sleep(0.1)
                if not extra_done and pid in STEER and time.time() - t0 >= STEER[pid][0]:
                    app.click("composer-field"); app.type(STEER[pid][1] + "\n"); extra_done = True; log(f"arbos {pid} steer sent")
                if not extra_done and pid in ANSWER and time.time() - t0 >= ANSWER[pid][0]:
                    shot(OUT / f"{tag}-arbos-{pid}-card.png", geom)
                    app.click("composer-field"); app.type(ANSWER[pid][1] + "\n"); extra_done = True; log(f"arbos {pid} answered")
            if pid in CANCEL and at >= CANCEL[pid] and not cancelled:
                if app.exists("composer-stop"): app.click("composer-stop"); log(f"arbos {pid} stop pressed")
                cancelled = True
            shot(OUT / f"{tag}-arbos-{pid}-{name}.png", geom); log(f"arbos {pid} {name}")
        if rec is not None: rec.wait()
        t1 = time.time()
        while busy(app.state()) and time.time() - t1 < 150: time.sleep(0.5)
        time.sleep(2.0)
        shot(OUT / f"{tag}-arbos-{pid}-end.png", geom); log(f"arbos {pid} end")
        if pid in PAGE_AFTER and app.exists("panel-project-head"):
            app.click("panel-project-head"); time.sleep(1.5)
            shot(OUT / f"{tag}-arbos-{pid}-page.png", geom); log(f"arbos {pid} project page")
            app.key("escape"); time.sleep(0.8)
        if pid in SCROLL_TOP:
            app.click("composer-field")
            for _ in range(30): app.scroll("composer-field", dy=800)
            time.sleep(0.8); shot(OUT / f"{tag}-arbos-{pid}-top.png", geom); log(f"arbos {pid} scrolled to top")
    if not os.environ.get("SBS_KEEP_APP"):
        app.close()


# -- pairs ---------------------------------------------------------------------

def pairs(ids, tag, cycle):
    dest = STORE / "media" / "cursor-reference" / f"cycle-{cycle}"
    dest.mkdir(parents=True, exist_ok=True)
    written = []
    for pid in ids:
        for name in [s[0] for s in STAGES] + ["card", "end"]:
            c = OUT / f"{tag}-cursor-{pid}-{name}.png"; a = OUT / f"{tag}-arbos-{pid}-{name}.png"
            if not (c.exists() and a.exists()): continue
            ci, ai = Image.open(c).convert("RGB"), Image.open(a).convert("RGB")
            h = max(ci.height, ai.height)
            pair = Image.new("RGB", (ci.width + ai.width + 24, h + 28), (40, 40, 40))
            pair.paste(ci, (0, 28)); pair.paste(ai, (ci.width + 24, 28))
            out = dest / f"{tag}-{pid}-{name}-pair.png"; pair.save(out)
            shutil.copyfile(c, dest / f"{tag}-{pid}-{name}-cursor.png"); shutil.copyfile(a, dest / f"{tag}-{pid}-{name}-arbos.png")
            written.append(str(out.relative_to(STORE)))
        for side in ("cursor", "arbos"):
            clip = OUT / f"{tag}-{side}-{pid}-working.mp4"
            if clip.exists():
                shutil.copyfile(clip, dest / clip.name); written.append(str((dest / clip.name).relative_to(STORE)))
    print("\n".join(written))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--cycle", type=int, required=True)
    ap.add_argument("--side", required=True, choices=["cursor", "arbos", "pair"])
    ap.add_argument("--tag", default="before")
    ap.add_argument("--prompts", default="p1,p2,p3")
    ap.add_argument("--binary", default="/workspace/desktop/target/debug/arbos-desktop")
    ap.add_argument("--kernel", default="/workspace/target/debug/arbos-kernel")
    a = ap.parse_args()
    OUT.mkdir(parents=True, exist_ok=True)
    ids = a.prompts.split(",")
    if a.side == "cursor": side_cursor(ids, a.tag)
    elif a.side == "arbos": side_arbos(ids, a.tag, a.binary, a.kernel)
    else: pairs(ids, a.tag, a.cycle)


if __name__ == "__main__":
    main()
