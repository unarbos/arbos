# Bundled Inter subsets

**Inter** by Rasmus Andersson — <https://rsms.me/inter/> — Copyright 2016 The Inter Project
Authors, licensed under the **SIL Open Font License, Version 1.1**. The licence is
[`Inter-LICENSE.txt`](Inter-LICENSE.txt) in this folder, verbatim from the release below.

## The files

| File | What |
| --- | --- |
| `Inter-Regular-subset.ttf` | 205 codepoints (Latin letters, digits, common punctuation) |
| `Inter-SemiBold-subset.ttf` | the same 205 codepoints |
| `Inter-LICENSE.txt` | the OFL 1.1 text these two are licensed under |

Both are cuts of the **Inter 4.1** release (`extras/ttf/Inter-Regular.ttf` and
`extras/ttf/Inter-SemiBold.ttf`, internal version `4.001;git-9221beed3`), made with
`pyftsubset`: 2937 glyphs become 279, and ~410 KB per face becomes ~48 KB. Nothing else
about the faces differs from upstream, so the metrics and outlines are the release's own.

Inter declares no Reserved Font Name, so a subset may keep the name `Inter`.

## Who ships them

`include_bytes!` compiles them into two binaries, so they are **distributed**, not merely
built with:

- `arbos-mac` — the SemiBold face for notification avatar initials
  (`crates/arbos-mac/src/ui/avatars.rs`), both faces for chart PNGs. Reaches people inside
  `Arbos.app` (the DMG and the updater tarball).
- `arbos-agentd` — both faces for the chart PNG fallback, via the `chart-png` feature.

`arbos-relay`, `arbosctl` and `arbos-ffi` (the iPhone app's Rust core) leave `chart-png`
off and embed no font.

## How the licence travels

OFL 1.1 clause 2 lets a distributed copy carry its licence as a stand-alone text file, a
human-readable header, or a machine-readable metadata field. All three apply here:

1. **This folder** — `Inter-LICENSE.txt` sits beside the fonts in the source tree.
2. **The Mac app bundle** — `scripts/bundle-mac.sh` copies it to
   `Arbos.app/Contents/Resources/Inter-LICENSE.txt`, so a downloaded app carries it.
3. **The fonts themselves** — `name` table ID 13 (licence description) and ID 14 (licence
   URL), which is what a font inspector and macOS's own Font Book read.

## Re-subsetting

`pyftsubset` defaults to `--name-IDs=0,1,2,3,4,5,6`. That keeps the copyright string and
**drops IDs 13 and 14**, which is how these two binaries first shipped without their
licence. Any future cut must pass `--name-IDs+=13,14`, or run
`scripts/fonts/inter-license-names.py --write` afterwards.
`scripts/fonts/inter-license-names.py --check` answers whether the committed files are
compliant right now.
