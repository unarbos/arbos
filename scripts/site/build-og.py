"""Build the arbos.life social preview (1200x630) with D79 initials avatars.

Lays out a row of coloured-initial circles (palette = GET https://relay.arbos.life/avatar-colors,
index = sum of the name's UTF-8 bytes mod 7) beside the Arbos mark and wordmark in an HTML page
and screenshots it with headless Chromium (Playwright if installed, else google-chrome/chromium
on PATH). Output: site/assets/og/arbos-social.<sha256[:10]>.png — /assets/* is served with a 7-day
immutable cache, so the file name carries its content hash and site/index.html's og:image / twitter:image
are rewritten to the new name (older arbos-social.*.png files are removed). Run: python scripts/site/build-og.py
"""
import hashlib
import pathlib
import re
import shutil
import subprocess
import tempfile

REPO = pathlib.Path(__file__).resolve().parents[2]
OG_DIR = REPO / "site/assets/og"
OUT = OG_DIR / "arbos-social.png"
INDEX = REPO / "site/index.html"
MARK = (REPO / "site/assets/logo/arbos-mark.svg").read_text()

PALETTE = [  # light pairs (top, bottom); the OG image is light like the default page
    ("#FF845E", "#D45246"), ("#FEBB5B", "#F68136"), ("#B694F9", "#6C61DF"), ("#9AD164", "#46BA43"),
    ("#53EDD6", "#28C9B7"), ("#5CAFFA", "#408ACF"), ("#FF8AA5", "#E05F8E"),
]
CAST = ["Studio", "Affine", "Research", "Website", "Books", "Hiring", "Talent Scout", "Expense Manager", "Comms"]


def initials(name: str) -> str:
    parts = [p for p in name.replace("-", " ").split() if p]
    return "".join(p[0] for p in parts[:2]).upper() if len(parts) > 1 else parts[0][0].upper()


def circle(name: str, size: int) -> str:
    top, bottom = PALETTE[sum(name.encode()) % 7]
    return (
        f'<span class="ini" style="width:{size}px;height:{size}px;font-size:{size * 0.445:.0f}px;'
        f'background:linear-gradient(180deg,{top},{bottom})">{initials(name)}</span>'
    )


sizes = [72, 92, 112, 136, 164, 136, 112, 92, 72]
row = "".join(circle(n, s) for n, s in zip(CAST, sizes))
html = f"""<!doctype html><meta charset="utf-8"><style>
html,body{{margin:0;width:1200px;height:630px;background:#FCFCFC;font-family:-apple-system,BlinkMacSystemFont,"SF Pro Text","Segoe UI",Inter,Helvetica,Arial,sans-serif;overflow:hidden}}
.wrap{{position:relative;width:1200px;height:630px;display:flex;flex-direction:column;align-items:center;justify-content:center;gap:56px}}
.brand{{display:flex;align-items:center;gap:22px;color:#111}}
.brand svg{{width:72px;height:72px}}
.brand span{{font-size:64px;font-weight:600;letter-spacing:-0.02em}}
.row{{display:flex;align-items:center;gap:18px}}
.ini{{display:inline-flex;align-items:center;justify-content:center;border-radius:50%;color:#fff;font-weight:600;line-height:1;box-shadow:0 24px 60px rgba(0,0,0,.14)}}
.tag{{position:absolute;left:0;right:0;bottom:44px;text-align:center;font-size:26px;color:#6E6E73}}
</style><div class="wrap"><div class="brand">{MARK.replace('color="#000"', 'color="#111"')}<span>Arbos</span></div>
<div class="row">{row}</div><div class="tag">An always-on agent for every project, on your own cloud machine.</div></div>"""

tmp = pathlib.Path(tempfile.mkdtemp()) / "og.html"
tmp.write_text(html)
OUT.parent.mkdir(parents=True, exist_ok=True)
try:
    from playwright.sync_api import sync_playwright  # type: ignore

    with sync_playwright() as p:
        b = p.chromium.launch()
        page = b.new_page(viewport={"width": 1200, "height": 630})
        page.goto(tmp.as_uri())
        page.screenshot(path=str(OUT))
        b.close()
except ImportError:
    chrome = shutil.which("google-chrome") or shutil.which("chromium") or shutil.which("chromium-browser")
    if not chrome:
        raise SystemExit("install playwright (pip) or put google-chrome/chromium on PATH")
    subprocess.run([chrome, "--headless=new", "--hide-scrollbars", "--window-size=1200,630",
                    f"--screenshot={OUT}", tmp.as_uri()], check=True, capture_output=True)
digest = hashlib.sha256(OUT.read_bytes()).hexdigest()[:10]
final = OG_DIR / f"arbos-social.{digest}.png"
for old in OG_DIR.glob("arbos-social.*.png"):
    if old != final:
        old.unlink()
OUT.rename(final)
INDEX.write_text(re.sub(r"/assets/og/arbos-social(\.[0-9a-f]{10})?\.png", f"/assets/og/{final.name}", INDEX.read_text()))
print(final, final.stat().st_size // 1024, "KB")
