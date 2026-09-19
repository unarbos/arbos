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
# Two shapes, not one. Running `~/tool.sh` is the obvious reach; `cd
# ~/some-checkout` is the same fault wearing a coat, and it is how
# poll-feedback.sh came to run its poller from a second clone in $HOME.
# The exemptions end at a boundary, and the boundary is "not a name
# character" rather than "a slash": `cd ~/arbos &&` ends in a space. Without
# any boundary `arbos` exempted `arbos-tools`, the one thing this was
# widened to catch, and the report went clean with the probe still sitting
# in the directory. Both mistakes were made here, in that order.
#
# `~/arbos` is exempt because it *is* the checkout on the Mac, which is a
# different thing from a second clone beside it. Home directories holding
# data rather than code — mobile-out, the clips, the docs mirror, the vault
# file — are where output belongs, and are named here so the check stays
# about tools.
HITS=$(grep -rnE '(~|\$HOME)/[A-Za-z0-9_-]+\.(py|sh)|cd +"?(~|\$HOME)/' "$HERE" \
       --include='*.sh' --include='*.py' 2>/dev/null \
       | grep -vE "check-tools.sh|(~|\\\$HOME)/(arbos|mobile-out|mobile-clips|mobile-docs|mobile-refs|mobile-bundles|mobile-feedback|mobile-derived|\.op-env)([^A-Za-z0-9_-]|$)" || true)

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
echo "scenarios that need the projects list after launch:"
# The journey is a scenario in everything but its folder, and it had the
# same fault: J1, "open the project from the list", tapped the chat's own
# header at y=85 and passed because the chat it woke in was the target.
LIST_FAULTS=$(for f in "$HERE"/scenarios/*.sh "$HERE"/mac-journey.sh; do
  awk -v name="$(basename "$f")" '
    /simctl launch/ { launched = NR }
    # The first one, not the last. mac-journey reaches the list twice — at
    # J1 and again after J6k relaunch — and taking the later one made
    # "reached after used" true for a file that reaches it first.
    /reach_the_list/ { if (!reached) reached = NR }
    # Tapping a row needs the list. So does reading one, which is how
    # list-composer slipped past the first version of this: it never taps,
    # it only counts rows and reads the placeholder, and it was
    # inconclusive in every sweep for a week.
    #
    # Only lines in the main flow count. A helper that greps for rows is a
    # definition, not a use, and counting those flagged four files that were
    # already correct — the same mistake as the first version, from the
    # other side.
    # A scenario whose subject *is* the landing must not be sent to the
    # list first. It says so in a line of its own, and is then its own
    # business — one declared exception beats a rule nobody can satisfy.
    /# reaches-the-list: not before the landing is measured/ { exempt = 1 }
    # A one-line helper — `score() { ...; }` — opens a brace and closes it on
    # the same line. Treating that as entering a function left `infn` set for
    # the rest of the file, so every use after the first helper was ignored:
    # the check passed `mac-journey.sh` with its reach-the-list step deleted.
    # Proven by deleting it and watching the check stay silent.
    # A function definition is a definition whether or not it fits on one
    # line. Skip the line either way; only a multi-line one puts us inside a
    # body. Getting this wrong in both directions is how the check first
    # passed `mac-journey.sh` with its step deleted (one-line helper left
    # `infn` set for the whole file) and then flagged four files that were
    # already correct: a grep inside a helper counted as a use.
    /^[a-zA-Z_][a-zA-Z0-9_]*\(\) *\{/ { if (!/\}/) infn = 1; next }
    infn && /^\}/ { infn = 0; next }
    !infn && /ui tap "\$ROW"|Button \+\[a-z|, \(Idle\|Working\)/ { if (!used) used = NR }
    END {
      if (!exempt && launched && used && (!reached || reached > used))
        printf "  %-34s needs the list at line %d, with no reach_the_list before it\n", name, used
    }' "$f"
done)
if [ -z "$LIST_FAULTS" ]; then
  echo "  all of them reach the list first"
  echo "  (first use only — a scenario that goes back to the list halfway"
  echo "   through is not covered here; several-workers failed that way at"
  echo "   cycle 133 with this check silent)"
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

# A scenario can call a shared helper it never sourced. The call reads fine,
# the check above sees the words and passes it, and at runtime the shell says
# "command not found", the helper returns 127, and a `|| exit 1` beside it
# ends the run before it measures anything. Cycle 167 nearly shipped exactly
# that: list-rows.sh gained a reach_the_list call and had no sim-lib line.
echo
echo "scenarios calling a shared helper without sourcing sim-lib:"
HELPERS=$(grep -oE "^[a-z_]+\(\)" "$HERE/sim-lib.sh" | tr -d '()' | tr '\n' '|' | sed 's/|$//')
ORPHANS=0
for f in "$HERE"/scenarios/*.sh "$HERE"/mac-*.sh; do
  [ -f "$f" ] || continue
  grep -q "sim-lib.sh" "$f" && continue
  # A call, not the word. "pt" appears in a comment about pt coordinates in
  # two of these files, and matching bare words reported them as calling a
  # helper they only mentioned.
  USED=$(grep -vE "^[[:space:]]*#" "$f" \
         | grep -oE "(^|[;&|(]|\\\$\()[[:space:]]*($HELPERS)[[:space:]]" \
         | grep -oE "($HELPERS)" | sort -u | tr '\n' ' ')
  [ -n "$USED" ] || continue
  echo "  $(basename "$f") calls: $USED"
  ORPHANS=$((ORPHANS + 1))
done
[ "$ORPHANS" = 0 ] && echo "  none — every helper called is one the file has sourced"

# A build path outside the loop's own derived data. Three scenarios
# reinstalled the app from /tmp/dd, a directory some cycle left behind, and
# the build in it was a day old — so every scenario that ran after one of them
# measured yesterday's app. That is where the separator six cycles chased was
# coming from.
echo
echo "scenarios installing an app from outside the loop's derived data:"
STRAYAPP=0
for f in "$HERE"/scenarios/*.sh "$HERE"/mac-*.sh; do
  [ -f "$f" ] || continue
  # A literal root only. mac-cycle.sh writes "$DERIVED/Build/Products/..."
  # and matching the tail of that reported the one file doing it correctly.
  HIT=$(grep -vE "^[[:space:]]*#" "$f" \
        | grep -oE "(/tmp|/Users|/var|/private)/[A-Za-z0-9_./-]*Build/Products/[A-Za-z0-9_./-]*" \
        | grep -v "mobile-derived" | sort -u | tr '\n' ' ')
  [ -n "$HIT" ] || continue
  echo "  $(basename "$f") installs from: $HIT"
  STRAYAPP=$((STRAYAPP + 1))
done
[ "$STRAYAPP" = 0 ] && echo "  none — every reinstall uses the build this loop just made"

# Tools that reach for a Homebrew program without asking for Homebrew's
# directory. The loop drives this Mac over ssh, and a non-login ssh shell has
# a bare PATH: /usr/bin and no more. So `ffmpeg` and `idb` are invisible, and
# the tool reports the machine lacks a program the machine has. Cycle 160 hit
# this with Python, fixed that one tool, and the lesson did not travel — at
# cycle 192 review-demo.sh said "no ffmpeg on this machine" while ffmpeg sat
# in /opt/homebrew/bin. This is the lesson written down where it applies.
echo
echo "tools calling a Homebrew program without Homebrew on PATH:"
STRAYPATH=0
for f in $(find "$HERE" -name "*.sh" | sort); do
  USES=$(grep -oE "(^|[^-a-zA-Z_./])(ffmpeg|ffprobe|idb)[ \"']" "$f" 2>/dev/null \
    | grep -oE "ffmpeg|ffprobe|idb" | sort -u | tr '\n' ' ')
  [ -n "$USES" ] || continue
  grep -q 'PATH="/opt/homebrew/bin' "$f" && continue
  # Sourcing a library that sets the PATH counts: the program is found by the
  # time it is called, which is the only thing that matters.
  grep -qE '\. .*sim-lib\.sh' "$f" && continue
  echo "  $(basename "$f") calls: ${USES}— add export PATH=\"/opt/homebrew/bin:\$PATH\""
  STRAYPATH=$((STRAYPATH + 1))
done
[ "$STRAYPATH" = 0 ] && echo "  none — every tool that calls one asks for its directory first"

# The same question of the Python tools, because the check above reads *.sh
# and cycle 193 tripped over ui.py for exactly that reason: it ran `idb` by
# bare name, inherited a bare ssh PATH, and raised FileNotFoundError blaming
# the program rather than the PATH. A Python tool cannot fix this with an
# export — it is one process — so what is wanted is that it *finds* the
# program: shutil.which, or an absolute path, not a bare name in the argv.
echo
echo "python tools running a Homebrew program by bare name:"
STRAYPY=0
for f in $(find "$HERE" -name "*.py" | sort); do
  BARE=$(grep -oE '\["(ffmpeg|ffprobe|idb)"' "$f" 2>/dev/null | tr -d '["' | sort -u | tr '\n' ' ')
  [ -n "$BARE" ] || continue
  echo "  $(basename "$f") runs: ${BARE}by bare name — resolve it with shutil.which first"
  STRAYPY=$((STRAYPY + 1))
done
[ "$STRAYPY" = 0 ] && echo "  none — every one resolves the program before running it"
