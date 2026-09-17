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
import json
import subprocess
import sys


def elements(udid):
    out = subprocess.run(["idb", "ui", "describe-all", "--udid", udid],
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


def match(els, needle):
    """First element whose label or value contains `needle`, case-insensitively.

    Prefers an exact label match so that "demo" does not pick
    "qa-cycle-11-demo" when both are on screen.
    """
    needle = needle.lower()
    exact = [e for e in els if (e.get("AXLabel") or "").lower().split(",")[0].strip() == needle]
    if exact:
        return exact[0]
    for e in els:
        for field in ("AXLabel", "AXValue", "AXUniqueId"):
            if needle in (e.get(field) or "").lower():
                return e
    return None


def main():
    if len(sys.argv) < 3:
        sys.exit(__doc__)
    udid, verb = sys.argv[1], sys.argv[2]
    els = elements(udid)

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
        subprocess.run(["idb", "ui", "tap", str(x), str(y), "--udid", udid], check=True)
        print(f"caret at the end of the text field, {x},{y}")
        return

    if verb == "dump":
        for e in els:
            x, y = centre(e)
            label = e.get("AXLabel") or e.get("AXValue") or e.get("AXUniqueId") or e.get("type")
            print(f"{x:>4} {y:>4}  {e.get('type',''):<12} {label}")
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
        subprocess.run(["idb", "ui", "tap", str(x), str(y), "--udid", udid], check=True)
        print(f"tapped {el.get('AXLabel') or needle!r} at {x},{y}")
        return
    sys.exit(__doc__)


if __name__ == "__main__":
    main()
