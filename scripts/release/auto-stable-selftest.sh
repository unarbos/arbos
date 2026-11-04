#!/usr/bin/env bash
# auto-stable-selftest.sh — how auto-stable.sh reads scripts/release/mac-change.sh.
# No network, no credentials, no git, no release. Run it: scripts/release/auto-stable-selftest.sh
#
# WHAT THIS IS FOR, because it reads like a test of exit codes and is about what ships.
#
# mac-change.sh is a three-way instrument. It answers 0 (no Mac change) or 1 (the Mac app must be
# released), and any other code is the script failing rather than answering — 127 when it or
# something it needs is not there, 126 when it is not executable, 128 out of git. auto-stable.sh
# used to read it two ways: 0 was no release and *every* other code was a release. So a predicate
# that crashed was recorded as the verdict "Mac release needed".
#
# That erred towards publishing, which is why nothing ever shipped wrong from it. But the
# direction was an accident of which branch the unhandled code fell into, not a property anyone
# chose, and the same conflation read the other way publishes nothing and says nothing.
#
# The cases below are run twice: once against mac_verdict, and once against the two-way read put
# back. The first run says the hardening works. The second says the cases can tell the difference
# — which the first run cannot say on its own, and "the predicate did not answer" is exactly the
# kind of case whose absence nobody would notice, because it never fires in a healthy run.
set -u
D=$(cd "$(dirname "$0")" && pwd)

# The real file, sourced for its functions. ARBOS_AUTO_STABLE_LIB stops it before the release
# itself, so what runs below is the shipping mac_verdict and not a copy that could drift from it.
ARBOS_AUTO_STABLE_LIB=1
# shellcheck source=scripts/release/auto-stable.sh
source "$D/auto-stable.sh"
# auto-stable.sh turns on `set -e`, which would end this file at its first failing case.
set +e

fail=0
ok() { printf '  ok    %s\n' "$1"; }
bad() {
  printf '  FAIL  %s\n' "$1"
  fail=$((fail + 1))
}

# A mac-change.sh whose exit code is dictated by the case. auto-stable.sh finds the predicate at
# "$here/mac-change.sh", so pointing `here` at this directory is the seam — nothing is patched and
# no call is intercepted.
stub=$(mktemp -d) || exit 1
empty=$(mktemp -d) || exit 1
trap 'rm -rf "$stub" "$empty"' EXIT
cat >"$stub/mac-change.sh" <<'STUB'
#!/usr/bin/env bash
printf '%s\n' "${STUB_SAY:-}"
[ -n "${STUB_ERR:-}" ] && printf '%s\n' "$STUB_ERR" >&2
exit "${STUB_RC:?the case says what the predicate does}"
STUB
chmod +x "$stub/mac-change.sh"

BASE=1111111111111111111111111111111111111111
TARGET=2222222222222222222222222222222222222222

# One reading, as "<exit code>|<what it printed>". The implementation is an argument so the same
# table can be run against the restored defect further down.
reads() { # <fn> <what the predicate exits> <what the predicate prints>
  local fn=$1 rc=0 out
  out=$(STUB_RC="$2" STUB_SAY="$3" "$fn" "$BASE" "$TARGET") || rc=$?
  printf '%s|%s' "$rc" "$out"
}

# want_rc 0 = no Mac change, 1 = release the Mac app, 2 = undecided, the predicate did not answer.
#
# What the predicate prints and what the reading must say are separate columns, and that is not
# tidiness. For 0 and 1 they are the same string because passing the predicate's own sentence
# through is the thing being tested — the release notes are written from it. For the undecided
# rows they must differ: if the required phrase were something the predicate itself printed, the
# case would pass on the predicate's words being echoed back, which is what a mac_verdict that
# had stopped classifying would also do. The phrase is therefore a sentence only mac_verdict
# writes.
CASES='0|0|nothing on the Mac paths moved|nothing on the Mac paths moved|no Mac change is read as no Mac change
1|1|Mac sources changed: crates/arbos-mac|Mac sources changed: crates/arbos-mac|a Mac change is read as a Mac change
2|2|usage: mac-change.sh <base> <target>|mac-change.sh exited 2|a bad argument is undecided, not a release
126|2|bash: Permission denied|mac-change.sh exited 126|a predicate that is not executable is undecided, not a release
127|2|bash: No such file or directory|mac-change.sh exited 127|a predicate that is not there is undecided, not a release
128|2|fatal: bad object HEAD|mac-change.sh exited 128|a git failure inside the predicate is undecided, not a release'

echo "mac_verdict — what each exit code of mac-change.sh is classified as:"
here=$stub
while IFS='|' read -r rc want says phrase name; do
  got=$(reads mac_verdict "$rc" "$says")
  if [[ "${got%%|*}" == "$want" && "${got#*|}" == *"$phrase"* ]]; then
    ok "$name"
  else
    bad "$name: wanted exit $want saying '$phrase', got '$got'"
  fi
done <<<"$CASES"

# The undecided message is what an operator acts on, so it has to say which run it came from. An
# exit code alone would not: 2 is shared by every row above except the first two.
echo
echo "mac_verdict — an undecided reading says enough to act on:"
got=$(reads mac_verdict 127 "")
for want in "exited 127" "11111111..22222222" "no output"; do
  if [[ "${got#*|}" == *"$want"* ]]; then ok "names '$want'"; else bad "does not name '$want': $got"; fi
done

# The reason for a 0 or a 1 is written into the plan's `notes=` line, which the merge kit greps
# back out. So it is the predicate's stdout and nothing else: a git `fatal:` on stderr belongs in
# an undecided message, not in a release note, and a second line there would put a stray line into
# key=value output. An undecided reading has no note to write and stderr is usually the only thing
# that says what went wrong, so there it is kept.
echo
echo "mac_verdict — stdout is the reason, stderr is not:"
for rc in 0 1; do
  got=$(STUB_ERR="warning: refname is ambiguous" reads mac_verdict "$rc" "no Mac path changed")
  if [[ "${got#*|}" == "no Mac path changed" ]]; then
    ok "exit $rc keeps the note clean"
  else
    bad "exit $rc let stderr into the release note: '${got#*|}'"
  fi
done
got=$(STUB_ERR="fatal: bad object HEAD" reads mac_verdict 128 "")
if [[ "${got#*|}" == *"fatal: bad object HEAD"* ]]; then
  ok "an undecided reading keeps stderr, which is where the reason is"
else
  bad "an undecided reading dropped stderr: '${got#*|}'"
fi

# Not a stub: bash's own 127 for a predicate that is not on disk, which is the live case. The gate
# lane hit it on 2026-11-04 running mac-change.sh in a checkout without the files it reads, and a
# person had to decide by hand that it was not a verdict.
#
# A failing predicate says why on stderr, where the old read was not looking — so the reading has
# to carry bash's own words and not just the number.
echo
echo "mac_verdict — a predicate that is genuinely absent:"
here=$empty
rc=0
out=$(mac_verdict "$BASE" "$TARGET" 2>/dev/null) || rc=$?
if [[ "$rc" == 2 ]]; then ok "exit 2, undecided"; else bad "wanted exit 2, got $rc: $out"; fi
if [[ "$out" == *"exited 127"* ]]; then ok "says the predicate exited 127"; else bad "does not name the exit code: $out"; fi
if [[ "$out" == *"No such file or directory"* ]]; then ok "carries what the shell said on stderr"; else bad "the stderr diagnostic was dropped: $out"; fi
here=$stub

# ── The defect, restored ────────────────────────────────────────────────────────────────────────
# The call site as it stood before this change: capture stdout, 0 is no release, anything else is
# a release. It is a copy, which is normally the thing to avoid in a self-test — but it is a copy
# of code that is no longer in the tree, and nothing that no longer exists can drift away from it.
two_way() {
  local out rc=0
  out="$("$here/mac-change.sh" "$1" "$2")" || rc=$?
  printf '%s\n' "$out"
  [ "$rc" = 0 ] && return 0
  return 1
}

# Both halves of the proof come out of running the same table against it:
#   the 0 and 1 rows must still PASS — the fix changes nothing about the answers it does give;
#   every other row must FAIL — those are the rows that catch the defect, and a row that passes
#   here is a row that would pass in the broken state too, which is a row worth nothing.
echo
echo "the two-way read, restored — the 0 and 1 rows hold, the rest must break:"
while IFS='|' read -r rc want says phrase name; do
  got=$(reads two_way "$rc" "$says")
  passes=false
  [[ "${got%%|*}" == "$want" && "${got#*|}" == *"$phrase"* ]] && passes=true
  if [[ "$want" == 2 ]]; then
    if $passes; then
      bad "$name — this case passes against the defect, so it cannot fail"
    else
      ok "$name — caught (the old read called it exit ${got%%|*})"
    fi
  else
    if $passes; then
      ok "$name — unchanged, as it must be"
    else
      bad "$name — the fix changed a reading it had no business changing: got '$got'"
    fi
  fi
done <<<"$CASES"

# ── The other half: an undecided reading has to be loud ─────────────────────────────────────────
# Classifying correctly buys nothing if the caller then swallows it. An undecided arm that called
# emit_none would skip the release and print "nothing to do", which is the silent direction and
# reads exactly like a healthy no-change run. That arm cannot be exercised here — running it means
# running a release — so it is read, the way scripts/merge/selftest.sh reads the verdicts it
# cannot run, and for the same reason.
#
# Comments are stripped first. The comment on that arm necessarily names emit_none, since it is
# there to say why emit_none is not, so a grep over the raw text would match the explanation of an
# arm that had been deleted.
echo
echo "the call site's undecided arm:"
arm=$(sed -n '/^  case "\$mac_rc" in/,/^  esac/p' "$D/auto-stable.sh" |
  grep -vE '^[[:space:]]*#' | sed -n '/^[[:space:]]*\*)/,$p')
if [[ -n "$arm" ]]; then ok "exists"; else bad "there is no \`*)\` arm on the mac_rc case at all"; fi
if [[ "$arm" == *"exit 2"* ]]; then ok "exits 2, so the Release run goes red"; else bad "does not exit 2: $arm"; fi
if [[ "$arm" == *"::error"* ]]; then ok "says so where GitHub shows it"; else bad "prints no ::error: $arm"; fi
if [[ "$arm" != *emit_none* ]]; then ok "does not emit_none, which would skip the release quietly"; else bad "calls emit_none: $arm"; fi

echo
if ((fail == 0)); then
  echo "auto-stable-selftest: all cases pass, and every undecided case fails against the defect"
else
  echo "auto-stable-selftest: $fail failure(s)"
fi
exit $((fail > 0))
