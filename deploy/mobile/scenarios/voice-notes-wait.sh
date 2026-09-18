#!/bin/bash
# Dictation puts words in the composer and sends nothing until he does.
#
#   voice-notes-wait.sh <cycle> [row] [kernel target]
#
# The row is what to tap in the projects list; the target is whose transcript
# is counted. They default to `phone` and `pod`, which are one kernel drawn
# twice (M-121) — tap any other row and you must name its target too, or the
# counts below describe a kernel the dictation never touched.
#
# The claim from cycle 44 is a correctness one, not a cosmetic one: the
# kernel's transcript must not move while dictation runs, and must move once
# on the send. Counted off the kernel rather than read off the screen,
# because "the words are in the field" and "nothing was sent" are different
# facts and only the second one is about the kernel.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
HERE=$(cd "$(dirname "$0")" && pwd)
CYCLE=${1:?cycle}; ROW=${2:-phone}; TARGET=${3:-pod}
OUT="$HOME/mobile-out/$CYCLE/voice-notes"; mkdir -p "$OUT"
UDID=$(xcrun simctl list devices booted -j | python3 -c 'import json,sys;print(next(d["udid"] for v in json.load(sys.stdin)["devices"].values() for d in v))')
B=com.unarbos.arbos.ios
. "$HERE/../sim-lib.sh"
ui() { python3 "$HERE/../ui.py" "$UDID" "$@"; }
total() { python3 "$HERE/../kernel.py" "$TARGET" total 2>/dev/null | tail -1; }
shot() { xcrun simctl io "$UDID" screenshot "$OUT/$1.png" >/dev/null 2>&1; }

xcrun simctl terminate "$UDID" $B 2>/dev/null; sleep 1
xcrun simctl launch "$UDID" $B -noAskNotifications 1 -dictateWav ~/mobile-clips/note.wav >/dev/null 2>&1

sleep 8
reach_the_list "$UDID" || exit 1
ui tap "$ROW" >/dev/null || { echo "no $ROW row"; exit 1; }
sleep 4

BEFORE=$(total)
echo "  kernel transcript before:      $BEFORE"
case $BEFORE in
  ''|*[!0-9]*) echo "  cannot count $TARGET's transcript — stopping rather than printing counts that mean nothing"; exit 1;;
esac

ui tap "Microphone" || { echo "no microphone button"; exit 1; }
sleep 3; shot 01-listening
sleep 8
ui tap "Stop" >/dev/null 2>&1 || echo "  no Stop button"
sleep 3; shot 02-words-waiting

HEARD=$(ui field plain 2>/dev/null)
echo "  the field holds:               ${HEARD:-nothing}"
# The claim is "dictation puts words in the composer", and until now this
# printed the words without ever looking at them: an empty field, or a
# sentence from some other clip, passed on the counts alone. Recognition is
# never exact — a synthesised voice saying "summarise" comes back
# "summaries" — so this asks how much of the sentence survived rather than
# demanding all of it, and names what differed.
WORDS=$(python3 - "$NOTE_SAYS" "$HEARD" <<'PY'
import re, sys
def words(s): return [w for w in re.findall(r"[a-z]+", s.lower())]
said, heard = words(sys.argv[1]), words(sys.argv[2] if len(sys.argv) > 2 else "")
kept = [w for w in said if w in heard]
missing = [w for w in said if w not in heard]
extra = [w for w in heard if w not in said]
pct = round(100 * len(kept) / len(said)) if said else 0
print(f"{pct}|{len(kept)}/{len(said)}|{' '.join(missing)}|{' '.join(extra)}")
PY
)
PCT=${WORDS%%|*}
REST=${WORDS#*|}
echo "  of the clip's words:           ${REST%%|*} kept (${PCT}%)"
MISSING=$(echo "$WORDS" | cut -d'|' -f3); EXTRA=$(echo "$WORDS" | cut -d'|' -f4)
[ -n "$MISSING" ] && echo "    not heard:                   $MISSING"
[ -n "$EXTRA" ] && echo "    heard instead:               $EXTRA"
echo "  the button beside it:          $(ui dump | awk '$2 > 740 && $2 < 830 && $3 == "Button" { print $4 }' | tr '\n' ' ')"

DURING=$(total)
echo "  kernel transcript, unsent:     $DURING"
[ "$DURING" = "$BEFORE" ] && echo "  nothing reached the kernel while it waited" \
                          || echo "  SOMETHING REACHED THE KERNEL BEFORE HE SENT"

ui tap "Send" >/dev/null || { echo "  no send button"; exit 1; }
sleep 12
AFTER=$(total)
echo "  kernel transcript after send:  $AFTER"
shot 03-sent
python3 - "$BEFORE" "$DURING" "$AFTER" "$PCT" <<'PY'
import sys
b, d, a = (int(x) if x.lstrip("-").isdigit() else None for x in sys.argv[1:4])
pct = int(sys.argv[4]) if sys.argv[4].isdigit() else 0
# Half the claim is about the kernel's counts and half is about the words.
# A run that waits and sends perfectly, with an empty composer, is not a
# pass — it is dictation that heard nothing, and the counts cannot see it.
words = "the words arrived" if pct >= 70 else f"but only {pct}% of the clip's words arrived"
if None in (b, d, a):
    print("  could not read all three counts")
elif d == b and a > b and pct >= 70:
    print(f"  VERDICT: waited, then went — {b} → {d} unsent → {a} on the send, and {words}")
elif d == b and a > b:
    print(f"  VERDICT: it waited and sent — {b} → {d} → {a} — {words}")
elif d != b:
    print(f"  VERDICT: it did not wait — {b} → {d} before he sent")
else:
    print(f"  VERDICT: the send did not land — {b} → {d} → {a}")
PY
