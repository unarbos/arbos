#!/usr/bin/env python3
"""Pair the two legs of an iPhone capture run and measure what changed between them.

    pair-screenshots.py <legs dir> <out dir>

`<legs dir>` holds the downloaded artifacts `shots-base/` and `shots-head/`, whose PNGs are named
`<leg>-<shot>.png` by ArbosUITests/CaptureRigUITests. A shot name present in both legs is a pair.

The point of the measurement is the one made in internal/ios-fade-measurement-rig.md: a brightness
or spacing fix cannot be judged by eye, and a reviewer should be handed numbers with the pictures.
Nothing here is time-based, so all of it is valid on a hosted runner — a still is a still.

Per pair it reports the changed-pixel count, the largest per-channel difference, the bounding box
of the change, and each side's peak and mean luminance. Peak luminance is the figure the fade rig
keys on: text drawn at full opacity peaks at 255, and a row faded to 0.90 opacity over a dark page
peaks near 238.
"""

import os
import shutil
import sys
from pathlib import Path

from PIL import Image, ImageChops, ImageDraw, ImageFont

LABEL_BAND = 72
GUTTER = 16
# PNG is lossless, so any non-zero difference is real. The second figure separates "a pixel moved"
# from "a pixel changed enough for a person to see it".
VISIBLE = 8


def font(size):
    try:
        return ImageFont.load_default(size=size)
    except TypeError:  # Pillow older than 10.1 has no size here
        return ImageFont.load_default()


def luminance(image):
    grey = image.convert("L")
    return grey.getextrema()[1], round(sum(i * n for i, n in enumerate(grey.histogram())) / (grey.width * grey.height), 1)


def shots(leg_dir):
    """`{shot name: path}` for one leg, keyed by the part after `<leg>-`."""
    out = {}
    for png in sorted(leg_dir.glob("*.png")):
        name = png.stem.split("-", 1)[1] if "-" in png.stem else png.stem
        out[name] = png
    return out


def contact_sheet(name, before, after, dest):
    """Before on the left, after on the right, each captioned, padded to a common size."""
    width = max(before.width, after.width)
    height = max(before.height, after.height)
    sheet = Image.new("RGB", (width * 2 + GUTTER, height + LABEL_BAND), (24, 24, 26))
    sheet.paste(before, (0, LABEL_BAND))
    sheet.paste(after, (width + GUTTER, LABEL_BAND))
    draw = ImageDraw.Draw(sheet)
    # Plain ASCII: the bundled default font has no glyph for an em dash and draws a tofu box.
    draw.text((12, 20), f"BEFORE: {name}", font=font(34), fill=(235, 235, 235))
    draw.text((width + GUTTER + 12, 20), f"AFTER: {name}", font=font(34), fill=(235, 235, 235))
    sheet.save(dest)


def compare(name, before_path, after_path, pairs_dir):
    before = Image.open(before_path).convert("RGB")
    after = Image.open(after_path).convert("RGB")
    contact_sheet(name, before, after, pairs_dir / f"{name}.png")

    row = {"name": name, "size": f"{before.width}x{before.height}"}
    if before.size != after.size:
        row["verdict"] = f"SIZE DIFFERS — before {before.width}x{before.height}, after {after.width}x{after.height}"
        return row

    diff = ImageChops.difference(before, after)
    flat = diff.convert("L")
    changed = sum(n for value, n in enumerate(flat.histogram()) if value > 0)
    visible = sum(n for value, n in enumerate(flat.histogram()) if value >= VISIBLE)
    total = before.width * before.height
    box = flat.getbbox()

    row["changed"] = f"{changed} ({changed / total:.2%})"
    row["visible"] = f"{visible} ({visible / total:.2%})"
    row["max_delta"] = max(channel.getextrema()[1] for channel in diff.split())
    row["box"] = "none" if box is None else f"{box[0]},{box[1]} → {box[2]},{box[3]}"
    before_peak, before_mean = luminance(before)
    after_peak, after_mean = luminance(after)
    row["peak"] = f"{before_peak} → {after_peak}"
    row["mean"] = f"{before_mean} → {after_mean}"
    row["verdict"] = "identical — the fix changed nothing on this screen" if changed == 0 else "differs"
    return row


def provenance():
    """What was shot and how, so the artifact still explains itself in a month."""
    fields = [
        ("before", f"`{os.environ.get('BASE_REF', '?')}` at `{os.environ.get('BASE_SHA', '?')}`"),
        ("after", f"`{os.environ.get('HEAD_REF', '?')}` at `{os.environ.get('HEAD_SHA', '?')}`"),
        ("device", f"{os.environ.get('DEVICE', '?')}, {os.environ.get('APPEARANCE', '?')}"),
        ("launch", f"`{os.environ.get('SHOT_LAUNCH_ENV', '')}`"),
        ("script", f"`{os.environ.get('SHOT_SCRIPT', '')}`"),
    ]
    return ["| | |", "|---|---|"] + [f"| {name} | {value} |" for name, value in fields]


def main():
    legs_dir, out_dir = Path(sys.argv[1]), Path(sys.argv[2])
    pairs_dir = out_dir / "pairs"
    pairs_dir.mkdir(parents=True, exist_ok=True)

    found = {}
    for leg in ("base", "head"):
        source = legs_dir / f"shots-{leg}"
        label = {"base": "before", "head": "after"}[leg]
        if source.is_dir():
            shutil.copytree(source, out_dir / label, dirs_exist_ok=True)
            found[leg] = shots(source)
        else:
            found[leg] = {}

    lines = ["## iPhone capture — pairs", "", *provenance(), ""]
    missing = [leg for leg in ("base", "head") if not found[leg]]
    if missing:
        lines += [
            f"**{' and '.join(missing)} produced no screenshots.** That leg failed; read its "
            "`capture-logs-*` artifact before changing anything, and re-dispatch if the log shows "
            "no error lines — an interrupted build or a runner that ran out of room is "
            "environmental, not a reason to edit the rig.",
            "",
        ]

    both = sorted(set(found["base"]) & set(found["head"]))
    only_base = sorted(set(found["base"]) - set(found["head"]))
    only_head = sorted(set(found["head"]) - set(found["base"]))

    rows = [compare(name, found["base"][name], found["head"][name], pairs_dir) for name in both]

    if rows:
        lines += [
            "A pair is only evidence when the fixture data was identical in both builds and a "
            "control the bug does not touch is in the same frame. This job cannot check either; "
            "see the workflow's header.",
            "",
            "| shot | size | changed px | visible px | max delta | changed area | peak luminance | mean luminance |",
            "|---|---|---|---|---|---|---|---|",
        ]
        for r in rows:
            if "changed" not in r:
                lines.append(f"| `{r['name']}` | {r['size']} | — | — | — | — | — | — |")
                lines.append(f"| | **{r['verdict']}** | | | | | | |")
                continue
            lines.append(
                f"| `{r['name']}` | {r['size']} | {r['changed']} | {r['visible']} | "
                f"{r['max_delta']} | {r['box']} | {r['peak']} | {r['mean']} |"
            )
        unchanged = [r["name"] for r in rows if r.get("verdict", "").startswith("identical")]
        if unchanged:
            lines += [
                "",
                f"**Nothing changed in {', '.join('`' + n + '`' for n in unchanged)}.** Either the "
                "fix does not reach this screen, or the step script never got to the state the bug "
                "needs. Check `steps-head.log` before concluding the fix is wrong.",
            ]
    elif not missing:
        lines += ["Both legs ran but no shot name appears in both. Give the same `shot <name>` steps to one script, not two.", ""]

    for label, names in (("before", only_base), ("after", only_head)):
        if names:
            lines += ["", f"Only in the {label} leg: {', '.join('`' + n + '`' for n in names)}."]

    for leg in ("base", "head"):
        log = legs_dir / f"shots-{leg}" / f"steps-{leg}.log"
        if log.is_file():
            shutil.copy(log, out_dir / log.name)

    lines += ["", "Step logs and the full-size shots are in this artifact."]
    (out_dir / "report.md").write_text("\n".join(lines) + "\n")
    print("\n".join(lines))


if __name__ == "__main__":
    main()
