"""Generate favicon/logo assets from the tree-of-life source image.

Run: uv run --with pillow python scripts/gen_favicons.py
"""

import base64
import io
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "tree.jpeg"

# Directories that consume the icon set.
WEB_DIRS = [ROOT / "web" / "public", ROOT / "web" / "dist"]
SITE_DIRS = [ROOT / "internal" / "forest" / "site", ROOT / "site" / "public"]

BG = (255, 255, 255, 255)  # source art is a dark tree on white; keep it legible


def trim_whitespace(src: Image.Image) -> Image.Image:
    """Crop the near-white border so the tree fills the frame."""
    rgb = src.convert("RGB")
    gray = rgb.convert("L")
    # Pixels darker than ~244 count as content; invert so content is bright.
    mask = gray.point(lambda p: 255 if p < 244 else 0)
    box = mask.getbbox()
    return src.crop(box) if box else src


def square_master(size: int = 1024) -> Image.Image:
    """Trim, then center the source on a square white canvas with a small margin."""
    src = trim_whitespace(Image.open(SRC).convert("RGBA"))
    canvas = Image.new("RGBA", (size, size), BG)
    margin = int(size * 0.08)
    box = size - 2 * margin
    scale = min(box / src.width, box / src.height)
    w, h = int(src.width * scale), int(src.height * scale)
    resized = src.resize((w, h), Image.LANCZOS)
    canvas.paste(resized, ((size - w) // 2, (size - h) // 2), resized)
    return canvas


def png(master: Image.Image, size: int) -> bytes:
    img = master.resize((size, size), Image.LANCZOS)
    buf = io.BytesIO()
    img.save(buf, format="PNG")
    return buf.getvalue()


def main() -> None:
    master = square_master()

    sizes = {
        "favicon-16.png": 16,
        "favicon-32.png": 32,
        "favicon-96x96.png": 96,
        "favicon.png": 96,
        "apple-touch-icon.png": 180,
    }
    png_bytes = {name: png(master, s) for name, s in sizes.items()}

    # Multi-resolution .ico
    ico_buf = io.BytesIO()
    master.resize((256, 256), Image.LANCZOS).save(
        ico_buf, format="ICO", sizes=[(16, 16), (32, 32), (48, 48), (64, 64)]
    )
    ico_bytes = ico_buf.getvalue()

    # SVG wrapper embedding a crisp PNG so favicon.svg keeps working.
    b64 = base64.b64encode(png(master, 256)).decode("ascii")
    svg = (
        '<svg xmlns="http://www.w3.org/2000/svg" width="256" height="256" '
        'viewBox="0 0 256 256">'
        f'<image width="256" height="256" href="data:image/png;base64,{b64}"/>'
        "</svg>\n"
    ).encode("ascii")

    def write(d: Path, name: str, data: bytes) -> None:
        d.mkdir(parents=True, exist_ok=True)
        (d / name).write_bytes(data)
        print(f"wrote {(d / name).relative_to(ROOT)} ({len(data)} bytes)")

    for d in WEB_DIRS:
        for name, data in png_bytes.items():
            write(d, name, data)
        write(d, "favicon.ico", ico_bytes)
        write(d, "favicon.svg", svg)

    for d in SITE_DIRS:
        write(d, "favicon-32.png", png_bytes["favicon-32.png"])
        write(d, "favicon-96x96.png", png_bytes["favicon-96x96.png"])
        write(d, "apple-touch-icon.png", png_bytes["apple-touch-icon.png"])
        write(d, "favicon.ico", ico_bytes)


if __name__ == "__main__":
    main()
