"""Pick the simulator to photograph the app on: the newest iOS runtime the runner
has, and the newest iPhone Pro device type.

A Pro because it has a notch and a known width, so the layout is judged on the
shape of phone the app is actually used on. Chosen at run time rather than pinned
to a name, because the runner image changes its Xcode — and therefore its device
types and runtimes — without warning.

Prints two lines: the runtime identifier, then the device type identifier.
"""

import json
import re
import subprocess
import sys


def simctl(kind):
    out = subprocess.run(
        ["xcrun", "simctl", "list", kind, "-j"],
        check=True, capture_output=True, text=True,
    ).stdout
    return json.loads(out)[kind]


def version(text):
    return [int(part) for part in text.split(".")]


runtimes = [
    r for r in simctl("runtimes")
    if r.get("isAvailable") and "SimRuntime.iOS" in r["identifier"]
]
if not runtimes:
    sys.exit("no available iOS simulator runtime on this runner")

phones = [
    d for d in simctl("devicetypes")
    if re.fullmatch(r"iPhone (\d+) Pro", d["name"])
]
if not phones:
    sys.exit("no iPhone Pro device type on this runner")

newest_runtime = max(runtimes, key=lambda r: version(r["version"]))
newest_phone = max(phones, key=lambda d: int(re.search(r"\d+", d["name"]).group()))

print(newest_runtime["identifier"])
print(newest_phone["identifier"])
print(f"{newest_phone['name']} on {newest_runtime['name']}", file=sys.stderr)
