#!/usr/bin/env bash
# Mac-only SwiftUI behaviour checks behind the first-open warm-up's design. Each is a small
# macOS program (or a SIL dump) that shows one fact the warm-up depends on.
#   laziness:  which property wrappers run their initial value when a View struct is built and
#              mirrored, not rendered. Expected: ["init ObservedObject", "constructed + mirrored"].
#   hosting:   an off-screen hosting view and ImageRenderer both run body, onAppear, task and
#              onChange(initial:) - so rendering off screen is not side-effect free.
#   closures:  @State and @StateObject initial values are closures in SIL (lazy), for iOS 17 and
#              iOS 27 simulator targets. Expected lines for S._rows and S._rec.
#   typewalk:  walking a view's body types through generic containers without building values.
# Usage: checks/run.sh [laziness|hosting|closures|typewalk]...   (default: all)
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
out=${OUT:-$(mktemp -d)}
mkdir -p "$out"

exe() { xcrun swiftc -O -o "$out/$2" "$here/$1" && "$out/$2"; }

for c in ${@:-laziness hosting closures typewalk}; do
  echo "== $c"
  case $c in
    laziness)  exe state_laziness.swift laziness ;;
    hosting)   exe offscreen_hosting.swift hosting ;;
    typewalk)  exe type_walk.swift typewalk ;;
    closures)
      sdk=$(xcrun --sdk iphonesimulator --show-sdk-path)
      for t in arm64-apple-ios17.0-simulator arm64-apple-ios27.0-simulator; do
        echo "-- $t"
        xcrun swiftc -target $t -sdk "$sdk" -emit-silgen -parse-as-library "$here/state_closures.swift" |
          grep -E '^// .*closure.* in variable initialization expression of S\._(rows|rec)$'
      done ;;
    *) echo "unknown check $c" >&2; exit 2 ;;
  esac
done
