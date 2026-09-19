#!/usr/bin/env python3
"""ui.py <udid> find|tap|value|dump <label…>

Drive and read the simulator by accessibility label, not by pixels.

Why this exists. The loop's scenarios located things by measuring a
screenshot. That went wrong three ways in one week, and every time the run
carried on and reported about whatever it had actually touched:

  * `find_row.py` knew four project names by glyph colour and raised on any
    other, and callers fell back to a default row — so cycle 49 twice opened
    `pod` while believing it had opened a fixture project;
  * it divided by 3 for a screenshot that is 1.2x the point size on this
    device, so even a name it knew landed on the wrong row;
  * coordinates read off a still are pixels and `idb ui tap` wants points
    (cycle 46, two wrong projects).

`idb ui describe-all` gives every element's label and its frame already in
points. Nothing here measures an image.

  find   print "x y" — the centre, in points — of the first match
  tap    tap that centre
  value  print the element's AXValue (the composer's placeholder or text)
  field  print what the text field holds, found by being a text field
         `field plain` undoes iOS's typographic substitutions first
  focus  tap that same text field
  menu   tap the overflow button in the top bar, which carries no label
  dump   print every label and frame, for writing a new scenario

`field` and `focus` take no label because the composer has none once there
is text in it: the placeholder is the label and it goes the moment a
character lands, so anything that names it can neither read it back nor
tap it again. Its frame moves too — the box grows taller as the text wraps,
and the keyboard pushes it up the screen — so a remembered point is wrong
by the second line. Both work with the keyboard up.

Read the field back before sending. `idb ui text` returns before its
characters arrive, so a scenario that types and presses return at once
sends whatever had landed by then, and a second call weaves itself into the
first (M-162).

A label that matches nothing exits 1 and prints nothing, so a scenario
fails where it went wrong rather than touching something else.
"""
import glob
import json
import os
import re
import shutil
import subprocess
import sys


def idb():
    """The path to idb, found rather than assumed.

    This is called over ssh, and a non-login ssh shell carries a bare PATH
    with no Homebrew on it, so a plain "idb" raises FileNotFoundError and the
    traceback blames the tool rather than the PATH. Cycle 192 fixed this for
    the shell scripts and added a check for it; cycle 193 tripped over the
    same thing here, because that check reads *.sh and this is Python.
    """
    found = shutil.which("idb")
    if found:
        return found
    # idb is installed with pip, so it lands in the user bin for whichever
    # Python installed it — not in Homebrew. Globbed, because the version in
    # that path changes and every place it was written down went stale.
    for pattern in (os.path.expanduser("~/Library/Python/*/bin/idb"),
                    "/opt/homebrew/bin/idb", "/usr/local/bin/idb"):
        for candidate in sorted(glob.glob(pattern), reverse=True):
            if os.path.exists(candidate):
                return candidate
    sys.exit("ui: no idb on PATH, in ~/Library/Python/*/bin, or in "
             "/opt/homebrew/bin — install it, or add its directory to PATH")


def elements(udid):
    out = subprocess.run([idb(), "ui", "describe-all", "--udid", udid],
                         capture_output=True, text=True, timeout=60)
    if out.returncode != 0:
        sys.exit(f"ui: idb describe-all failed: {out.stderr.strip()[:200]}")
    try:
        return json.loads(out.stdout)
    except json.JSONDecodeError:
        sys.exit("ui: idb did not return JSON")


def centre(el):
    f = el.get("frame") or {}
    return round(f.get("x", 0) + f.get("width", 0) / 2), round(f.get("y", 0) + f.get("height", 0) / 2)


def on_screen(el):
    """Is this element somewhere a finger could reach?

    `describe-all` returns the whole scroll view, including messages far
    above the top of the screen. Tapping one of those taps nothing, or
    something else — cycle 55's dictation step "sent" its line by tapping a
    message at y=-573.
    """
    f = el.get("frame") or {}
    x, y = f.get("x", 0), f.get("y", 0)
    return y + f.get("height", 0) > 0 and y < 1000 and x + f.get("width", 0) > 0 and x < 500


def match(els, needle):
    """First on-screen element whose label or value matches `needle`.

    An exact label wins, so "demo" does not pick "qa-cycle-11-demo" when both
    are on screen. Failing that the needle must appear as a whole word:
    plain substring matching found "Up" inside "setup" in the body of a
    message and tapped that instead of the send button.
    """
    needle = needle.lower()
    here = [e for e in els if on_screen(e)]
    exact = [e for e in here if (e.get("AXLabel") or "").lower().split(",")[0].strip() == needle]
    if exact:
        return exact[0]
    word = re.compile(r"(?<!\w)" + re.escape(needle) + r"(?!\w)")
    for e in here:
        for field in ("AXLabel", "AXValue", "AXUniqueId"):
            if word.search((e.get(field) or "").lower()):
                return e
    return None


def main():
    if len(sys.argv) < 3:
        sys.exit(__doc__)
    udid, verb = sys.argv[1], sys.argv[2]
    els = elements(udid)

    if verb == "menu":
        # The top bar's pop-up buttons carry names now ("More" in a chat,
        # "Filter" in the list), so prefer the name and keep the positional
        # search only for builds older than that. Either way the menu's own
        # items are in the tree only once it is open, which is why this verb
        # exists instead of a plain tap by label.
        wanted = sys.argv[3] if len(sys.argv) > 3 else None
        bar = [e for e in els if (e.get("type") or "") == "PopUpButton"
               and (e.get("frame") or {}).get("y", 999) < 150]
        if wanted:
            named = [e for e in bar if (e.get("AXLabel") or "") == wanted]
            if not named:
                print(f"ui: no pop-up button named {wanted!r} in the top bar", file=sys.stderr)
                sys.exit(1)
            bar = named
        if len(bar) != 1:
            print(f"ui: want one pop-up button in the top bar, found {len(bar)}", file=sys.stderr)
            sys.exit(1)
        x, y = centre(bar[0])
        subprocess.run([idb(), "ui", "tap", str(x), str(y), "--udid", udid], check=True)
        print(f"opened the top-bar menu at {x},{y}")
        return

    if verb in ("field", "focus"):
        fields = [e for e in els if (e.get("type") or "") == "TextField"]
        if not fields:
            print("ui: no text field on screen — is the keyboard up?", file=sys.stderr)
            sys.exit(1)
        if len(fields) > 1:
            print(f"ui: {len(fields)} text fields on screen, want one", file=sys.stderr)
            sys.exit(1)
        if verb == "field":
            held = fields[0].get("AXValue") or ""
            # iOS rewrites punctuation as it types: 'seeded' comes back as
            # ‘seeded’, and -- as an em dash. The sentence is the same and
            # the length can even match, so a scenario comparing what it
            # typed against what the box holds reads a false mismatch,
            # clears a perfectly good line and retries for ever.
            if len(sys.argv) > 3 and sys.argv[3] == "plain":
                for fancy, plain in (("\u2018", "'"), ("\u2019", "'"),
                                     ("\u201c", '"'), ("\u201d", '"'),
                                     ("\u2014", "--"), ("\u2013", "-")):
                    held = held.replace(fancy, plain)
            print(held, end="")
            return
        # The caret lands where the tap lands. The composer grows into a
        # multi-line box, so its centre is in the middle of what is already
        # written and typing there weaves the new line into the old one —
        # which is what garbled six of cycle 48's eight typed lines. Tapping
        # inside the last line, past its end, puts the caret after
        # everything. For an empty field this is the same place as anywhere.
        f = fields[0].get("frame") or {}
        x = round(f.get("x", 0) + f.get("width", 0) - 12)
        y = round(f.get("y", 0) + f.get("height", 0) - 12)
        subprocess.run([idb(), "ui", "tap", str(x), str(y), "--udid", udid], check=True)
        print(f"caret at the end of the text field, {x},{y}")
        return

    if verb == "dump":
        for e in els:
            x, y = centre(e)
            label = e.get("AXLabel") or e.get("AXValue") or e.get("AXUniqueId") or e.get("type")
            print(f"{x:>4} {y:>4}  {e.get('type',''):<12} {label}")
        return

    if verb == "values":
        # `dump` prints the label *or* the value, so an element carrying both
        # shows only the label and the value cannot be read at all. The call
        # orb is the case that matters: cycle 84 put the phase in the
        # accessibility value so the screen would say whether it is listening,
        # thinking or speaking — and no check has ever been able to see it,
        # because the orb's label is "Call" and the `or` stops there.
        #
        # It is a separate verb rather than a wider `dump` because scenarios
        # match dump lines exactly. A text field would grow from "Message pod…"
        # to "Message pod… | what was typed", and every one of those greps
        # would quietly stop matching.
        for e in els:
            x, y = centre(e)
            label = e.get("AXLabel") or ""
            value = e.get("AXValue") or ""
            both = f"{label} | {value}" if label and value and label != value else (label or value)
            print(f"{x:>4} {y:>4}  {e.get('type',''):<12} {both}")
        return

    needle = " ".join(sys.argv[3:])
    if not needle:
        sys.exit(__doc__)
    el = match(els, needle)
    if el is None:
        print(f"ui: nothing matching {needle!r} on screen", file=sys.stderr)
        sys.exit(1)

    if verb == "value":
        print(el.get("AXValue") or "")
        return
    x, y = centre(el)
    if verb == "find":
        print(f"{x} {y}")
        return
    if verb == "tap":
        subprocess.run([idb(), "ui", "tap", str(x), str(y), "--udid", udid], check=True)
        print(f"tapped {el.get('AXLabel') or needle!r} at {x},{y}")
        return
    sys.exit(__doc__)


if __name__ == "__main__":
    main()
