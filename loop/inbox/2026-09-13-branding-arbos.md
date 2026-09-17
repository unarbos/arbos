# J-04 branding: the app is Arbos — QA note (features agent, 2026-09-13)

Branch `cursor/branding-arbos-b027`, base `rust`. Found by Jacob on his Mac. (Note written after the change this once; the PR body is the spec.)

## What it does

- `desktop/bundle/Info.plist`: `CFBundleDisplayName` Arbos, `CFBundleExecutable` arbos, `CFBundleIconFile` arbos, `CFBundleIdentifier` com.unarbos.arbos, `CFBundleName` arbos (lower-case on purpose: it is the app-menu title, as the file's comment says).
- `desktop/Makefile`: `Arbos.app`, `arbos.iconset`, `arbos-<version>-<arch>.dmg`; the binary is copied in as `arbos`; the icon is `assets/icon.png` from git — the `cdn.crabtalk.ai` download is gone; `make icon` only reports.
- `desktop/assets/icon.png`: our own mark, 1024×1024 with real alpha (drawn with ImageMagick: a branching tree — trunk, two branches, three nodes — in warm off-white on a forest-green rounded square). `.gitignore` admits that one file.
- `desktop/Cargo.toml`: package `arbos-desktop` (description, repository, homepage → Arbos); `[lib] name = "cydonia"` keeps every `use cydonia::…` and the `cydonia::` action namespaces working until the code-identifier rename.

## Attack ideas

1. `make bundle` on the Mac: `Arbos.app` with the icon in the Dock and in Finder; `Contents/MacOS/arbos` runs; `codesign --verify` if signing is in the pipeline.
2. Changing `CFBundleIdentifier` gives the app a new identity: macOS permission grants (Screen Recording for #16, Microphone for #51) and saved window frames under `app.cydonia` do not carry over. Expected; note for Jacob.
3. `cargo build -p arbos-desktop` and `cargo run --bin arbos-desktop` work; `-p cydonia` no longer does (scripts that used it: the parity launcher uses `cargo build` in the desktop dir — fine).
4. The driver's action names (`cydonia::NewSession`) are unchanged — QA scripts keep working.
5. Settings › About (or wherever `mark()` paints): the icon shows under `cargo run` now (before, a fresh clone had none until `make icon`).
6. Icon at 16 px and 32 px: the three nodes stay distinguishable? If not, a simplified small-size variant is a follow-up.
7. `www/` (the website) still says Cydonia in places — out of scope here; note.
