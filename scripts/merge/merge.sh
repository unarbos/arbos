#!/bin/bash
# merge.sh <pr> <pinned-head-sha> [accept-red=<run-id>[,<run-id>]]
# Runs a fresh GO and, only on GO, merges <pr> as a merge commit pinned to <pinned-head-sha>.
# Refuses if main's head moves between the GO check and the merge call, and names each commit that landed.
# The PAT lives in one shell variable of this process (sent to curl over stdin, never argv or a file)
# and is unset as soon as the merge call returns.
# Run from the repo checkout. MERGE_KIT (default /tmp/k) holds this session's files: merge-allow (the PR numbers
# the coordinator cleared, `#` starts a comment), watch-solo (merge-window.sh) and op-last.err.
#
# EXIT CODES, and why there are four of them rather than three.
#   2  REFUSED  — a policy answer about this pull request. The kit read the coordinator's list and the PR's
#                 state, and the answer is no.
#   3  NOT MERGED — a verdict answer. The window check said NO-GO, or main moved out from under the GO.
#   4  credential refusal — 1Password was reached and did not serve the read.
#   5  SETUP — the kit or its tools are not installed on this host. NOTHING WAS JUDGED. This code exists
#                 because the other three were being used for it: a missing /tmp/k reported as "no merge list"
#                 (indistinguishable from "the coordinator did not clear you"), and a missing `op` binary
#                 reported as a PAT read failure after a clean GO. On 2026-11-05 a lane read a setup gap as a
#                 refusal, fell back to `gh pr merge`, and landed a merge inside a closed release window.
#                 A setup gap must never be reportable as a policy refusal or a credential refusal.
set -u
D=$(cd "$(dirname "$0")" && pwd); K=${MERGE_KIT:-/tmp/k}
source "$D/opread.sh"
source "$D/checks.sh"

# --- setup preflight: everything that is MISSING rather than refused, named before anything is judged ------
# All of it runs before the first API call, so a host with no tooling cannot reach a gate, a verdict or the
# vault and so cannot dress a setup gap as one of their answers. Every finding is reported in one pass, so
# one run names the whole gap instead of one layer of it per run.
setup=0
if [ ! -d "$K" ]; then
  echo "SETUP: the merge kit directory $K does not exist, so there is no allow-list to read."
  echo "  This is a MISSING DIRECTORY, not a policy answer: the coordinator's clearance for any pull request"
  echo "  has neither been granted nor withheld, because nothing was read. Create the kit and ask the"
  echo "  coordinator for the list; do not write your own clearance into it."
  echo "    mkdir -p $K   (or point MERGE_KIT at the kit this session already has)"
  setup=1
elif [ ! -r "$K/merge-allow" ]; then
  echo "SETUP: the merge kit directory $K exists but holds no readable allow-list at $K/merge-allow."
  echo "  This is a MISSING FILE, not a policy answer: nothing has been read, so no pull request has been"
  echo "  cleared or declined. Ask the coordinator which pull requests are cleared and write what they say."
  setup=1
fi
op_preflight || setup=1
[ $setup -eq 0 ] || { echo "  Nothing was judged and nothing was merged. Fix the above, then run this command again."; exit 5; }
# --- end setup preflight -----------------------------------------------------------------------------------

[ $# -ge 2 ] || { echo "REFUSED: usage: merge.sh <pr> <pinned-head-sha> [accept-red=<id>[,<id>]] [accept-pending=<id>[,<id>]] [over-serialize=<fid>@<head8>]"; exit 2; }
ALLOW=$(sed 's/#.*//' "$K/merge-allow" | tr -s ' \t\n' ' ' | sed 's/^ //; s/ $//')
PR=$1; SHA=$2; FLAKE=""; OVER=""; PEND=""
for a in "${@:3}"; do case "$a" in accept-red=*) FLAKE=$a ;; over-serialize=*) OVER=${a#over-serialize=} ;; accept-pending=*) PEND=${a#accept-pending=} ;; *) echo "REFUSED: unknown argument $a"; exit 2;; esac; done
R=unarbos/arbos
# The allow-list is present and readable by here, so every answer below is a policy answer and says so. The
# empty list and the list that names somebody else are different sentences because they call for different
# actions, and neither is the missing-directory sentence above.
case " $ALLOW " in
  *" $PR "*) ;;
  *) if [ -z "$ALLOW" ]; then
       echo "REFUSED: $K/merge-allow is present and names no pull request at all, so the coordinator has cleared none."
     else
       echo "REFUSED: #$PR is not in the coordinator's merge list ($ALLOW)."
     fi
     echo "  This is a POLICY answer and not a setup gap: the allow-list was read from $K/merge-allow."
     echo "  Ask the coordinator to clear #$PR. Do not add yourself to the list."
     exit 2 ;;
esac

# main's head, read live from GitHub (never the local origin/main, which another fetch can move mid-run).
main_head() { local o; o=$(git ls-remote origin refs/heads/main 2>&1) || { echo "git ls-remote failed: ${o:0:160}"; return; }; echo "$o" | awk '$2=="refs/heads/main"{print $1}'; }
# main_moved <old> <new>: main's first-parent commits after <old>, newest first, each with what landed it.
# Every lane merges as the same GitHub account, so "merged by" never tells lanes apart: the head branch's
# suffix (cursor/<name>-<suffix>) is the only lane identifier in main's history.
main_moved() {
  local cur=$2 n=0 c sha when author parent subj pr via
  while [ "$cur" != "$1" ]; do
    [ $n -lt 20 ] || { echo "    … stopped after 20 commits without reaching ${1:0:9}"; return; }
    c=$(gh api "repos/$R/commits/$cur" --jq '[.sha, .commit.committer.date, (.author.login // .commit.author.name), (.parents[0].sha // "-"), (.commit.message | split("\n")[0])] | @tsv' 2>&1) \
      || { echo "    ${cur:0:9} (cannot read this commit: ${c:0:160})"; return; }
    IFS=$'\t' read -r sha when author parent subj <<< "$c"
    pr=$(echo "$subj" | sed -nE 's/.*#([0-9]+).*/\1/p' | head -1)
    [ -n "$pr" ] || pr=$(gh api "repos/$R/commits/$sha/pulls" --jq '.[0].number // empty' 2>/dev/null)
    via=$([ -n "$pr" ] && gh api "repos/$R/pulls/$pr" --jq '"PR #\(.number) from \(.head.ref), merged by \(.merged_by.login // "nobody (not merged through the PR)")"' 2>/dev/null)
    echo "    ${sha:0:9} $when ${via:-no PR, pushed or merged directly by $author}: $subj"
    n=$((n+1)); cur=$parent
    [ "$cur" != "-" ] || { echo "    … reached the root without meeting ${1:0:9}: main was rewritten"; return; }
  done
}
# main_refuse <old> <new> <when>: the refusal names every commit the GO verdict did not judge.
main_refuse() {
  echo "NOT MERGED: main moved $3 — GO judged main at ${1:0:9}, main is now ${2:0:9}. Not judged by that GO:"
  main_moved "$1" "$2"
  echo "  Run merge.sh again for a fresh GO."
}
[ ${#SHA} -eq 40 ] || { echo "REFUSED: pin the full 40-char head sha"; exit 2; }

head=$(gh pr view "$PR" -R $R --json headRefOid --jq .headRefOid)
[ "$head" = "$SHA" ] || { echo "REFUSED: #$PR head is $head, not the pinned $SHA"; exit 2; }
base=$(gh pr view "$PR" -R $R --json baseRefName --jq .baseRefName)
[ "$base" = main ] || { echo "REFUSED: #$PR targets '$base', not main (a stacked PR is retargeted only after its parent lands)"; exit 2; }
# Every check must be terminal: a running Swift/xcframework lane cannot be inferred from mergeStateStatus.
# accept-pending=<run-id>[,<run-id>]: a coordinator ruling for ONE merge. Each pending check must be named by its run id;
# any other pending check, or one that cannot be traced to a run, still refuses.
pend=$(gh pr view "$PR" -R $R --json statusCheckRollup --jq '.statusCheckRollup[] | select((.status // "COMPLETED") != "COMPLETED" or (.state // "") == "PENDING" or (.state // "") == "EXPECTED")
  | "\(((.detailsUrl // .targetUrl // "") | [capture("runs/(?<id>[0-9]+)").id] | first) // "untraceable") \(.name // .context)"') \
  || { echo "REFUSED: cannot read the check rollup"; exit 2; }
pacc=",$PEND,"
while read -r id name; do
  [ -n "$id" ] || continue
  case "$pacc" in *",$id,"*) echo "    pending check accepted by run id (coordinator ruling, this merge only): $id $name" ;;
    *) echo "REFUSED: checks not terminal yet: run $id $name"; exit 2 ;; esac
done <<< "$pend"

# Every gate in checks.sh that applies to this head must be present and successful ON this head.
# This runs before the GO because it is cheap and decisive, and there is no argument that waives it:
# accept-red names a run whose result is to be ignored, and a gate with no result has no run to name.
# That is deliberate. Rule 9 of the merge procedure already allows a red check only when "a run on the
# identical head sha is green for that same check", which is exactly what PASS below means, so the
# legitimate flake case needs no override and the illegitimate one must not have one.
files=$(pr_files "$PR") || { echo "REFUSED: cannot read #$PR's changed files: ${files#BLOCKED:}"; exit 2; }
[ -n "$files" ] || { echo "REFUSED: #$PR reports no changed files, so no gate can be judged"; exit 2; }
require_gates "$SHA" "$files"; grc=$?
case $grc in
  1) echo "REFUSED: a required check has no successful run on the pinned head $SHA."
     echo "  A skipped, cancelled, red or absent gate is not a pass. Get the gate green on this exact sha, then merge."; exit 2 ;;
  2) echo "REFUSED: a required check could not be read for $SHA, so it is not inferred."; exit 2 ;;
  # accept-pending= waives a pending check in the rollup; it does not reach a required gate. A gate
  # still queued on a saturated macOS pool has no verdict, and waiting is the whole point of a gate.
  3) echo "REFUSED: a required check is still running on $SHA. Wait for it; accept-pending does not cover a required gate."; exit 2 ;;
esac
# Non-success results that are not required are shown rather than silently dropped, because the bug
# this block replaces was precisely that they printed nothing at all.
skipped=$(gh pr view "$PR" -R $R --json statusCheckRollup --jq '.statusCheckRollup[]
  | select((.conclusion // .state) as $c | ["SKIPPED","NEUTRAL"] | index($c)) | "      \(.name // .context) \(.conclusion // .state)"' | sort -u)
[ -n "$skipped" ] && { echo "    not a verdict either way, and not required on this head:"; echo "$skipped"; }

main0=$(main_head)
[ ${#main0} -eq 40 ] || { echo "REFUSED: cannot read main's head before the GO check: ${main0:-empty answer}"; exit 2; }
echo "    main before the GO check: ${main0:0:9}"
out=$(bash "$D/merge-window.sh" go "$PR" 2>&1); mwrc=$?
echo "$out" | grep -vE '^\s+(release:|dmg|feed \(|latest release|download/mac)'
# Exit 5 from the window check is its own setup refusal (see merge-window.sh). Without this arm it arrives
# here as a missing VERDICT line and is reported as "verdict is not GO" — a setup gap read as a verdict.
[ $mwrc -ne 5 ] || { echo "SETUP: the window check could not run on this host, for the reason it gave above. Nothing was judged and nothing was merged."; exit 5; }
v=$(echo "$out" | grep '^VERDICT' | tail -1)
# over-serialize=<fid>@<head8>: a coordinator ruling for one merge. It lifts exactly one NO-GO reason, and only
# when the sole releasable loop item is that item, at that head, still under a serialize-on-release flag.
if [ -n "$OVER" ] && [[ "$v" == *": NO-GO — a loop item the timer can release is gating" ]]; then
  rel=$(echo "$out" | grep -E '^\s+RELEASABLE ')
  [ "$(echo "$rel" | wc -l)" -eq 1 ] && [[ "$(echo "$rel" | awk '{print $2"@"$4" "$5}')" == "${OVER} serialize-on-release:"* ]] \
    || { echo "NOT MERGED: override ${OVER} does not match the releasable item(s): $rel"; exit 3; }
  echo "    NO-GO lifted by coordinator ruling for this merge only: $OVER under serialize-on-release"
else
  case "$v" in *": GO —"*) ;; *) echo "NOT MERGED: verdict is not GO"; exit 3;; esac
fi
# A second pass, over every other check the rollup reports. The required gates above decide whether the
# checks that matter ran and passed; this one catches a red result in a check nobody thought to name,
# and accept-red=<id>[,<id>] can waive it (rule 9 flake, or a run that did not test this PR).
# It cannot waive a required gate: that refusal already happened, above, and took no arguments.
red=$(gh pr view "$PR" -R $R --json statusCheckRollup --jq '.statusCheckRollup[]
  | select((.conclusion // .state) as $c | ["FAILURE","CANCELLED","TIMED_OUT","ACTION_REQUIRED","STARTUP_FAILURE","ERROR","STALE"] | index($c))
  | "\(((.detailsUrl // .targetUrl // "") | [capture("runs/(?<id>[0-9]+)").id] | first) // "untraceable") \(.name // .context) \(.conclusion // .state)"') || { echo "NOT MERGED: cannot read the check rollup"; exit 3; }
acc=",${FLAKE#accept-red=},"
while read -r id name; do
  [ -n "$id" ] || continue
  case "$acc" in *",$id,"*) echo "    red check accepted by run id: $id $name" ;;
    *) echo "NOT MERGED: red check not accepted: run $id $name"; exit 3 ;; esac
done <<< "$red"
head=$(gh pr view "$PR" -R $R --json headRefOid --jq .headRefOid)
[ "$head" = "$SHA" ] || { echo "NOT MERGED: head moved to $head during GO"; exit 3; }
main1=$(main_head)
[ ${#main1} -eq 40 ] || { echo "NOT MERGED: cannot re-read main's head after the GO check: ${main1:-empty answer}"; exit 3; }
[ "$main1" = "$main0" ] || { main_refuse "$main0" "$main1" "during the GO check"; exit 3; }

rl0=$(oprl)
TOKEN=$(opread 'op://Arbos/gefompyaklwbv2iuyyb4z5rvbm/credential') && [ -n "$TOKEN" ] \
  || { echo "NOT MERGED: PAT read failed at $(date -u +%FT%TZ). op is installed and the token is in the environment"
       echo "  (both checked in the preflight above), and the cache is off, so this reached 1Password and is a real"
       echo "  credential answer rather than a setup gap. Refused requests are not counted against the budget."
       echo "  op said: $(head -c 300 "$(op_err_file)")"; exit 4; }
rl1=$(oprl)
echo "PAT read $(date -u +%FT%TZ): served by 1Password with the cache off. The client cannot see its cost in account requests; the account counter (shared by every lane) went from $rl0 to $rl1 across the read."
body=$(printf '{"sha":"%s","merge_method":"merge"}' "$SHA")
hdr=$(mktemp); chmod 600 "$hdr"
# The PR merge API pins the PR head but not main, so this last read is the narrowest window this tool can give.
main2=$(main_head)
[ "$main2" = "$main0" ] || { unset TOKEN; rm -f "$hdr"; [ ${#main2} -eq 40 ] || { echo "NOT MERGED: cannot re-read main's head before the merge call: ${main2:-empty answer}"; exit 3; }; main_refuse "$main0" "$main2" "during the PAT read"; exit 3; }
echo "    main re-read just before the merge call: ${main2:0:9}, unchanged since before GO"
resp=$(printf 'header = "Authorization: Bearer %s"\nheader = "Accept: application/vnd.github+json"\nheader = "X-GitHub-Api-Version: 2022-11-28"\n' "$TOKEN" \
  | curl -sS -K - -D "$hdr" -X PUT --max-time 60 -d "$body" "https://api.github.com/repos/$R/pulls/$PR/merge")
unset TOKEN
echo "merge call $(date -u +%FT%TZ): $(head -1 "$hdr" | tr -d '\r')"
grep -i '^x-oauth-scopes:' "$hdr" | tr -d '\r' || echo "x-oauth-scopes: (header absent — fine-grained token)"
rm -f "$hdr"
echo "$resp" | jq -c '{merged, sha, message}' 2>/dev/null || echo "$resp"
msha=$(echo "$resp" | jq -r 'select(.merged == true) | .sha' 2>/dev/null)
if [ -n "$msha" ]; then
  p1=$(gh api "repos/$R/commits/$msha" --jq '.parents[0].sha' 2>&1)
  if [ "$p1" = "$main0" ]; then echo "    merge commit ${msha:0:9} sits directly on ${main0:0:9}, the main GO judged"
  else echo "WARNING: merge commit ${msha:0:9} has first parent ${p1:0:40}, not ${main0:0:9}, the main GO judged. Landed in between:"; main_moved "$main0" "$p1"; fi
fi
