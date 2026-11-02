#!/bin/bash
# checks.sh — the required-check reader and checkout guard shared by merge.sh and merge-window.sh.
# Sourced, never run.
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
# This list IS the repository's required-check policy. main has no branch protection and no ruleset
# (branches/main reports "protected": false; rulesets and rules/branches/main are both empty), and
# the protection endpoint needs a token with Administration:Read, which the merge lane's token is
# not. Nothing on GitHub's side requires any check, so nothing but this file does. Keep it in step
# with .github/workflows/.

# main's head, read live from GitHub (never the local origin/main, which another fetch can move mid-run).
main_head() { local o; o=$(git ls-remote origin refs/heads/main 2>&1) || { echo "git ls-remote failed: ${o:0:160}"; return; }; echo "$o" | awk '$2=="refs/heads/main"{print $1}'; }

# require_main_checkout: 0 only when this is a clean checkout of unarbos/arbos sitting on main's
# live head. Otherwise it names what it found and what it expected, and returns 1.
#
# "Run from the repo checkout" was the entire specification, and nothing read the checkout to see
# whether it was true. Most of what this kit judges about main comes from local refs and objects:
# changes_since() takes the range <tag>..origin/main, verdict() asks `git tag --contains`, the
# NOPATH branch reads that commit's ci.yml and diff out of the object store. A stale origin/main
# silently narrows the range, so a GO clears commits it never looked at; a feature branch or a
# dirty tree means the kit being run is not the kit main carries. The fetches inside this kit do
# not close that — each is `git fetch -q ... 2>/dev/null`, so a fetch that failed is
# indistinguishable from one that had nothing to do, which is how a stale ref gets this far.
#
# Judged against GitHub rather than against the local refs, for the same reason main_head() exists.
# Untracked files count as dirty: this kit keeps its session state in $MERGE_KIT, outside the
# checkout, so nothing a lane legitimately needs lives loose in the worktree.
require_main_checkout() {
  local url live ref head on dirty
  git rev-parse --is-inside-work-tree >/dev/null 2>&1 \
    || { echo "REFUSED: $PWD is not a git checkout; run the merge kit from the repo checkout"; return 1; }
  url=$(git remote get-url origin 2>/dev/null) \
    || { echo "REFUSED: this checkout has no 'origin' remote, so main's head cannot be read from it"; return 1; }
  case "$url" in *github.com[:/]"$R"|*github.com[:/]"$R".git) ;;
    *) echo "REFUSED: origin is $(printf '%s' "$url" | sed 's#://[^/@]*@#://#'), not $R, so this checkout is some other main"; return 1 ;;
  esac
  live=$(main_head)
  [ ${#live} -eq 40 ] || { echo "REFUSED: cannot read main's live head, so this checkout cannot be judged: ${live:-empty answer}"; return 1; }
  ref=$(git rev-parse -q --verify refs/remotes/origin/main) \
    || { echo "REFUSED: this checkout has no origin/main ref; main is ${live:0:9} on GitHub. Fetch it and check it out"; return 1; }
  [ "$ref" = "$live" ] \
    || { echo "REFUSED: local origin/main is ${ref:0:9}, main is ${live:0:9} on GitHub, so every range judged here is the wrong one. Fetch it and check it out"; return 1; }
  head=$(git rev-parse -q --verify HEAD) \
    || { echo "REFUSED: this checkout has no HEAD commit; expected main's head ${live:0:9}"; return 1; }
  if [ "$head" != "$live" ]; then
    on=$(git rev-parse --abbrev-ref HEAD); [ "$on" = HEAD ] && on="a detached HEAD"
    echo "REFUSED: HEAD is ${head:0:9} ($on), not main's head ${live:0:9}, so this would judge a tree main does not have"
    return 1
  fi
  dirty=$(git status --porcelain)
  [ -z "$dirty" ] || { echo "REFUSED: the working tree or index is not clean, so this checkout is not main at ${live:0:9}:"; printf '%s\n' "$dirty" | head -10 | sed 's/^/    /'; return 1; }
}

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
GATES='.github/workflows/swift-core-check.yml|Swift core check'

# gate_patterns <workflow> <ref>: every quoted path pattern under that workflow's top level `on:`
# block, read at <ref>.
#
# The push and pull_request lists are unioned. They are identical today, and a superset errs towards
# asking for a verdict, which is the safe direction. Unquoted entries yield nothing, and nothing is
# BLOCKED by the caller rather than "not required".
gate_patterns() {
  local body
  body=$(rd "repos/$R/contents/$1?ref=$2" --jq '.content' 2>/dev/null) || return 1
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

# gate_state <workflow> <sha>: one line — PASS <detail> | RED <detail> | NOVERDICT <detail> |
# PENDING <detail> | NONE | BLOCKED <what could not be read>.
#
# Read by head_sha, not from the PR's rollup. Two check runs on one head can share a job name (the
# push run's and the pull_request run's), and the rollup gives no way to tell which result belongs
# to which event. The runs API keyed on the sha is exact. Every page is read, and a short read is
# loud, for the reason #2467 documents.
#
# CONSTRAINT — what a run-level conclusion is allowed to mean, and why it is not this file's to
# guarantee. This reads the RUN's conclusion rather than a named check row. GitHub concludes a run
# `success` when every one of its jobs is success-or-skipped, so that equals a real verdict only
# while no job in the workflow can succeed without testing. Today that holds because the workflow
# has exactly one job, `check`. Once #2448 lands it holds for a different reason: the workflow ends
# in a job that fails closed — `verdict` ("Swift verdict") is `if: always()` and exits non-zero
# unless a check actually passed on this head. Without such a job a run whose check was skipped
# concludes `success`, which is precisely the bug this file was written to end.
#
# So the dependency lives in a YAML file nobody reviewing this one would think to open. If that job
# gains `continue-on-error: true`, or the workflow gains any other job that always succeeds, this
# function goes quietly permissive again without a line here changing. The matching constraint
# comment is on the workflow side. Reading rows by name instead would only trade this dependency for
# a worse one: `build-xcframework + fastlane test` is skipped in the mirror run exactly when the
# push twin passed, so requiring it by name would refuse when coverage exists.
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
gate_applies() {
  local wf=$1 sha=$2 files=$3 phead pbase pats
  phead=$(gate_patterns "$wf" "$sha"); pbase=$(gate_patterns "$wf" "${GATE_BASE:-main}")
  if [ -z "$phead" ] || [ -z "$pbase" ]; then
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

# require_gates <sha> <files>: print one indented finding per gate and return 0 only when every gate
# that applies to this head is satisfied. 1 = a gate has no passing run, 2 = a gate could not be read,
# 3 = a gate is still running. All three are refusals to a caller that fails closed; they are separate
# so the message can say which, and so "wait" does not read as "wrong".
#
# <files> is the newline-separated diff for this head, from pr_files or commit_files.
require_gates() {
  local sha=$1 files=$2 wf name phead pbase pats state rc=0
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
