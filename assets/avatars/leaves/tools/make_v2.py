#!/usr/bin/env python3
"""Leaf avatars v2 (D92 amendment 3, Jacob 2026-10-23): the 4 × 4 leaf sheet as single-path
SVGs, the matching idle / gust Lottie files, and `manifest.json`.

Rule: a soft pastel disc and a darker, same-hue, flat leaf centred on it; the glyph is 15 %
larger than v1 (v1 spanned ~62 of the 100-unit box → 71 here) and centred on its opaque
bounding box. Each leaf is one `<path>` filled even-odd: the outline, then thin vein cut-outs
that show the disc through them (Jacob's "white vein cutouts"). No outline stroke, no shading.

Jacob's own files (the 4 × 4 sheet and the six hero PNGs) did not reach the store; these
shapes follow his description (coral maple, blue single leaf with veins, green double leaf,
violet single leaf, teal three-leaf sprig, pink double leaf, then yellow ginkgo, orange heart,
lime fern, deep-purple oak, …) and are the drop-in slot for his traced art: replace an SVG,
re-run this tool for its Lottie, keep the slug.

    python3 assets/avatars/leaves/tools/make_v2.py
"""

from __future__ import annotations

import json
import math
import os
import sys

ROOT = os.path.join(os.path.dirname(__file__), "..")
BOX = 100.0
SPAN = 71.0  # v1 leaves spanned ~62 units; +15 %
FPS = 60

# ----- palette: (name, disc pastel, leaf ink), indices 0–9 ------------------------------------
PALETTE = [
    ("Coral", "#F9D3C6", "#E0634F"),
    ("Royal blue", "#CFDFF8", "#2F5BC7"),
    ("Green", "#D3EBD2", "#3E9A5A"),
    ("Violet", "#E3D9F6", "#7B57C9"),
    ("Teal", "#CDEEE6", "#2A9C8C"),
    ("Pink", "#FAD5E3", "#D9558A"),
    ("Yellow", "#FBEFC2", "#D9A41E"),
    ("Orange", "#FBDDC0", "#E4802B"),
    ("Lime", "#E4F2C3", "#88B425"),
    ("Deep purple", "#DDD2EE", "#5E3E9E"),
]

# ----- geometry helpers ----------------------------------------------------------------------

Pt = tuple[float, float]


def rot(p: Pt, deg: float, about: Pt = (0.0, 0.0)) -> Pt:
    a = math.radians(deg)
    x, y = p[0] - about[0], p[1] - about[1]
    return (about[0] + x * math.cos(a) - y * math.sin(a), about[1] + x * math.sin(a) + y * math.cos(a))


def smooth(points: list[Pt], tension: float = 0.5) -> list:
    """Closed Catmull-Rom through `points` → cubic segments [(c1, c2, p)…] starting at points[0]."""
    n = len(points)
    segs = []
    for k in range(n):
        p0, p1, p2, p3 = points[(k - 1) % n], points[k], points[(k + 1) % n], points[(k + 2) % n]
        c1 = (p1[0] + (p2[0] - p0[0]) * tension / 3, p1[1] + (p2[1] - p0[1]) * tension / 3)
        c2 = (p2[0] - (p3[0] - p1[0]) * tension / 3, p2[1] - (p3[1] - p1[1]) * tension / 3)
        segs.append((c1, c2, p2))
    return segs


class Path:
    """Subpaths of (start, [segments]) where a segment is ('L', p) or ('C', c1, c2, p)."""

    def __init__(self):
        self.subs: list[tuple[Pt, list]] = []

    def smooth_closed(self, points: list[Pt], tension: float = 0.5):
        segs = [("C", c1, c2, p) for (c1, c2, p) in smooth(points, tension)]
        self.subs.append((points[0], segs))

    def polygon(self, points: list[Pt]):
        self.subs.append((points[0], [("L", p) for p in points[1:]]))

    def transform(self, f):
        out = Path()
        for start, segs in self.subs:
            ns = []
            for s in segs:
                if s[0] == "L":
                    ns.append(("L", f(s[1])))
                else:
                    ns.append(("C", f(s[1]), f(s[2]), f(s[3])))
            out.subs.append((f(start), ns))
        return out

    def points(self) -> list[Pt]:
        pts = []
        for start, segs in self.subs:
            pts.append(start)
            for s in segs:
                pts.extend(s[1:])
        return pts

    def bbox(self):
        xs = [p[0] for p in self.points()]
        ys = [p[1] for p in self.points()]
        return min(xs), min(ys), max(xs), max(ys)

    def svg_d(self) -> str:
        out = []
        for start, segs in self.subs:
            out.append(f"M{start[0]:.2f} {start[1]:.2f}")
            for s in segs:
                if s[0] == "L":
                    out.append(f"L{s[1][0]:.2f} {s[1][1]:.2f}")
                else:
                    out.append(
                        f"C{s[1][0]:.2f} {s[1][1]:.2f} {s[2][0]:.2f} {s[2][1]:.2f} {s[3][0]:.2f} {s[3][1]:.2f}"
                    )
            out.append("Z")
        return "".join(out)

    def lottie_shapes(self) -> list[dict]:
        """One Lottie `sh` per subpath (bezier vertices with in/out tangents)."""
        shapes = []
        for n, (start, segs) in enumerate(self.subs):
            v = [list(start)]
            o: list[list[float]] = []
            i: list[list[float]] = [[0.0, 0.0]]
            for s in segs:
                if s[0] == "L":
                    o.append([0.0, 0.0])
                    v.append(list(s[1]))
                    i.append([0.0, 0.0])
                else:
                    c1, c2, p = s[1], s[2], s[3]
                    prev = v[-1]
                    o.append([c1[0] - prev[0], c1[1] - prev[1]])
                    v.append(list(p))
                    i.append([c2[0] - p[0], c2[1] - p[1]])
            # Closed: the last segment ends where the first vertex is.
            if len(v) > 1 and abs(v[-1][0] - v[0][0]) < 1e-6 and abs(v[-1][1] - v[0][1]) < 1e-6:
                i[0] = i[-1]
                v.pop()
                i.pop()
            else:
                o.append([0.0, 0.0])
            shapes.append(
                {
                    "ty": "sh",
                    "nm": f"path{n + 1}",
                    "ind": n,
                    "ks": {"a": 0, "k": {"c": True, "v": [[round(a, 3) for a in p] for p in v], "i": [[round(a, 3) for a in p] for p in i], "o": [[round(a, 3) for a in p] for p in o]}},
                }
            )
        return shapes


# ----- leaf parts ----------------------------------------------------------------------------


def blade_points(length: float, width: float, tip: float = 1.0, base_round: float = 0.35, n: int = 14) -> list[Pt]:
    """A pointed blade along +y (base at origin, tip at (0, -length)); `tip` sharpens the apex."""
    pts = []
    for k in range(n + 1):
        t = k / n
        y = -length * t
        w = width / 2 * math.sin(math.pi * (t ** 0.9)) ** (0.75 + tip * 0.6)
        if t < 0.15:
            w *= 0.6 + base_round * t / 0.15
        pts.append((w, y))
    right = pts[1:-1]
    left = [(-x, y) for (x, y) in reversed(right)]
    return [pts[0]] + right + [pts[-1]] + left


def midrib(length: float, w0: float = 1.5, w1: float = 0.6, start: float = 0.08, end: float = 0.9) -> list[Pt]:
    y0, y1 = -length * start, -length * end
    return [(-w0 / 2, y0), (w0 / 2, y0), (w1 / 2, y1), (-w1 / 2, y1)]


def side_veins(length: float, width: float, count: int, spread: float = 0.42, w: float = 1.0) -> list[list[Pt]]:
    out = []
    for k in range(count):
        t = 0.28 + 0.5 * k / max(count - 1, 1)
        y = -length * t
        reach = width / 2 * math.sin(math.pi * t ** 0.9) * 0.62
        for sgn in (-1, 1):
            a = (sgn * reach, y - reach * spread)
            out.append([(0.0, y + w / 2), (0.0, y - w / 2), (a[0], a[1] - w / 2), (a[0], a[1] + w / 2)])
    return out


def stem(length: float, w: float = 2.2) -> list[Pt]:
    return [(-w / 2, 0.0), (w / 2, 0.0), (w / 2 * 0.8, length), (-w / 2 * 0.8, length)]


def lobed_points(lobes: int, r: float, depth: float, tip: float, rotation: float = -90.0, n: int = 96) -> list[Pt]:
    """A lobed outline (maple / oak) as a polar curve: lobes of depth `depth`, sharpened by `tip`."""
    pts = []
    for k in range(n):
        a = 2 * math.pi * k / n
        s = (1 + math.cos(lobes * a)) / 2
        rr = r * (1 - depth + depth * s ** tip)
        # Flatten the base so the stem has a seat.
        if math.cos(a) < -0.6:
            rr *= 0.92
        pts.append(rot((rr * math.cos(a), rr * math.sin(a)), rotation))
    return pts


# ----- the sixteen leaves ---------------------------------------------------------------------


def leaf_single(veins: int, width: float, tip: float, lean: float = 0.0) -> Path:
    p = Path()
    L = 60.0
    p.smooth_closed(blade_points(L, width, tip), 0.55)
    p.polygon(midrib(L))
    for v in side_veins(L, width, veins):
        p.polygon(v)
    p.polygon(stem(9.0))
    if lean:
        p = p.transform(lambda q: rot(q, lean))
    return p


def leaf_double(width: float, gap: float, tip: float, veins: int = 2) -> Path:
    p = Path()
    L = 44.0
    for sgn in (-1, 1):
        blade = Path()
        blade.smooth_closed(blade_points(L, width, tip), 0.55)
        blade.polygon(midrib(L, 1.2, 0.5))
        for v in side_veins(L, width, veins, spread=0.25, w=0.8):
            blade.polygon(v)
        blade = blade.transform(lambda q, s=sgn: rot(q, s * gap))
        p.subs.extend(blade.subs)
    p.polygon(stem(12.0))
    return p


def leaf_sprig(count: int, width: float, tip: float) -> Path:
    p = Path()
    L = 36.0
    angles = [0.0] if count == 1 else ([-48.0, 0.0, 48.0] if count == 3 else [-30.0, 30.0])
    for a in angles:
        blade = Path()
        blade.smooth_closed(blade_points(L, width, tip), 0.55)
        blade.polygon(midrib(L, 1.1, 0.5))
        blade = blade.transform(lambda q: (q[0], q[1] - 16.0))
        blade = blade.transform(lambda q, a=a: rot(q, a, (0.0, -16.0)))
        p.subs.extend(blade.subs)
    p.polygon(stem(14.0, 2.4))
    p.polygon([(-1.2, 0.0), (1.2, 0.0), (1.0, -16.0), (-1.0, -16.0)])
    return p


def leaf_lobed(lobes: int, depth: float, tip: float, veins: bool = True) -> Path:
    p = Path()
    r = 30.0
    pts = lobed_points(lobes, r, depth, tip)
    pts = [(x, y - r * 0.9) for (x, y) in pts]
    p.smooth_closed(pts, 0.5)
    if veins:
        for k in range(lobes):
            a = -90 + (k - (lobes - 1) / 2) * (180 / max(lobes - 1, 1)) * 0.9
            reach = r * (0.72 if abs(a + 90) < 1 else 0.6)
            end = rot((reach, 0.0), a, (0.0, 0.0))
            end = (end[0], end[1] - r * 0.9 + r * 0.25)
            base = (0.0, -r * 0.9 + r * 0.55)
            w = 1.2
            n = (-(end[1] - base[1]), end[0] - base[0])
            ln = math.hypot(*n) or 1.0
            n = (n[0] / ln * w / 2, n[1] / ln * w / 2)
            p.polygon([(base[0] + n[0], base[1] + n[1]), (base[0] - n[0], base[1] - n[1]), (end[0] - n[0], end[1] - n[1]), (end[0] + n[0], end[1] + n[1])])
    p.polygon(stem(10.0))
    return p


def leaf_fan() -> Path:
    """Ginkgo: a fan with a wavy top edge and a notch, on a long stem."""
    p = Path()
    pts = []
    r = 34.0
    for k in range(40):
        a = math.radians(-150 + 120 * k / 39)
        rr = r * (1 + 0.06 * math.sin(k * 1.7))
        if 17 <= k <= 22:
            rr *= 0.88
        pts.append((rr * math.cos(a), rr * math.sin(a) - 6.0))
    pts.append((0.0, 0.0))
    p.smooth_closed(pts, 0.45)
    for a in (-140.0, -120.0, -100.0, -80.0, -60.0, -40.0):
        end = (r * 0.8 * math.cos(math.radians(a)), r * 0.8 * math.sin(math.radians(a)) - 6.0)
        p.polygon([(-0.4, -1.0), (0.4, -1.0), (end[0] + 0.4, end[1]), (end[0] - 0.4, end[1])])
    p.polygon(stem(16.0, 2.0))
    return p


def leaf_heart() -> Path:
    p = Path()
    pts = []
    for k in range(48):
        t = 2 * math.pi * k / 48
        x = 16 * math.sin(t) ** 3
        y = -(13 * math.cos(t) - 5 * math.cos(2 * t) - 2 * math.cos(3 * t) - math.cos(4 * t))
        pts.append((x * 1.9, y * 1.9 - 30.0))
    p.smooth_closed(pts, 0.5)
    p.polygon(midrib(60.0, 1.4, 0.6, 0.1, 0.78))
    for v in side_veins(58.0, 52.0, 3, spread=0.5, w=0.9):
        p.polygon(v)
    p.polygon(stem(8.0))
    return p


def leaf_fern() -> Path:
    p = Path()
    L = 62.0
    p.polygon([(-1.1, 0.0), (1.1, 0.0), (0.4, -L), (-0.4, -L)])
    for k in range(7):
        t = 0.1 + 0.8 * k / 6
        y = -L * t
        size = 15.0 * (1 - 0.6 * abs(t - 0.45))
        for sgn in (-1, 1):
            leaflet = Path()
            leaflet.smooth_closed(blade_points(size, size * 0.5, 0.6), 0.55)
            leaflet = leaflet.transform(lambda q, s=sgn, yy=y: rot((q[0], q[1]), s * 62.0, (0.0, 0.0)))
            leaflet = leaflet.transform(lambda q, yy=y: (q[0], q[1] + yy))
            p.subs.extend(leaflet.subs)
    p.polygon(stem(8.0))
    return p


def leaf_clover() -> Path:
    p = Path()
    for a in (-120.0, 0.0, 120.0):
        pts = []
        for k in range(24):
            t = 2 * math.pi * k / 24
            rr = 13.0 * (1 - 0.18 * math.cos(2 * t))
            pts.append((rr * math.cos(t), rr * math.sin(t) - 14.0))
        pts = [rot(q, a) for q in pts]
        p.smooth_closed(pts, 0.5)
        hub = rot((0.0, -14.0), a)
        p.polygon([(hub[0] - 0.5, hub[1] - 0.5), (hub[0] + 0.5, hub[1] - 0.5), (hub[0] * 1.9 + 0.5, hub[1] * 1.9 - 0.5), (hub[0] * 1.9 - 0.5, hub[1] * 1.9 + 0.5)])
    p = p.transform(lambda q: (q[0], q[1] - 20.0))
    p.polygon(stem(16.0, 2.0))
    return p


def leaf_holly() -> Path:
    p = Path()
    L = 60.0
    pts = []
    n = 28
    for k in range(n + 1):
        t = k / n
        y = -L * t
        w = 20.0 * math.sin(math.pi * t ** 0.95)
        spike = 1.0 + (0.28 if k % 4 == 2 else 0.0)
        pts.append((w * spike, y))
    right = pts[1:-1]
    left = [(-x, y) for (x, y) in reversed(right)]
    p.smooth_closed([pts[0]] + right + [pts[-1]] + left, 0.35)
    p.polygon(midrib(L, 1.6, 0.6))
    for v in side_veins(L, 40.0, 3, w=0.9):
        p.polygon(v)
    p.polygon(stem(8.0))
    return p


def leaf_palm() -> Path:
    p = Path()
    for a in (-70.0, -45.0, -20.0, 5.0, 30.0, 55.0, 80.0):
        blade = Path()
        blade.smooth_closed(blade_points(34.0, 7.0, 1.2), 0.55)
        blade = blade.transform(lambda q, aa=a: rot((q[0], q[1] - 4.0), aa + 90.0 - 90.0))
        blade = blade.transform(lambda q, aa=a: rot(q, (aa + 5.0) * 0.0))
        p.subs.extend(blade.subs)
    # Fan the blades out from the hub.
    out = Path()
    for n, (start, segs) in enumerate(p.subs):
        a = (-70.0, -45.0, -20.0, 5.0, 30.0, 55.0, 80.0)[n]
        f = lambda q, aa=a: rot(q, aa)
        ns = [(("L", f(s[1])) if s[0] == "L" else ("C", f(s[1]), f(s[2]), f(s[3]))) for s in segs]
        out.subs.append((f(start), ns))
    out = out.transform(lambda q: (q[0], q[1] - 6.0))
    out.polygon(stem(16.0, 2.2))
    return out


LEAVES = [
    # slug, name, default hue, builder
    ("leaf-01-maple", "Maple", 0, lambda: leaf_lobed(5, 0.42, 1.6)),
    ("leaf-02-veined", "Veined", 1, lambda: leaf_single(3, 44.0, 0.9)),
    ("leaf-03-pair", "Pair", 2, lambda: leaf_double(24.0, 24.0, 0.9, veins=1)),
    ("leaf-04-spear", "Spear", 3, lambda: leaf_single(2, 34.0, 1.4, lean=-8.0)),
    ("leaf-05-sprig", "Sprig", 4, lambda: leaf_sprig(3, 18.0, 0.8)),
    ("leaf-06-twin", "Twin", 5, lambda: leaf_double(28.0, 30.0, 0.5, veins=0)),
    ("leaf-07-oak", "Oak", 9, lambda: leaf_lobed(7, 0.3, 0.9, veins=False)),
    ("leaf-08-ginkgo", "Ginkgo", 6, leaf_fan),
    ("leaf-09-heart", "Heart", 7, leaf_heart),
    ("leaf-10-fern", "Fern", 8, leaf_fern),
    ("leaf-11-willow", "Willow", 2, lambda: leaf_single(0, 16.0, 1.6, lean=12.0)),
    ("leaf-12-clover", "Clover", 4, leaf_clover),
    ("leaf-13-aspen", "Aspen", 6, lambda: leaf_single(3, 54.0, 0.4)),
    ("leaf-14-holly", "Holly", 2, leaf_holly),
    ("leaf-15-palm", "Palm", 1, leaf_palm),
    ("leaf-16-birch", "Birch", 7, lambda: leaf_lobed(3, 0.22, 0.7, veins=True)),
]

IDLE_FRAMES = [150, 160, 140, 170, 150, 144, 156, 166, 136, 148, 154, 162, 142, 158, 146, 164]
GUST_FRAMES = 60
IDLE_DEGREES = 3.0
GUST_DEGREES = 9.0


def fit(path: Path) -> tuple[Path, Pt]:
    """Scale the glyph to span SPAN on its longer side and centre its bbox at (50, 50).
    Returns the path and the stem pivot (bottom centre of the bbox)."""
    x0, y0, x1, y1 = path.bbox()
    w, h = x1 - x0, y1 - y0
    s = SPAN / max(w, h)
    cx, cy = (x0 + x1) / 2, (y0 + y1) / 2
    out = path.transform(lambda q: (BOX / 2 + (q[0] - cx) * s, BOX / 2 + (q[1] - cy) * s))
    nx0, ny0, nx1, ny1 = out.bbox()
    return out, ((nx0 + nx1) / 2, ny1)


def idle_angle(t: float, period: float) -> float:
    return math.sin(t / period * math.tau) * IDLE_DEGREES


def gust_angle(t: float) -> float:
    if not 0.0 <= t < 1.0:
        return 0.0
    envelope = min((1.0 - t) * math.exp(-3.0 * t) * 4.0, 1.0)
    return math.sin(t * math.pi * 2.5) * envelope * GUST_DEGREES


def lottie(slug: str, path: Path, pivot: Pt, frames: int, angle, ink: str) -> dict:
    keys = []
    step = 3 if frames <= 60 else 6
    for f in range(0, frames + 1, step):
        a = angle(f / FPS)
        keys.append({"t": f, "s": [round(a, 3)], "o": {"x": [0.333], "y": [0.333]}, "i": {"x": [0.667], "y": [0.667]}})
    r, g, b = int(ink[1:3], 16) / 255, int(ink[3:5], 16) / 255, int(ink[5:7], 16) / 255
    group = {
        "ty": "gr",
        "nm": "leaf",
        "np": len(path.subs) + 2,
        "bm": 0,
        "it": path.lottie_shapes()
        + [
            {"ty": "fl", "nm": "leaf", "c": {"a": 0, "k": [round(r, 3), round(g, 3), round(b, 3), 1]}, "o": {"a": 0, "k": 100}, "r": 2, "bm": 0},
            {
                "ty": "tr",
                "p": {"a": 0, "k": [0, 0]},
                "a": {"a": 0, "k": [0, 0]},
                "s": {"a": 0, "k": [100, 100]},
                "r": {"a": 0, "k": 0},
                "o": {"a": 0, "k": 100},
                "sk": {"a": 0, "k": 0},
                "sa": {"a": 0, "k": 0},
            },
        ],
    }
    layer = {
        "ddd": 0,
        "ind": 1,
        "ty": 4,
        "nm": "leaf",
        "sr": 1,
        "ks": {
            "o": {"a": 0, "k": 100},
            "p": {"a": 0, "k": [round(pivot[0], 3), round(pivot[1], 3), 0]},
            "a": {"a": 0, "k": [round(pivot[0], 3), round(pivot[1], 3), 0]},
            "r": {"a": 1, "k": keys},
            "s": {"a": 0, "k": [100, 100, 100]},
            "sk": {"a": 0, "k": 0},
            "sa": {"a": 0, "k": 0},
        },
        "ao": 0,
        "shapes": [group],
        "ip": 0,
        "op": frames,
        "st": 0,
        "bm": 0,
    }
    return {
        "v": "5.7.4",
        "fr": FPS,
        "ip": 0,
        "op": frames,
        "w": 100,
        "h": 100,
        "nm": slug,
        "ddd": 0,
        "assets": [],
        "meta": {"g": "arbos make_v2.py", "a": "", "k": "", "d": "", "tc": ""},
        "layers": [layer],
        "markers": [],
    }


def main() -> int:
    svg_dir = os.path.join(ROOT, "svg")
    lot_dir = os.path.join(ROOT, "lottie")
    os.makedirs(svg_dir, exist_ok=True)
    os.makedirs(lot_dir, exist_ok=True)
    for name in os.listdir(svg_dir):
        os.remove(os.path.join(svg_dir, name))
    for name in os.listdir(lot_dir):
        os.remove(os.path.join(lot_dir, name))
    manifest_leaves = []
    for n, (slug, name, hue, build) in enumerate(LEAVES):
        path, pivot = fit(build())
        d = path.svg_d()
        svg = (
            f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" width="100" height="100">'
            f'<path fill="currentColor" fill-rule="evenodd" d="{d}"/></svg>\n'
        )
        with open(os.path.join(svg_dir, f"{slug}.svg"), "w") as f:
            f.write(svg)
        ink = PALETTE[hue][2]
        period = IDLE_FRAMES[n] / FPS
        with open(os.path.join(lot_dir, f"{slug}-idle.json"), "w") as f:
            json.dump(lottie(slug, path, pivot, IDLE_FRAMES[n], lambda t, p=period: idle_angle(t, p), ink), f, separators=(",", ":"))
            f.write("\n")
        with open(os.path.join(lot_dir, f"{slug}-gust.json"), "w") as f:
            json.dump(lottie(slug, path, pivot, GUST_FRAMES, gust_angle, ink), f, separators=(",", ":"))
            f.write("\n")
        x0, y0, x1, y1 = path.bbox()
        manifest_leaves.append(
            {
                "id": n + 1,
                "name": name,
                "slug": slug,
                "svg": f"svg/{slug}.svg",
                "lottie": {"idle": f"lottie/{slug}-idle.json", "gust": f"lottie/{slug}-gust.json"},
                "idle_frames": IDLE_FRAMES[n],
                "gust_frames": GUST_FRAMES,
                "pivot": [round(pivot[0], 3), round(pivot[1], 3)],
                "bbox": [round(x0, 2), round(y0, 2), round(x1, 2), round(y1, 2)],
                "default_color": {"index": hue, "name": PALETTE[hue][0], "disc": PALETTE[hue][1], "ink": PALETTE[hue][2]},
            }
        )
    manifest = {
        "version": "2.0.0",
        "name": "arbos-leaf-avatars",
        "rule": "a soft pastel disc and a darker same-hue flat leaf centred on it; glyph spans 71 of the 100-unit box (v1 + 15 %), centred on its opaque bbox; even-odd fill, veins are cut-outs",
        "avatar": {"viewBox": "0 0 100 100", "leaf_span": SPAN, "circle_drawn_by_app": True},
        "lottie": {
            "schema": "bodymovin 5.7",
            "size": [100, 100],
            "fps": FPS,
            "fill_name": "leaf",
            "recolor_keypath": "**.leaf.Color",
            "idle": "seamless loop, starts and ends at rest pose",
            "gust": "one-shot 1 s hover gust, starts and ends at rest pose",
            "generated_by": "tools/make_v2.py",
        },
        "palette": [{"index": i, "name": n, "disc": d, "ink": k} for i, (n, d, k) in enumerate(PALETTE)],
        "default_rule": "h = fnv1a64(id); leaf = h mod 16; colour = the leaf's default_color (an owner's pick overrides both)",
        "leaves": manifest_leaves,
    }
    with open(os.path.join(ROOT, "manifest.json"), "w") as f:
        json.dump(manifest, f, indent=2)
        f.write("\n")
    print(f"wrote {len(LEAVES)} leaves")
    return 0


if __name__ == "__main__":
    sys.exit(main())
