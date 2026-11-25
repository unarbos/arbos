#!/bin/bash
# checks.sh — the required-check reader shared by merge.sh and merge-window.sh. Sourced, never run.
#
# Why this file exists. Until 2026-10-31 both tools judged a head by rejecting known-bad conclusions
# among whatever the PR's check rollup happened to report. "skipped" and "neutral" are on no
# deny-list, and a gate that never ran reports nothing at all, so all three read as a pass. Measured
# on 3e5549e077d00d2f3a91226ca2f4063d9d0d241b: the deny-list's output was identical to what it would
# print for a head whose Swift gate had never existed. A deny-list cannot tell "nothing is wrong"
# from "nothing is there".
#
# So the gates below are named instead, and each must be PRESENT and successful on the pinned head.
# Silence is a refusal. Every uncertainty — an unparseable filter, a short read, an unreadable API —
# is BLOCKED, never "not required".
#
# That shape has now turned up three times in this one kit, which makes it a pattern rather than a
# tally: "nothing is there" standing in for "the query was wrong".
#   1. the deny-list described above, which had no entry for skipped, for neutral, or for absence;
#   2. verdict()'s run read, which looked at one page and reported "found nothing" when the truth
#      was "did not look at everything" (#2467);
#   3. gate_state's run lookup, which gets total_count 0 for an abbreviated head_sha and so reported
#      "no run exists for this sha at all" about a head whose gate had in fact run and failed.
# All three failed closed, and all three gave a false reason. The false reason is the harm: it points
# at the wrong remedy, and "no run exists" sends the reader to push and wait out a saturated macOS
# pool when the real answer is to go read the failed run. So the rule here is to assert a floor, and
# to let an unanswerable question be BLOCKED rather than letting it become an answer.
#
# The wider shape, because those three are one instance of it and not the whole: A CHECK THAT FAILED
# TO RUN REPORTS A VERDICT ABOUT ITS SUBJECT. Three more turned up in a single evening, in three
# different tools, none of them this file:
#   - a self-test case printed UNKNOWN because an unset $R made building an API path fatal under
#     `set -u`, so the stub it was about never executed. UNKNOWN was read as a finding about the
#     workflow under test. It was a finding about the test's own environment;
#   - a CI run evicted from an unkeyed concurrency group completed with zero jobs, and a zero-job
#     run satisfies "no job failed". It was read as green. Nothing had been compiled;
#   - a query returning no rows was read as "no job ran", when the query had never been constrained
#     to the thing being asked about.
# In each case something answered a question it had not asked, and the answer was a pass. The three
# wording changes in this change are the same bug in the reporting layer rather than the reading
# layer: SATISFIED printed when no gate applied said "this head is good" when it meant "I read
# nothing"; one-argument `checks` on a merge commit measured the incoming parent and said it was
# measuring the PR; a 300-entry file listing is GitHub's cap, so "the diff touches no gated path"
# may mean "the gated path is on page two". A reader that cannot say which of those it did is
# indistinguishable from one that checked, and that is the whole defect — so each of them now says
# what it did not read, and the two that cannot know refuse instead of reporting.
#
# This list IS the repository's required-check policy. main has no branch protection and no ruleset
# (branches/main reports "protected": false; rulesets and rules/branches/main are both empty), and
# the protection endpoint needs a token with Administration:Read, which the merge lane's token is
# not. Nothing on GitHub's side requires any check, so nothing but this file does. Keep it in step
# with .github/workflows/.

# rd <gh api args>: the read, or "BLOCKED:<error>" with a non-zero status. A failed read never
# becomes a verdict.
rd() {
  local o e; e=$(mktemp)
  if o=$(gh api "$@" 2>"$e"); then rm -f "$e"; printf '%s' "$o"
  else echo "BLOCKED:$(tr '\n' ' ' <"$e" | cut -c1-200)"; rm -f "$e"; return 1; fi
}

# GATES: one line per required check — <workflow file>|<human name>. A gate applies to a head only
# when that head's diff touches the workflow's own path filter, so a Mac-only change is not asked
# for an iPhone verdict.
#
# ci.yml is deliberately absent. Its requiredness turns on `paths-ignore`, which merge-window.sh's
# verdict() already reads for a different purpose, and asserting it here without that logic would
# refuse legitimate iOS-only merges. Adding it is a separate change with its own proof.
GATES='.github/workflows/swift-core-check.yml|Swift core check
.github/workflows/release-gate.yml|Release gate'

# gate_patterns <workflow> <ref>: every quoted path pattern under that workflow's top level `on:`
# block, read at <ref>. Status 0 with the patterns on stdout when it was read; 2 when <ref>'s tree
# does not contain the workflow at all; 1 when the read itself failed.
#
# The push and pull_request lists are unioned. They are identical today, and a superset errs towards
# asking for a verdict, which is the safe direction. Unquoted entries yield nothing, and nothing is
# BLOCKED by the caller rather than "not required".
#
# Absent and unreadable are separate answers because they have opposite remedies. This file's header
# lists three times that "nothing is there" stood in for "the query was wrong"; this is the mirror
# of it, and it bites once per gate added. A workflow a ref's tree does not have is a fact about
# that ref, not an unknown: a gate added to main after a branch forked is in no open branch's tree,
# and the pull request that adds a gate is in no tree on main until it merges. Folding that into
# BLOCKED refuses every open pull request the moment a gate is added — including the one adding it,
# which can then never land. Every other failure (network, 403, a bad token) stays a refusal,
# because then the answer really is unknown. Which of the two it was is decided by the 404 GitHub
# returns for a path that is not in the tree, never by an empty answer.
gate_patterns() {
  local body
  if ! body=$(rd "repos/$R/contents/$1?ref=$2" --jq '.content' 2>/dev/null); then
    case "$body" in *"HTTP 404"*|*"Not Found"*) return 2 ;; esac
    return 1
  fi
  printf '%s' "$body" | base64 -d 2>/dev/null | awk '
    /^on:/          { inon = 1; next }
    /^[^[:space:]]/ { inon = 0 }
    inon && /^ *- *"/ { gsub(/^ *- *"|" *$/, ""); print }'
}

# files_match_patterns <files> <patterns>: 0 when any file matches any pattern. Handles the two
# shapes this repo's filters use — a `dir/**` prefix and a literal path. Anything else is a pattern
# this cannot read, so it returns 2 and the caller blocks.
files_match_patterns() {
  local files=$1 pats=$2 f p
  while IFS= read -r p; do
    [ -n "$p" ] || continue
    case "$p" in
      */'**') ;;
      *'*'*) return 2 ;;
    esac
  done <<< "$pats"
  while IFS= read -r f; do
    [ -n "$f" ] || continue
    while IFS= read -r p; do
      [ -n "$p" ] || continue
      case "$p" in
        */'**') case "$f" in "${p%'**'}"*) return 0 ;; esac ;;
        *) [ "$f" = "$p" ] && return 0 ;;
      esac
    done <<< "$pats"
  done <<< "$files"
  return 1
}

# pr_files <pr>: every file in the PR's three-dot diff, every page. The three-dot diff is the one
# that matters: a push's two-dot diff counts whatever main changed since the branch forked.
pr_files() {
  rd --paginate "repos/$R/pulls/$1/files?per_page=100" --jq '.[].filename'
}

# commit_files <sha>: the files a commit changed against its first parent. GitHub caps this listing
# at 300, so a list at the cap is treated as "cannot tell" by the caller rather than as a short
# answer that happens to miss the gated path.
commit_files() {
  rd "repos/$R/commits/$1" --jq '.files[]?.filename'
}

# files_capped <files>: 0 when the listing is at GitHub's 300-file cap, and so cannot be trusted to
# contain the gated path.
#
# A function rather than an inline comparison because a guard that is only ever grepped for is not
# a guard that is tested: the digits "300" survive in the BLOCKED message even after the
# comparison is gone, so a source-level assertion passes against the broken state. This is
# callable, so the cases can hand it 299, 300 and 301 files and watch what it does.
files_capped() {
  [ "$(printf '%s\n' "$1" | grep -c .)" -ge 300 ]
}

# gate_push_triggered <workflow> <ref>: can that workflow produce a run for a commit sitting on a
# branch at all? Status 0 yes, 1 no, 2 could not tell. The reason is on stdout for 1 and 2.
#
# WHY A GATE CAN BE REQUIRED AND YET UNANSWERABLE ON main, which is what this is for.
#
# range_gate asks each gate for a verdict on a commit that is already on main. For a gate that runs
# on `push` that question has an answer. For a gate that runs only on `pull_request` it has none and
# never will: GitHub starts no run for a push, so the commit carries total_count 0 for ever.
# range_gate read that as "a required check has no successful run on the commit that decides it" and
# refused — pointing the operator at a remedy that does not exist, since nothing anyone can do makes
# that run appear. It is the same mistake as gate_run_class above, one door along: there the gate
# died before judging and was called a refusal, here the gate cannot be asked and is called a
# missing pass. Both refuse. Neither is true.
#
# Measured, and it deadlocked the window: .github/workflows/release-gate.yml is `pull_request` only
# and deliberately lists ITSELF in its own `paths:`, so #2727 — which changed that workflow — landed
# on main as 54b199042 and became a commit the gate applies to and can never judge. No new tag
# would clear it either, because the range only ends at a tag and mac-change.sh was rc=0, so
# release.yml had nothing to publish. Every merge was refused, permanently, with the window open.
#
# WHY SKIPPING IT OPENS NO HOLE. The gate's real coverage is before the merge, not after. merge.sh
# runs require_gates on the pull request's own head and takes no argument that waives it — accept-red
# names a run whose result is ignored, and a gate with no result has no run to name. So the verdict
# this would re-ask for has already been obtained, on the same content: for 54b199042 the three files
# on main are byte-identical blobs to #2727's head 3a41016c3, which run 41859964806 passed on both
# jobs. The only case not covered is a direct push to main, which carries no pull request and so has
# no verdict by either route — and merge-window.sh's header already concedes that an unprotected main
# permits a push no check can see. This narrows nothing that was ever actually checked.
#
# Reading `on:` rather than taking a list: a hardcoded "release-gate.yml is pull_request only" goes
# stale silently the day someone adds a trigger, in the permissive direction. Anything this cannot
# classify is status 2, and the caller keeps refusing, because an unanswerable question is BLOCKED
# and not an answer — the same rule the rest of this file follows.
gate_push_triggered() {
  local body
  if ! body=$(rd "repos/$R/contents/$1?ref=$2" --jq '.content' 2>/dev/null); then
    echo "cannot read $1 at $2, so whether it can run on a commit there is unknown"; return 2
  fi
  local verdict
  verdict=$(printf '%s' "$body" | base64 -d 2>/dev/null | awk '
    /^on:[[:space:]]*$/          { inon = 1; seen = 1; next }
    /^on:[[:space:]]*[^[:space:]]/ { seen = 1; inline = $0; next }
    /^[^[:space:]]/              { inon = 0 }
    inon && /^  push:/           { push = 1 }
    END {
      if (!seen) { print "UNKNOWN"; exit }
      if (inline != "") {
        # `on: push`, `on: [push, pull_request]`. Anything else is not classified.
        if (inline ~ /^on:[[:space:]]*push[[:space:]]*$/) { print "PUSH"; exit }
        if (inline ~ /^on:[[:space:]]*\[/) { print (inline ~ /[[,][[:space:]]*push[[:space:]]*[],]/) ? "PUSH" : "NOPUSH"; exit }
        print "UNKNOWN"; exit
      }
      print (push ? "PUSH" : "NOPUSH")
    }')
  case "$verdict" in
    PUSH)   return 0 ;;
    NOPUSH) echo "$1 has no \`push:\` trigger at $2, so no commit on $2 can carry a run of it — its verdict is taken before the merge, on the pull request's own head, where merge.sh requires it and waives nothing"; return 1 ;;
    *)      echo "the \`on:\` block of $1 at $2 is not in a shape this can classify, so whether it can run on a commit there is unknown"; return 2 ;;
  esac
}

# gate_state <workflow> <sha>: one line — PASS <detail> | RED <detail> | NOVERDICT <detail> |
# PENDING <detail> | NONE | BLOCKED <what could not be read>.
#
# Read by head_sha, not from the PR's rollup. Two check runs on one head can share a job name (the
# push run's and the pull_request run's), and the rollup gives no way to tell which result belongs
# to which event. The runs API keyed on the sha is exact. Every page is read, and a short read is
# loud, for the reason #2467 documents.
#
# CONSTRAINT — what a run-level conclusion is allowed to mean, and why it is not this file's to
# guarantee. This reads the RUN's conclusion rather than a named check row, and PASS accepts only
# `success`.
#
# What `success` is worth. A run concludes `success` only when some job actually succeeded and none
# failed or was cancelled. Skipped jobs neither produce that conclusion nor prevent it, which takes
# two measurements to pin down: run 41144617495 has a single skipped job and concludes `skipped`,
# while run 41244242835 has one skipped job beside three successes and concludes `success`. So a
# `success` run is always one in which something really ran and passed. The only open question is
# whether the thing that passed was the check.
#
# Today it was, because this workflow has exactly one job, `check`. After #2448 it is because the
# workflow ends in `verdict` ("Swift verdict"), which is `if: always()` and exits non-zero unless a
# check actually passed on this head: `always()` keeps it out of only a cancelled run, and a
# cancelled run concludes `cancelled`, so a `success` run is one where `verdict` itself ran and
# passed.
#
# Two edits break that, and both live in a YAML file nobody reviewing this one would think to open:
#   - today, adding ANY job that can succeed while `check` is skipped. Run 41244242835 above is
#     already that shape, so this is observed behaviour and not a worry about one.
#   - after #2448, `continue-on-error: true` on `verdict`. A tolerated failure stops failing the
#     run, so the run concludes `success` on the strength of its other jobs with no check passed.
# Either one makes this function quietly permissive again with no line here changing. The matching
# constraint comment is carried on the workflow side by #2448's branch, the change that adds
# `verdict`; until that lands, main's workflow is the single-job form above and there is no comment
# there to find.
#
# Reading rows by name instead would only trade this dependency for a worse one:
# `build-xcframework + fastlane test` is skipped in the mirror run exactly when the push twin
# passed, so requiring it by name would refuse when coverage exists.
gate_state() {
  local wf=$1 sha=$2 base total list got
  base="repos/$R/actions/workflows/${wf##*/}/runs?head_sha=$sha"
  total=$(rd "$base&per_page=1" --jq '.total_count') || { echo "BLOCKED counting ${wf##*/} runs for ${sha:0:8}: ${total#BLOCKED:}"; return; }
  [ "${total:-0}" -eq 0 ] && { echo "NONE"; return; }
  list=$(rd --paginate "$base&per_page=100" --jq '.workflow_runs[]|"\(.id) \(.event) \(.status) \(.conclusion)"') \
    || { echo "BLOCKED listing ${wf##*/} runs for ${sha:0:8}: ${list#BLOCKED:}"; return; }
  got=$(printf '%s\n' "$list" | grep -c .)
  [ "$got" -ge "${total:-0}" ] || { echo "BLOCKED ${wf##*/} run lookup incomplete for ${sha:0:8}: read $got of $total runs"; return; }

  # Every run is named in the detail, whatever the label. The bug this replaces was a result that
  # never appeared in any output, so a result that is not a pass is listed rather than summarised.
  local pass all red pend
  pass=$(printf '%s\n' "$list" | awk '$4=="success"{print $1" ("$2")"}' | head -1)
  [ -n "$pass" ] && { echo "PASS run $pass"; return; }
  all=$(printf '%s\n' "$list" | awk '{print $1" ("$2", "($3 == "completed" ? $4 : $3)")"}' | paste -sd', ' -)
  pend=$(printf '%s\n' "$list" | awk '$3!="completed"')
  [ -n "$pend" ] && { echo "PENDING $got run(s) on this head, none successful yet: $all"; return; }
  # skipped and neutral are not failures, and that is the whole trap: they are not verdicts either.
  # A job skipped by an `if:` condition did no work, so it proves nothing about this commit.
  red=$(printf '%s\n' "$list" | awk '$4!="success" && $4!="skipped" && $4!="neutral"')
  [ -n "$red" ] && { echo "RED $got run(s) on this head, none successful: $all"; return; }
  # A canary, not a live path — and kept deliberately. Under #2448 every run reaches the always()
  # verdict job, so a wholly skipped or neutral run should be unreachable for a fresh head. It stays
  # because it is still live for historical heads, whose all-skipped runs sit on main's history
  # permanently and are read by the audit paths, and because the red= filter above deliberately
  # excludes skipped and neutral, so this case arrives here by falling through everything else.
  # Delete the branch and the case has to be reclassified; the convenient reclassification is "pass",
  # which restores the original bug inside the one function written to prevent it. If this line ever
  # fires for a fresh head, the verdict job's fail-closed guarantee has been broken — read it as that
  # alarm, not as a routine skip.
  echo "NOVERDICT $got run(s) on this head, every one skipped or neutral, so none of them tested it: $all"
}

# gate_applies <workflow> <sha> <files>: 0 when the diff touches the workflow's path filter, 1 when it
# touches none, 2 when that cannot be told. For 1 and 2 it prints why.
#
# Both filters, unioned. The head's own filter explains which runs exist; the base's filter is what
# will gate the code once it lands. They differ in practice: #2376 changes
# crates/arbos-matrix/src/client.rs, and crates/arbos-matrix/** was added to this filter after that
# branch forked, so no run could ever have been created for its head while main would gate the merged
# result. Judging by either filter alone misses that; the union asks for the verdict.
#
# A gate in neither tree is not yet policy: nothing can have run for it on this head, and nothing
# will gate the merged result either, so it is "not required" rather than a refusal. A gate in
# main's tree but not the head's is the #2376 case above, and still asks for the verdict the head
# cannot have — the remedy is to rebase, which is the point.
gate_applies() {
  local wf=$1 sha=$2 files=$3 phead pbase hrc brc pats
  phead=$(gate_patterns "$wf" "$sha"); hrc=$?
  pbase=$(gate_patterns "$wf" "${GATE_BASE:-main}"); brc=$?
  if [ $hrc -eq 2 ] && [ $brc -eq 2 ]; then
    echo "not required — neither this head's tree nor ${GATE_BASE:-main}'s contains $wf, so no merge of this head is gated by it"
    return 1
  fi
  if [ $hrc -eq 1 ] || [ $brc -eq 1 ] \
     || { [ $hrc -eq 0 ] && [ -z "$phead" ]; } || { [ $brc -eq 0 ] && [ -z "$pbase" ]; }; then
    echo "BLOCKED — cannot read the path filter in $wf at this head or at ${GATE_BASE:-main}, so whether it applies is unknown"
    return 2
  fi
  pats=$(printf '%s\n%s\n' "$phead" "$pbase" | grep -v '^$' | sort -u)
  files_match_patterns "$files" "$pats"
  case $? in
    0) return 0 ;;
    1) echo "not required — this diff touches none of its $(printf '%s\n' "$pats" | grep -c .) filtered paths"; return 1 ;;
    *) echo "BLOCKED — $wf has a path pattern this cannot evaluate"; return 2 ;;
  esac
}

# landed_gate_state <workflow> <sha>: gate_state for a commit that is ALREADY on main, which is a
# different question from gate_state's and needs two things gate_state must not do.
#
# Prints what gate_state prints, plus one more word it can return: NOTASKED <why>. The caller treats
# that as a finding and not a refusal.
#
# 1. A PULL-REQUEST RUN LANDS ON THE SECOND PARENT, NOT ON THE MERGE COMMIT. A `pull_request` run is
#    recorded against the pull request's head sha. When that pull request lands as a merge commit,
#    that very sha becomes the merge commit's second parent and is an ancestor of main, so the run
#    is still reachable and the audit still works. Asking only about the merge commit throws it
#    away. Measured on main's own history: dd0d8db10^2 is 92337f833, #2726's head.
#
# 2. A SQUASH HAS NO SECOND PARENT, AND NEITHER DOES A DIRECT PUSH. Of the five commits on main
#    after v0.6.382 on 2026-11-04, four were squash-merged by hand and carry one parent each; only
#    dd0d8db10, merged through this kit, has a second. So route 1 is the precise answer and not a
#    complete one, and the rule below is what the remainder falls to.
#
# Only a gate with NO RUN AT ALL reaches that rule. A run that exists and failed is a verdict on
# this commit and still refuses — the point here is the difference between a question answered badly
# and a question that was never asked, which is this file's whole subject.
landed_gate_state() {
  local wf=$1 sha=$2 state p2 s2 why prc
  state=$(gate_state "$wf" "$sha")
  case "$state" in PASS*) echo "$state"; return ;; esac
  p2=$(git rev-parse -q --verify "$sha^2" 2>/dev/null)
  if [ -n "$p2" ]; then
    s2=$(gate_state "$wf" "$p2")
    case "$s2" in PASS*) echo "PASS on ${p2:0:9}, the pull request head this commit merged — ${s2#PASS }"; return ;; esac
  fi
  if [ "$state" = NONE ]; then
    why=$(gate_push_triggered "$wf" "${GATE_BASE:-main}"); prc=$?
    [ $prc -eq 1 ] && { echo "NOTASKED not required on a commit already on ${GATE_BASE:-main} — $why"; return; }
    [ $prc -eq 2 ] && { echo "BLOCKED no run of ${wf##*/} exists for ${sha:0:9}, and $why"; return; }
  fi
  echo "$state"
}

# require_gates <sha> <files>: print one indented finding per gate and return 0 only when every gate
# that applies to this head is satisfied. 1 = a gate has no passing run, 2 = a gate could not be read,
# 3 = a gate is still running. All three are refusals to a caller that fails closed; they are separate
# so the message can say which, and so "wait" does not read as "wrong".
#
# <files> is the newline-separated diff for this head, from pr_files or commit_files.
#
# GATES_APPLIED is set to the number of gates that actually applied. A caller needs it to tell the
# two very different states that both return 0 apart: "every gate that applies passed" and "no gate
# applies, so nothing was tested here". Reporting both as a bare success is what let a head with
# all of CI queued read as satisfied.
require_gates() {
  local sha=$1 files=$2 wf name phead pbase pats state rc=0
  GATES_APPLIED=0
  # GitHub's head_sha run filter matches the full 40-character sha only. An abbreviated one is not
  # an error: it returns total_count 0, which gate_state would report as "no run exists for this
  # sha at all". That is this file's own bug one level down — "nothing is there" standing in for
  # "the query was wrong" — and it sends the reader to push a new commit when the gate in fact ran
  # and failed. Measured on d127f8df2960c81b84753077933635986f86d0d5, whose gate run 41207432393
  # failed: the full sha reports RED, and `d127f8df2` reported no run at all. So a sha this cannot
  # query is BLOCKED, never "no verdict" and never "not required". Callers inside this kit pass
  # %H, headRefOid or the pinned head; the operator subcommand resolves its argument first.
  local norm; norm=$(printf '%s' "$sha" | tr 'A-F' 'a-f')
  case "$norm" in *[!0-9a-f]*) norm='' ;; esac
  if [ "${#norm}" -ne 40 ]; then
    echo "    required checks on '$sha': BLOCKED — a gate is judged by the full 40-character head sha and this is not one, so no run lookup for it can be trusted. Resolve it first: gh api repos/\$R/commits/<ref> --jq .sha"
    return 2
  fi
  sha=$norm
  while IFS='|' read -r wf name; do
    [ -n "$wf" ] || continue
    state=$(gate_applies "$wf" "$sha" "$files")
    case $? in
      1) echo "    required check '$name' on ${sha:0:9}: $state"; continue ;;
      2) echo "    required check '$name' on ${sha:0:9}: $state"; rc=2; continue ;;
    esac
    GATES_APPLIED=$((GATES_APPLIED + 1))
    state=$(gate_state "$wf" "$sha")
    case "$state" in
      PASS*)    echo "    required check '$name' on ${sha:0:9}: $state" ;;
      BLOCKED*) echo "    required check '$name' on ${sha:0:9}: $state"; rc=2 ;;
      PENDING*) echo "    required check '$name' on ${sha:0:9}: NO VERDICT YET — $state"; [ $rc -eq 0 ] && rc=3 ;;
      NONE)     echo "    required check '$name' on ${sha:0:9}: NO VERDICT — required by path, and no run of $wf exists for this sha at all"; [ $rc -eq 2 ] || rc=1 ;;
      *)        echo "    required check '$name' on ${sha:0:9}: NO VERDICT — $state"; [ $rc -eq 2 ] || rc=1 ;;
    esac
  done <<< "$GATES"
  return $rc
}

# ---------------------------------------------------------------------------
# What a verdict() line means, in one place.
#
# verdict() is in merge-window.sh, but two callers there branch on its output —
# go() decides whether to add a NO-GO, after() decides LIVE / NO-TAG / STOP —
# and until 2026-11-04 each kept its own `case`. Two case statements that must
# agree about which words refuse is a bug waiting to be written, and it was:
# adding a verdict word without touching after() drops it into that function's
# default arm, which prints WAIT. WAIT is not a refusal. A new word intended to
# describe a *failure* would have quietly started permitting one.
#
# So the word list lives here, once. go() reads it directly. after() does not,
# and that is on purpose rather than an oversight: its arms are not a
# classification but five different pieces of prose, and TAG's arm does real
# work. It also leans on prefix order — `RED GATE ...` is meant to reach its
# `RED*` arm — so folding it onto these classes would send REDGATE somewhere
# it has no arm, which is the very bug this comment is about. after() is held
# to the same contract a different way: its default arm now STOPs instead of
# printing WAIT, and selftest.sh asserts both that it has an arm per refusing
# word and that the default is not a shrug.
#
# These two are pure — no network, no git — which is also why they are in this
# file rather than beside verdict(): merge-window.sh runs its dispatcher when
# sourced, so selftest.sh cannot reach anything defined in it.
#
# UNKNOWN is the important row. An unrecognised verdict refuses, so forgetting
# to register a new word costs a false refusal and never a false pass.

# verdict_class <verdict-line>: one word for the whole line.
verdict_class() {
  case "$1" in
    "TAG "*)      echo TAG ;;
    "NOMAC "*)    echo NOMAC ;;
    NOPATH*)      echo NOPATH ;;
    COVERED*)     echo COVERED ;;
    "RED GATE"*)  echo REDGATE ;;
    RED*)         echo RED ;;
    UNTESTED*)    echo UNTESTED ;;
    BLOCKED*)     echo BLOCKED ;;
    NOPLAN*)      echo NOPLAN ;;
    WAIT*)        echo WAIT ;;
    *)            echo UNKNOWN ;;
  esac
}

# verdict_clears_go <class>: 0 only for the classes that can let a GO proceed.
#
# NOMAC is here, but it is not sufficient on its own: go() must also check that
# the tag it names is the tag being released. That check stays at the call site
# because this function is deliberately ignorant of which tag is current.
#
# Everything else refuses, UNTESTED included. An aborted run tested nothing,
# which is the definition of ungated, so it is exactly the state that must not
# pass. See the comment above `ci_abort_state` in merge-window.sh.
# NOPLAN is "may clear", exactly as NOMAC is, and NOT "clears". Both require go() to find a
# covering descendant carrying a current-tag NOMAC; neither clears on its own. The difference
# from NOPATH, which does clear outright, is the whole reason go() must keep an explicit arm for
# each of them rather than letting either fall into its default.
verdict_clears_go() {
  case "$1" in
    NOMAC | NOPATH | NOPLAN) return 0 ;;
    *) return 1 ;;
  esac
}

# ---------------------------------------------------------------------------
# The same shape again, for stack_chain. These live here for the same two
# reasons verdict_class does: merge-window.sh runs its dispatcher when sourced,
# so selftest.sh cannot reach anything defined there, and a classification that
# only exists as a caller's `case` is one that drifts from its second caller.
#
# Why it needs classifying at all. A pull request based on a branch does not
# merge into main, so a clean reading of its own diff is accurate and partial
# at the same time. The base chain is what completes it — and a chain that
# cannot be walked completes nothing, which is a refusal rather than a pass.

# stack_class <chain-line> <pr>: one word for a stack_chain result.
#
#   BLOCKED     a read failed, so the chain is unknown rather than absent
#   SELF        based on main — this head's own reading IS the whole story
#   ROOT        a real stack, terminating at a pull request based on main
#   UNRESOLVED  a base branch that no open pull request produces
#   CYCLE/DEPTH the walk does not terminate
#   UNKNOWN     a chain shape nobody has reasoned about
stack_class() {
  case "$1" in BLOCKED*) echo BLOCKED; return ;; esac
  local last; last=$(printf '%s' "$1" | awk '{print $NF}')
  case "$last" in
    UNRESOLVED:*) echo UNRESOLVED ;;
    CYCLE)        echo CYCLE ;;
    DEPTH)        echo DEPTH ;;
    "$2")         echo SELF ;;
    ''|*[!0-9]*)  echo UNKNOWN ;;
    *)            echo ROOT ;;
  esac
}

# stack_refuses <class>: 0 when the stack reading on its own must refuse.
#
# UNKNOWN refuses, for the reason everything in this file refuses on an
# unrecognised word: forgetting to register a new shape costs a false refusal
# and never a false pass. BLOCKED is deliberately not in the refusing set — it
# refuses too, but its caller reports it as BLOCKED, so the reader is sent to
# the failed read rather than to the stack.
stack_refuses() {
  case "$1" in
    UNRESOLVED | CYCLE | DEPTH | UNKNOWN) return 0 ;;
    *) return 1 ;;
  esac
}

# result_line <rc> <gates-applied>: the one line a reader actually acts on.
#
# Here, and callable, for the same reason as everything else in this section: a
# message asserted by grepping the caller is a message no case can really test.
# The first version of the vacuous-pass case grepped checks() for the phrase,
# and the phrase occurs twice there — once for the head and once for the stack
# root — so a mutation that reverted the head's line survived behind the root's.
#
# rc=0 has TWO readings and that is the whole point of this function. "Every
# gate that applies has passed" and "no gate applies, so nothing ran" are the
# same exit code and very different facts, and collapsing them into one
# sentence is what let a head with all of CI queued read as satisfied.
result_line() {
  case "$1" in
    0) if [ "${2:-0}" -eq 0 ]; then
         echo "SATISFIED VACUOUSLY — no path-filtered gate applies to this head, so none of them ran and none of them had to. This is NOT 'this head is green': require_gates reads only the gates in its GATES list and never looks at the always-on CI workflow (ci.yml), so CI may be queued, running or red right now."
       else
         echo "SATISFIED — every path-filtered gate that applies to this head ($2 of them) has a successful run on it. CI (ci.yml) is not among the gates read here, so this says nothing about CI."
       fi ;;
    1) echo "REFUSE — a gate that applies to this head has no successful run on it, or what reaches main cannot be determined" ;;
    2) echo "BLOCKED — a gate could not be read, so nothing is inferred" ;;
    3) echo "WAIT — a gate that applies to this head is still running" ;;
    # An unrecognised code is not a pass. require_gates returns 0..3 today, so
    # this arm is reachable only by a fourth code added without reading here.
    *) echo "BLOCKED — unrecognised result code '$1', so nothing is inferred" ;;
  esac
}
