#!/bin/bash
# sym.sh addrs.tsv > syms.tsv : path<TAB>load<TAB>addr -> addr<TAB>symbol, by atos per binary.
# System images come from the simulator's dyld shared cache, extracted under $DSC.
export DEVELOPER_DIR=/Applications/Xcode-27.0.0.app/Contents/Developer
DSC=~/arbos-perf-82aa-w/dsc
RR="/Library/Developer/CoreSimulator/Volumes/iOS_24A434/Library/Developer/CoreSimulator/Profiles/Runtimes/iOS 27.0.simruntime/Contents/Resources/RuntimeRoot"
cut -f1,2 "$1" | sort -u | while IFS=$'\t' read -r path load; do
  bin="$path"
  case "$path" in "$RR"/*) bin="$DSC/${path#"$RR"/}" ;; esac
  [ -e "$bin" ] || { echo "MISSING $bin" >&2; continue; }
  awk -F'\t' -v p="$path" '$1==p {print $3}' "$1" > /tmp/sym-addrs.txt
  nice -n 12 xcrun atos -arch arm64 -o "$bin" -l "$load" -f /tmp/sym-addrs.txt 2>/dev/null | paste /tmp/sym-addrs.txt -
done
