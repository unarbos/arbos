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
echo "tools the harness ships:"
for t in kernel.py ui.py journey-record.py find_row.py sim-lib.sh mirror-docs.sh; do
  if [ -f "$HERE/$t" ]; then
    printf "  %-20s %s\n" "$t" "$(md5sum "$HERE/$t" 2>/dev/null | cut -c1-8 || md5 -q "$HERE/$t" | cut -c1-8)"
  else
    printf "  %-20s MISSING FROM THE REPOSITORY\n" "$t"
  fi
done

[ -z "$HITS" ]
