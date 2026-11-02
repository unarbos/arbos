#!/usr/bin/env bash
# Mac-only self-test of the side-effect counter (../probe.c), built for macOS and injected into a
# small program instead of the app.
#   sidefx:  file, keychain, defaults, notification, Task and URLSession calls inside and outside
#            the phase markers 1..4 (the same dlsym hook the warm-up patch uses).
#   network: one raw connect() and one URLSession request with a header, to check the NET lines.
# Usage: probe/selftest/run.sh sidefx|network   (OUT=<dir> keeps the build; default is a temp dir)
set -euo pipefail
case ${1:-} in sidefx|network) ;; *) echo "usage: $0 sidefx|network" >&2; exit 2 ;; esac
here=$(cd "$(dirname "$0")" && pwd)
out=${OUT:-$(mktemp -d)}
mkdir -p "$out"
cp "$here/../gen.py" "$here/../probe.c" "$here/$1.swift" "$out/"
cd "$out"
: > empty.txt
python3 gen.py empty.txt . >/dev/null
xcrun clang -dynamiclib -arch arm64 -O1 probe.c hooks.s -o probe.dylib -undefined dynamic_lookup \
  -framework Foundation -lobjc 2>&1 | grep -v dynamic_lookup || true
xcrun swiftc -O -swift-version 5 "$1.swift" -o "$1"
start=$(date "+%Y-%m-%d %H:%M:%S")
DYLD_INSERT_LIBRARIES=$PWD/probe.dylib "./$1"
sleep 3
pred='subsystem == "com.unarbos.arbos.perfprobe"'
[ "$1" = network ] && pred="$pred AND eventMessage CONTAINS \"NET\""
log show --style compact --start "$start" --predicate "$pred" | cut -c1-400
