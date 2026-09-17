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
  dump   print every label and frame, for writing a new scenario

`field` takes no label because the composer has none once there is text in
it: the placeholder is the label and it goes the moment a character lands,
so anything that names it cannot read it back. `idb ui text` returns before
its characters arrive, so a scenario that types and presses return without
reading the field back sends whatever had landed by then (M-162).

The keyboard must be down for `field`: while it is up, `describe-all`
returns the keyboard's own tree and the composer is not in it.

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

    if verb == "field":
        fields = [e for e in els if (e.get("type") or "") == "TextField"]
        if not fields:
            print("ui: no text field on screen — is the keyboard up?", file=sys.stderr)
            sys.exit(1)
        if len(fields) > 1:
            print(f"ui: {len(fields)} text fields on screen, want one", file=sys.stderr)
            sys.exit(1)
        print(fields[0].get("AXValue") or "", end="")
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
