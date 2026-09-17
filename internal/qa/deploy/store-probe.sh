#!/usr/bin/env bash
# One client's view of the Project Agent Store, one line of JSON per pass, from any machine that mounts it.
#
# Why: the store answers per client (2026-09-17 05:35 UTC — one VM saw it empty and unwritable while two others
# read and wrote it). A mirror and its alarm run on one client and can only refuse or alarm on what that client
# sees. A second reader on another machine is the only way a per-client fault is visible at all. Two or more
# machines run this every 15 minutes; the QA cycle compares the newest rows and alarms when they disagree.
#
#   MACHINE=templar bash store-probe.sh            # appends to $LOCAL_LOG, and to the store when it can
#
# It never deletes anything, writes only under internal/qa/store-probes/, and keeps its own copy locally so a
# blackout is recorded even when the store cannot be written.
set -u
STORE="${ARBOS_QA_STORE_ROOT:-/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983}"
MACHINE="${MACHINE:-$(hostname -s 2>/dev/null || hostname)}"
LOCAL_LOG="${STORE_PROBE_LOG:-$HOME/store-probe-$MACHINE.jsonl}"
ts="$(date -u +%FT%TZ)"
mounted=false; docs=-1; docs_dir=false; notes=-1; bugs=-1; mirror_script=false; write_ok=false; write_err=""
if mount 2>/dev/null | grep -q " $(dirname "$STORE") " || [ -d "$STORE" ]; then mounted=true; fi
if [ -d "$STORE/docs" ]; then docs_dir=true; docs="$(ls "$STORE/docs" 2>/dev/null | grep -c '\.md$')"; fi
[ -f "$STORE/notes.md" ] && notes="$(stat -c %s "$STORE/notes.md" 2>/dev/null || echo -1)"
[ -d "$STORE/internal/qa/bugs" ] && bugs="$(ls "$STORE/internal/qa/bugs" 2>/dev/null | wc -l)"
[ -f "$STORE/internal/mirror-docs.sh" ] && mirror_script=true
probe="$STORE/internal/qa/store-probes/.write-probe-$MACHINE"
if mkdir -p "$STORE/internal/qa/store-probes" 2>/tmp/store-probe-err && echo "$ts" > "$probe" 2>>/tmp/store-probe-err && [ "$(cat "$probe" 2>/dev/null)" = "$ts" ]; then
  write_ok=true; rm -f "$probe"
else
  write_err="$(tr '\n' ' ' < /tmp/store-probe-err 2>/dev/null | cut -c1-160)"
fi
row="$(printf '{"ts":"%s","machine":"%s","mounted":%s,"docs_dir":%s,"docs_md":%s,"notes_bytes":%s,"bugs":%s,"mirror_script":%s,"write_ok":%s,"write_err":"%s"}' \
  "$ts" "$MACHINE" "$mounted" "$docs_dir" "$docs" "$notes" "$bugs" "$mirror_script" "$write_ok" "${write_err//\"/\\\"}")"
echo "$row" >> "$LOCAL_LOG"
# Into the store when it takes writes; otherwise the local copy is the record and the next sound pass carries it.
if [ "$write_ok" = true ]; then
  { cat "$LOCAL_LOG.pending" 2>/dev/null; echo "$row"; } >> "$STORE/internal/qa/store-probes/$MACHINE.jsonl" 2>/dev/null && rm -f "$LOCAL_LOG.pending"
else
  echo "$row" >> "$LOCAL_LOG.pending"
fi
echo "$row"
