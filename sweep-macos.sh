#!/bin/bash
# macOS sweep: one line per Arbos process — is the file it executes still the file at its start path, and which build is it?
#   GONE  = the inode lsof reports for the executable (the `txt` entry) is not the inode now at the start path (`stat -f %i`),
#           or the start path is missing. Identity, not existence: after an in-app update the path EXISTS and holds the new
#           build while the process runs the old one, so a "does the path exist" test says "not gone" exactly when it matters.
#   runs  = the build from the place's kernel.json whose pid is THIS pid (the record that names a live process, not the
#           newest file); "-" for a worker or hub, which write none. There is no /proc/<pid>/exe to execute on macOS.
# Run as the user who owns the kernels (lsof shows only your own processes without sudo).
# Untested on a Mac at the time of writing; the lsof -F parsing was checked on Linux against a replaced binary.
# Self-test on any Mac: cp /bin/sleep /tmp/t && /tmp/t 300 & ; cp /bin/sleep /tmp/t.new && mv -f /tmp/t.new /tmp/t ; the copy must read GONE.
for p in $(pgrep -x arbos-kernel; pgrep -x arbos-hub); do
  path=$(ps -o comm= -p $p)                        # on macOS: the full path the process was started from
  set -- $(ps -o args= -p $p); role=$2; place=$3
  run_ino=$(lsof -p $p -Ffti 2>/dev/null | awk '/^ftxt$/ {t=1; next} t && /^i/ {print substr($0,2); exit} /^f/ {t=0}')
  disk_ino=$(stat -f %i "$path" 2>/dev/null)
  if [ -z "$disk_ino" ]; then st=GONE; elif [ "$run_ino" != "$disk_ino" ]; then st=GONE; else st=ok; fi
  runs=-
  for j in "$place/.arbos/runtime/kernel.json" "$place/.arbos/kernel.json"; do
    [ -f "$j" ] || continue
    grep -q "\"pid\": *$p[,}]" "$j" && { runs=$(sed -n 's/.*"git_sha": *"\([0-9a-f]*\)".*/\1/p' "$j" | head -1); break; }
  done
  disk=$( [ -n "$disk_ino" ] && stat -f %Sm -t '%Y-%m-%d %H:%M' "$path" || echo missing)
  printf '%-4s pid %-8s since %-16s runs %-12s %s %s  path:%s  disk:%s\n' "$st" "$p" "$(ps -o lstart= -p $p | awk '{print $2,$3,$4}')" "${runs:--}" "$role" "$place" "$path" "$disk"
done
