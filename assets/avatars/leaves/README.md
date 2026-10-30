# Arbos leaf avatars v2

Sixteen flat leaf silhouettes for agent avatars (Jacob's 4 × 4 sheet, 2026-10-23). The app draws the pastel circle; these files are the leaf only.

## The rule
- A **soft pastel disc** and a **darker, same-hue, flat leaf** centred on it (style ref: coral maple on peach). The ten hues are in `manifest.json` → `palette`: `disc` (pastel) and `ink` (leaf).
- The glyph spans **71 of the 100-unit box** (v1 + 15 %) and is centred on its opaque bounding box, so the app draws the box edge to edge on the disc with no offset.
- Flat vector, no outline, no shading; veins are **cut-outs** (even-odd fill) that show the disc through them.
- Default leaf is deterministic from the agent id (`fnv1a64`, `leaf = h mod 16`); the default colour is the leaf's own hue. Owners may override both. Old stored picks keep their indices (v1 leaf 0–8 → the v2 leaf at the same index).

## Files
- `manifest.json` — every leaf (id, slug, name, SVG, idle/gust Lottie, pivot, bbox, default hue) plus the palette. Source of truth; `arbos-core` has a test against it.
- `svg/leaf-NN-name.svg` — one `currentColor` path (even-odd) in a 100 × 100 viewBox.
- `lottie/leaf-NN-name-idle.json` / `-gust.json` — bodymovin 5.7, 100 × 100, 60 fps: a seamless 2.3–2.8 s sway loop (±3° about the stem base, lengths differ per leaf) and a one-shot 1 s gust (spring to ~9°, back to rest). One shape layer `leaf` → group `leaf` → fill `leaf`; recolour via keypath `**.leaf.Color`.
- `tools/make_v2.py` — generates the SVGs, the Lottie files and the manifest from the parametric shapes. Jacob's traced art: drop the SVG in under the same slug and re-run the tool for its Lottie and manifest row (`fit()` recentres and rescales it).

Honour reduced motion: show the static leaf and skip idle and gust.
