#!/bin/bash
# merge.sh <pr> <pinned-head-sha> [accept-red=<run-id>[,<run-id>]]
# Runs a fresh GO and, only on GO, merges <pr> as a merge commit pinned to <pinned-head-sha>.
# Refuses if main's head moves between the GO check and the merge call, and names each commit that landed.
# The PAT lives in one shell variable of this process (sent to curl over stdin, never argv or a file)
# and is unset as soon as the merge call returns.
# Run from the repo checkout. MERGE_KIT (default /tmp/k) holds this session's files: merge-allow (the PR numbers
# the coordinator cleared, `#` starts a comment), watch-solo (merge-window.sh) and op-last.err.
set -u
D=$(cd "$(dirname "$0")" && pwd); K=${MERGE_KIT:-/tmp/k}
source "$D/opread.sh"
source "$D/checks.sh"
ALLOW=$(sed 's/#.*//' "$K/merge-allow" 2>/dev/null | tr -s ' \t\n' ' ' | sed 's/^ //; s/ $//')
[ -n "$ALLOW" ] || { echo "REFUSED: no merge list at $K/merge-allow"; exit 2; }
PR=$1; SHA=$2; FLAKE=""; OVER=""; PEND=""
for a in "${@:3}"; do case "$a" in accept-red=*) FLAKE=$a ;; over-serialize=*) OVER=${a#over-serialize=} ;; accept-pending=*) PEND=${a#accept-pending=} ;; *) echo "REFUSED: unknown argument $a"; exit 2;; esac; done
R=unarbos/arbos
case " $ALLOW " in *" $PR "*) ;; *) echo "REFUSED: #$PR is not in the coordinator's merge list ($ALLOW)"; exit 2;; esac

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
out=$(bash "$D/merge-window.sh" go "$PR" 2>&1)
echo "$out" | grep -vE '^\s+(release:|dmg|feed \(|latest release|download/mac)'
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
# rollup_red: a statusCheckRollup object on stdin, one line out per non-success row:
#   <this row's run id, or "untraceable"> <run ids of the green rows for this same check, or "-"> <check name> <conclusion>
#
# Those green ids are the candidates for rule 9's "a run on the identical head sha is green for that
# same check". Identical head sha holds by construction and is not inferred: every row in one rollup
# belongs to one commit, the PR's head, pinned to $SHA above and re-read below. Same check means the
# same kind of row, from the same workflow, under the same name — GitHub files the push run's copy of
# a job and the pull_request run's copy under one name, and that pair is the shape rule 9 is about.
# SKIPPED and NEUTRAL are not green; a job that did no work proves nothing, which is the point
# checks.sh is built around. A green row with no run behind it is not offered at all, because what it
# tested cannot be established — see green_twin.
rollup_red() {
  jq -r '
    def runid: ((.detailsUrl // .targetUrl // "") | [capture("runs/(?<id>[0-9]+)").id] | first);
    def rowkey: [.__typename, (.workflowName // ""), (.name // .context)];
    [.statusCheckRollup[] | select((.conclusion // .state) == "SUCCESS")] as $green
    | .statusCheckRollup[]
    | select((.conclusion // .state) as $c | ["FAILURE","CANCELLED","TIMED_OUT","ACTION_REQUIRED","STARTUP_FAILURE","ERROR","STALE"] | index($c))
    | . as $row
    | ([$green[] | select(rowkey == ($row | rowkey)) | runid | select(. != null)] | join(",")) as $twins
    | "\(($row | runid) // "untraceable") \(if $twins == "" then "-" else $twins end) \($row | .name // .context) \($row | .conclusion // .state)"'
}
# run_event <run id>: the event GitHub recorded for that run, or "unreadable". The rollup does not
# carry it — a row names its run only through detailsUrl — so it has to be read, and $EVENTS keeps
# each answer. A rollup names about two distinct runs however many rows it has, and the loop below
# primes $EVENTS before it starts, so this costs about two reads per merge.
EVENTS=""
run_event() {
  local e
  case "$1" in ''|*[!0-9]*) printf 'unreadable'; return ;; esac
  case "$EVENTS " in *" $1="*) e=${EVENTS#*" $1="}; printf '%s' "${e%% *}"; return ;; esac
  e=$(gh api "repos/$R/actions/runs/$1" --jq '.event' 2>/dev/null) || e=''
  [ -n "$e" ] || e=unreadable
  EVENTS="$EVENTS $1=$e"
  printf '%s' "$e"
}
# tested_rank <event>: how much of the merged result a run of that event tested. A push run builds the
# head alone; a pull_request run builds the head merged into main's tip, so it tests strictly more.
# 0 is "cannot be told", and nothing is ever ranked against it.
tested_rank() { case $1 in push) echo 1 ;; pull_request) echo 2 ;; *) echo 0 ;; esac; }
# green_twin <this row's run id> <candidate green run ids>: "<run id> <event>" for the first candidate
# that tested at least as much of the merged result as this row's run did, or nothing.
green_twin() {
  local mine g ge
  mine=$(tested_rank "$(run_event "$1")")
  [ "$mine" -gt 0 ] || return 1
  for g in $(printf '%s' "$2" | tr ',' ' '); do
    case $g in ''|*[!0-9]*) continue ;; esac
    ge=$(run_event "$g")
    [ "$(tested_rank "$ge")" -ge "$mine" ] && { printf '%s %s' "$g" "$ge"; return 0; }
  done
  return 1
}
# A second pass, over every other check the rollup reports. The required gates above decide whether the
# checks that matter ran and passed; this one catches a red result in a check nobody thought to name,
# and accept-red=<id>[,<id>] can waive it (a run that did not test this PR).
# It cannot waive a required gate: that refusal already happened, above, and took no arguments.
#
# Rule 9's green clause is settled here rather than left to accept-red. gate_state has read a head that
# way since it was written, and the comment above this file's required-gate refusal leans on it — "a
# run on the identical head sha is green for that same check" is given there as the reason the
# legitimate flake case needs no override. This pass did not read it, so the kit refused merges its own
# stated rule allows and an argument was the only way past, which is how two of tonight's hours went.
# Measured on f4297c758f30acfbce5fbe4f83e3adab3cb106d0: `rust (fmt, clippy, test)` concluded failure in
# push run 41513010699 and success in pull_request run 41513719101, both on that sha.
#
# The clause runs one way only, and that direction is the rule rather than a trim on it. Nobody asserts
# anything here, so it has to hold with nobody in the loop. A push run builds the head alone; a
# pull_request run builds the head merged into main's tip, and is therefore the only automated test
# this change gets against what has landed since. A push green settling a pull_request red would throw
# that test away and merge on the strength of a weaker one — which is the failure AF-647 is about, where
# two changes to one route merge clean with no line in common and only the merged-result run sees it.
# So a green settles a red only when its run tested at least as much, and a run whose event cannot be
# established settles nothing: a commit status has no run at all, a run read can fail, and an event that
# is neither of these two says nothing about what was built. An unanswerable question is a refusal here
# as everywhere else in this kit.
#
# Measured against every PR this repository had red tonight. #2569 above is the safe direction. #2589
# (2b825a625), #2591 (06308d0b1) and #2592 (aa322cda0) are all the unsafe one — push green, pull_request
# red — and all three fail the same test, which main itself is red on (AF-646), while none of the three
# changes any Rust. So the unnarrowed clause would have done the wrong thing harmlessly on every case in
# front of it, in evidence indistinguishable from the case where it is not harmless.
rollup=$(gh pr view "$PR" -R $R --json statusCheckRollup) || { echo "NOT MERGED: cannot read the check rollup"; exit 3; }
red=$(printf '%s' "$rollup" | rollup_red) || { echo "NOT MERGED: cannot read the check rollup: it did not parse"; exit 3; }
acc=",${FLAKE#accept-red=},"
# Every run the rollup names, read once, here. It has to be here: the lookups below all happen inside
# a $( ), and a cache filled in a subshell goes away with it. Anything that is not a run id —
# "untraceable", "-" — run_event turns away without a call.
for rid in $(printf '%s\n' "$red" | cut -d' ' -f1,2 | tr ', ' '\n' | sort -u); do
  run_event "$rid" >/dev/null
done
while read -r id twins name; do
  [ -n "$id" ] || continue
  if g=$(green_twin "$id" "$twins"); then
    echo "    red check settled by run ${g% *} (${g#* }), green for this same check on ${SHA:0:9} and at least as merged as this $(run_event "$id") run: $id $name"
    continue
  fi
  [ "$twins" = "-" ] || echo "    a green run of this same check exists on ${SHA:0:9} (run ${twins//,/, }), but it cannot be shown to have tested at least as much of the merged result as run $id did, so it does not settle this row"
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
  || { echo "NOT MERGED: PAT read failed at $(date -u +%FT%TZ) (cache off, so it reached 1Password; refused requests are not counted): $(head -c 300 "$K/op-last.err")"; exit 4; }
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
