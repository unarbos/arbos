#!/bin/bash
# Does the harness run the tools in this checkout, or copies in $HOME?
#
#   check-tools.sh            # from anywhere; exits non-zero if any script
#                             # reaches outside the repository for a tool
#
# The loop lost forty cycles of tooling once because it existed only in one
# machine's home directory (M-133). It committed the tools, and then went on
# *calling* the home copies anyway: cycle 63 found the journey running four
# tools from `$HOME` while their fixes sat in the repository unused (M-238),
# and cycle 93 found eleven more scripts doing the same thing thirty cycles
# later.
#
# Both are the same failure and neither is visible at runtime — the home
# copy usually works, and silently lags. The only way to keep a rented Mac
# replaceable is to make the reach-outside impossible to add without
# somebody noticing, which is what this is for.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)

echo "scripts under $HERE reaching outside the checkout for a tool:"
# A tool is a .py or .sh; ~/mobile-clips and ~/mobile-out are data and
# machine-specific by design, so they are not what this is about.
HITS=$(grep -rnE '(~|\$HOME)/[A-Za-z0-9_-]+\.(py|sh)' "$HERE" \
       --include='*.sh' --include='*.py' 2>/dev/null \
       | grep -v "check-tools.sh" || true)

if [ -z "$HITS" ]; then
  echo "  none — every tool resolves inside the repository"
else
  echo "$HITS" | sed 's/^/  /'
  echo
  echo "Each of these runs whatever happens to sit in that machine's home"
  echo "directory. Use \$HERE, so the tool beside the script is the tool that"
  echo "runs and a fresh clone behaves the same as this one."
fi

echo
# Second hygiene check, and it exists because the first sweep for it used a
# proxy. Cycle 116 looked for the string "Button +Back" in each file and
# called four scenarios clean; one of them only mentioned it inside an
# unrelated helper, and it had been unable to open its project for a week.
# A file mentioning a thing is not a file doing it, so this asks whether
# the step happens before the tap, in line order.
echo "scenarios that tap a project row after launch:"
LIST_FAULTS=$(for f in "$HERE"/scenarios/*.sh; do
  awk -v name="$(basename "$f")" '
    /simctl launch/ { launched = NR }
    /reach_the_list/ { reached = NR }
    /ui tap "\$ROW"/ { if (!tapped) tapped = NR }
    END {
      if (launched && tapped && (!reached || reached > tapped))
        printf "  %-34s taps at line %d with no reach_the_list before it\n", name, tapped
    }' "$f"
done)
if [ -z "$LIST_FAULTS" ]; then
  echo "  all of them reach the list first"
else
  echo "$LIST_FAULTS"
  echo
  echo "A cold start comes back to the chat that was in front, so these tap"
  echo "a name that may not be on screen. They do not fail when they run"
  echo "first, on a fresh install with no front project — which is why this"
  echo "is a check and not a memory."
fi

echo
echo "tools the harness ships:"
for t in kernel.py ui.py journey-record.py find_row.py sim-lib.sh mirror-docs.sh; do
  if [ -f "$HERE/$t" ]; then
    printf "  %-20s %s\n" "$t" "$(md5sum "$HERE/$t" 2>/dev/null | cut -c1-8 || md5 -q "$HERE/$t" | cut -c1-8)"
  else
    printf "  %-20s MISSING FROM THE REPOSITORY\n" "$t"
  fi
done

[ -z "$HITS" ] && [ -z "$LIST_FAULTS" ]
