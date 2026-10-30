#!/usr/bin/env python3
"""Build the picker's data files (ARB-544): the emoji list the apps search, the relay's
sticker-pack manifest, and the starter stickers the apps bundle.

Inputs (downloaded into a scratch dir):
  - Unicode emoji-test.txt (names, groups, codepoints)            -> emoji list
  - microsoft/fluentui-emoji git tree (3D PNG paths; MIT)        -> "fluent-3d" pack
  - googlefonts noto-emoji-animation api.json (WebP; CC BY 4.0)  -> "noto-animated" pack

Outputs (committed):
  assets/fun/emoji.json                       shared emoji list (Mac include_str!, copied for iOS)
  crates/arbos-relay/assets/stickers/packs.json  pack manifest the relay serves and fetches from
  assets/fun/starter/*.png                    48 Fluent 3D stickers at 160 px (Mac include_bytes!)
  apps/ios/Arbos/Arbos/Resources/Fun/         copies of emoji.json + starter/ (folder reference)

Run: python3 scripts/fun/build-emoji-and-packs.py [--scratch DIR]
Needs: python3 with Pillow (for the starter resize) and network access.
"""
import argparse
import io
import json
import pathlib
import re
import sys
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parents[2]
EMOJI_TEST = "https://unicode.org/Public/emoji/16.0/emoji-test.txt"
FLUENT_TREE = "https://api.github.com/repos/microsoft/fluentui-emoji/git/trees/main?recursive=1"
FLUENT_RAW = "https://raw.githubusercontent.com/microsoft/fluentui-emoji/main/"
NOTO_API = "https://googlefonts.github.io/noto-emoji-animation/data/api.json"
NOTO_WEBP = "https://fonts.gstatic.com/s/e/notoemoji/latest/{cp}/512.webp"

# Unicode group -> the apps' short category id and order.
GROUPS = [
    ("Smileys & Emotion", "smileys"),
    ("People & Body", "people"),
    ("Animals & Nature", "nature"),
    ("Food & Drink", "food"),
    ("Travel & Places", "travel"),
    ("Activities", "activity"),
    ("Objects", "objects"),
    ("Symbols", "symbols"),
    ("Flags", "flags"),
]
TONE_CODEPOINTS = {"1F3FB", "1F3FC", "1F3FD", "1F3FE", "1F3FF"}

# Sticker packs take these Unicode groups / subgroups (Fluent has no flags).
STICKER_GROUPS = {"Smileys & Emotion", "Animals & Nature", "Food & Drink", "Activities", "Objects",
                  "Travel & Places"}
STICKER_SUBGROUPS = {"hand-fingers-open", "hand-fingers-partial", "hand-single-finger",
                     "hand-fingers-closed", "hands", "hand-prop", "body-parts", "heart",
                     "emotion", "person-fantasy", "transport-ground", "sky & weather"}

# The 48 Fluent 3D stickers both apps bundle so the Stickers tab is never empty offline.
STARTER = [
    "grinning face", "face with tears of joy", "rolling on the floor laughing", "smiling face with hearts",
    "smiling face with heart-eyes", "smiling face with sunglasses", "melting face", "thinking face",
    "smiling face with open hands",
    "face with raised eyebrow", "neutral face", "face with rolling eyes", "grimacing face",
    "loudly crying face", "crying face", "pleading face", "face screaming in fear", "exploding head",
    "partying face", "nerd face", "zany face", "winking face", "face blowing a kiss",
    "saluting face", "skull", "ghost", "robot", "alien", "clown face", "pile of poo", "thumbs up",
    "thumbs down", "clapping hands", "folded hands", "waving hand", "OK hand", "flexed biceps",
    "red heart", "broken heart", "fire", "sparkles", "party popper", "hundred points", "eyes",
    "rocket", "trophy", "pizza", "hot beverage",
]


def fetch(url: str, scratch: pathlib.Path, name: str) -> bytes:
    cached = scratch / name
    if cached.exists():
        return cached.read_bytes()
    req = urllib.request.Request(url, headers={"User-Agent": "arbos-fun-build"})
    data = urllib.request.urlopen(req, timeout=60).read()
    cached.write_bytes(data)
    return data


def parse_emoji_test(text: str):
    """fully-qualified, non-tone entries -> list of dicts; tone support flagged per base."""
    group = subgroup = None
    rows = []
    toned = set()
    for line in text.splitlines():
        if line.startswith("# group:"):
            group = line.split(":", 1)[1].strip()
            continue
        if line.startswith("# subgroup:"):
            subgroup = line.split(":", 1)[1].strip()
            continue
        if not line or line.startswith("#"):
            continue
        m = re.match(r"^([0-9A-F ]+?)\s*;\s*(\S+)\s*#\s*(\S+)\s+E\d+\.\d+\s+(.*)$", line)
        if not m:
            continue
        cps, status, char, name = m.groups()
        if status != "fully-qualified":
            continue
        cp_list = cps.split()
        if any(c in TONE_CODEPOINTS for c in cp_list):
            base = " ".join(c for c in cp_list if c not in TONE_CODEPOINTS)
            toned.add(base)
            continue
        if group == "Component":
            continue
        rows.append({"cps": cp_list, "char": char, "name": name, "group": group, "subgroup": subgroup})
    for r in rows:
        key = " ".join(r["cps"])
        # The toned form drops FE0F after the base, so match on the base scalar too.
        r["tone"] = key in toned or " ".join(c for c in r["cps"] if c != "FE0F") in toned
    return rows


# Unicode 16 names that differ from Fluent's folder names.
FLUENT_ALIASES = {
    "smiling face with open hands": "Hugging face",
    "face with crossed-out eyes": "Knocked-out face",
    "enraged face": "Pouting face",
    "ZZZ": "Zzz",
    "OK hand": "Ok hand",
    "Santa Claus": "Santa claus",
    "Mrs. Claus": "Mrs claus",
    "T-Rex": "T-rex",
}


def fluent_name(name: str) -> str:
    """CLDR short name -> Fluent asset folder name (first letter upper, same words)."""
    if name in FLUENT_ALIASES:
        return FLUENT_ALIASES[name]
    n = name.replace("’", "'")
    n = n[:1].upper() + n[1:].lower() if n.isupper() else n[:1].upper() + n[1:]
    return n


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--scratch", default="/tmp/arbos-fun-build")
    args = ap.parse_args()
    scratch = pathlib.Path(args.scratch)
    scratch.mkdir(parents=True, exist_ok=True)

    rows = parse_emoji_test(fetch(EMOJI_TEST, scratch, "emoji-test.txt").decode())
    cat_of = dict(GROUPS)

    # ---- emoji.json: [ { "e": "😀", "n": "grinning face", "c": "smileys", "t": 1 } ... ]
    emoji = []
    for r in rows:
        cat = cat_of.get(r["group"])
        if not cat:
            continue
        item = {"e": r["char"], "n": r["name"], "c": cat}
        if r["tone"]:
            item["t"] = 1
        emoji.append(item)
    out_emoji = ROOT / "assets/fun/emoji.json"
    out_emoji.parent.mkdir(parents=True, exist_ok=True)
    out_emoji.write_text(json.dumps({"version": 1, "categories": [c for _, c in GROUPS], "emoji": emoji},
                                    ensure_ascii=False, separators=(",", ":")) + "\n")
    print("emoji", len(emoji), out_emoji)

    # ---- fluent-3d pack
    tree = json.loads(fetch(FLUENT_TREE, scratch, "fluent-tree.json"))["tree"]
    by = {}
    for x in tree:
        p = x["path"]
        if not (p.endswith(".png") and "/3D/" in p):
            continue
        parts = p.split("/")
        if parts[2] == "3D":
            by.setdefault(parts[1], {})["plain"] = p
        elif parts[2] == "Default" and parts[3] == "3D":
            by.setdefault(parts[1], {})["default"] = p
    fluent_items = []
    missing = []
    for r in rows:
        if not (r["group"] in STICKER_GROUPS or r["subgroup"] in STICKER_SUBGROUPS):
            continue
        folder = by.get(fluent_name(r["name"]))
        if not folder:
            missing.append(r["name"])
            continue
        path = folder.get("default") or folder.get("plain")
        slug = re.sub(r"[^a-z0-9]+", "-", r["name"].lower()).strip("-")
        fluent_items.append({"id": slug, "name": r["name"], "emoji": r["char"], "mime": "image/png",
                             "upstream": FLUENT_RAW + urllib.request.quote(path)})
    print("fluent items", len(fluent_items), "unmatched", len(missing))

    # ---- noto-animated pack
    noto = json.loads(fetch(NOTO_API, scratch, "noto-anim.json"))
    name_by_cp = {}
    for r in rows:
        key = "_".join(c.lower() for c in r["cps"] if c != "FE0F")
        name_by_cp[key] = r
    noto_items = []
    for icon in sorted(noto["icons"], key=lambda i: -i.get("popularity", 0)):
        cp = icon["codepoint"]
        r = name_by_cp.get(cp) or name_by_cp.get(cp.replace("_fe0f", ""))
        name = r["name"] if r else icon["tags"][0].strip(":").replace("_", " ") if icon.get("tags") else cp
        char = r["char"] if r else "".join(chr(int(c, 16)) for c in cp.split("_"))
        slug = re.sub(r"[^a-z0-9]+", "-", name.lower()).strip("-") or cp
        noto_items.append({"id": slug, "name": name, "emoji": char, "mime": "image/webp",
                           "upstream": NOTO_WEBP.format(cp=cp)})
    print("noto items", len(noto_items))

    manifest = {
        "version": 1,
        "packs": [
            {"id": "fluent-3d", "name": "Fluent 3D", "publisher": "Microsoft", "license": "MIT",
             "licenseUrl": "https://github.com/microsoft/fluentui-emoji/blob/main/LICENSE",
             "attribution": "Fluent Emoji 3D · Microsoft · MIT", "animated": False,
             "pageUrl": "https://github.com/microsoft/fluentui-emoji", "items": fluent_items},
            {"id": "noto-animated", "name": "Noto Animated", "publisher": "Google", "license": "CC BY 4.0",
             "licenseUrl": "https://creativecommons.org/licenses/by/4.0/",
             "attribution": "Noto Animated Emoji · Google · CC BY 4.0", "animated": True,
             "pageUrl": "https://googlefonts.github.io/noto-emoji-animation/", "items": noto_items},
        ],
    }
    out_packs = ROOT / "crates/arbos-relay/assets/stickers/packs.json"
    out_packs.parent.mkdir(parents=True, exist_ok=True)
    out_packs.write_text(json.dumps(manifest, ensure_ascii=False, separators=(",", ":")) + "\n")
    print("packs", out_packs, out_packs.stat().st_size, "bytes")

    # ---- starter pack (48 Fluent 3D at 160 px)
    try:
        from PIL import Image
    except ImportError:
        print("Pillow missing: skipping the starter resize", file=sys.stderr)
        return
    starter_dir = ROOT / "assets/fun/starter"
    starter_dir.mkdir(parents=True, exist_ok=True)
    by_name = {i["name"]: i for i in fluent_items}
    starter = []
    for name in STARTER:
        item = by_name.get(name)
        if not item:
            print("starter missing", name, file=sys.stderr)
            continue
        raw = fetch(item["upstream"], scratch, "starter-" + item["id"] + ".png")
        im = Image.open(io.BytesIO(raw)).convert("RGBA")
        im.thumbnail((160, 160), Image.LANCZOS)
        buf = io.BytesIO()
        im.save(buf, "PNG", optimize=True)
        (starter_dir / f"{item['id']}.png").write_bytes(buf.getvalue())
        starter.append({"id": item["id"], "name": item["name"], "emoji": item["emoji"], "file": f"{item['id']}.png"})
    (starter_dir / "index.json").write_text(json.dumps({"pack": "fluent-3d", "items": starter},
                                                       ensure_ascii=False, separators=(",", ":")) + "\n")
    total = sum(p.stat().st_size for p in starter_dir.glob("*.png"))
    print("starter", len(starter), "files", total // 1024, "KB")

    # ---- iOS copies (a folder reference in the Xcode project)
    ios_dir = ROOT / "apps/ios/Arbos/Arbos/Resources/Fun"
    (ios_dir / "starter").mkdir(parents=True, exist_ok=True)
    (ios_dir / "emoji.json").write_bytes(out_emoji.read_bytes())
    for p in starter_dir.iterdir():
        (ios_dir / "starter" / p.name).write_bytes(p.read_bytes())
    print("ios copies", ios_dir)


if __name__ == "__main__":
    main()
