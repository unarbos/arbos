#!/bin/bash
# merge-window.sh go [<pr-number>]  — pre-merge check: GO or NO-GO, with every reason. Fails closed.
# merge-window.sh after <prev-ref>  — post-merge check. <prev-ref> = the last verified point: the tag (or,
#                                     after a no-tag merge, the commit) the previous GO/AFTER cleared.
#                                     Prints WAIT, LIVE (new tag published), NO-TAG (Release plan judged it
#                                     "no Mac change", nothing to publish) or STOP.
# merge-window.sh verdict <sha>     — the Release plan's verdict for one commit on main.
# merge-window.sh checks <sha>  — the required-check reading for one head, on its own. Read only.
# merge-window.sh range-gate <from-ref> — the required checks GO applies to main since <from-ref>
#                                     (MW_TEST_HEAD=<sha> ends the range early, to replay one). Read only.
# Read only. Run from the repo checkout. Needs: gh (read), op (Arbos vault), check3.sh, checks.sh and
# loop-public.sh next to this file, and in $MERGE_KIT (default /tmp/k): config (ssh, relay host entry;
# LOOP_SRC=host only) and, optionally, watch-solo ("<feedback-id>:<recorded-head-sha>" pairs; a push to
# any of them is a NO-GO).
#
# Required checks are READ, from checks.sh, which names them. They are not inferred from
# mergeStateStatus, which cannot report on required checks because main has none: branches/main says
# "protected": false, and rulesets and rules/branches/main are both empty. An earlier version of this
# file inferred "checks green" from mergeStateStatus because the protection endpoint returns HTTP 403.
# The 403 is real, but there was nothing behind it, so the inference had no premise and UNSTABLE — the
# state a red or skipped check produces — was on its accept list.
# Rebase-merge is not allowed: one merge = one first-parent commit on main (merge commit or squash).
set -u
D=$(cd "$(dirname "$0")" && pwd); K=${MERGE_KIT:-/tmp/k}
source "$D/opread.sh"
source "$D/checks.sh"
R=unarbos/arbos
WATCH_SOLO=$(sed 's/#.*//' "$K/watch-solo" 2>/dev/null | tr -s ' \t\n' ' ')
ACTIVE="queued in_progress waiting requested pending"
nogo=()
ts() { date -u +%FT%TZ; }
newest_tag() { git ls-remote --tags origin 'v*' | awk '{print $2}' | grep -E '^refs/tags/v[0-9]+\.[0-9]+\.[0-9]+$' | sed 's|refs/tags/||' | sort -V | tail -1; }
# First-parent commits on main after <ref>, minus release bumps: the changes that landed.
changes_since() { git log --format='%H %s' --first-parent "$1..${MW_TEST_HEAD:-origin/main}" | grep -v ' release: '; }

# rd() lives in checks.sh, which both this file and merge.sh source.

# ci_abort_state <run-id> <"status conclusion">: RED or UNTESTED, for a ci.yml run
# that reached a terminal state other than success.
#
# Why this exists. Every non-success conclusion used to print the one sentence
# "RED CI <status> <conclusion>", so a job that tested the commit and found it
# broken and a job that died before testing anything were the same word. On
# 2026-11-04 the whole merge kit was held for about eight and a half hours on
# e445f70ad because of that: run 41761774906 ran 86 seconds, died at step 4 of
# 24 inside Swatinem/rust-cache@v2 with "No space left on device", and cargo
# fmt, clippy and test never executed. The tree was fine — #2704, whose base is
# that very commit, passed the same job twice the same morning — but the kit
# said "main is red at e445f70ad", which points at the commit rather than at a
# re-run button. Failing closed was right; the reason was wrong, and a wrong
# reason sends the reader to the wrong remedy.
#
# The discriminator is the step list, and it needs no workflow knowledge: a run
# that genuinely failed has at least one STEP whose conclusion is "failure". A
# run killed by its host has none — its steps are `null`, because they never
# started. So "no step concluded failure" is the signal, not the names of the
# steps, which keeps this true for workflows nobody has written yet.
#
# BOTH ANSWERS STILL REFUSE, and that is not an oversight to be tidied up
# later. A run that tested nothing is the definition of ungated, so it is
# precisely the state a gate must not pass. The tempting follow-up — an
# accept-infra= override, or an automatic pass when the steps are null — would
# rebuild the bug checks.sh was written to end, approaching from the other
# side. The remedy for an aborted run is to re-run it, which costs minutes and
# produces a real verdict. This function changes the sentence, never the gate.
#
# Unreadable steps fall back to RED: when the new read fails, the output is
# what it was before this function existed.
ci_abort_state() {
  local id=$1 ci=$2 steps note
  [ -n "$id" ] || { echo "RED CI $ci"; return; }
  # "\(.conclusion)" and not a bare .conclusion: gh renders a JSON null as an
  # EMPTY LINE, and a step that never started is exactly the null case. Every
  # such step is at the end of the list, so both command substitutions below
  # strip them and `steps` arrives holding only the steps that did run — the
  # ones this function exists to look past. Interpolating makes null arrive as
  # the four-character token, which survives the strip and is greppable.
  steps=$(rd "repos/$R/actions/runs/$id/jobs?per_page=100" --jq '.jobs[].steps[]? | "\(.conclusion)"') \
    || { echo "RED CI $ci (and its steps could not be read, so it is reported as before: ${steps#BLOCKED:})"; return; }
  [ -n "$steps" ] || { echo "RED CI $ci (run $id lists no steps at all)"; return; }
  printf '%s\n' "$steps" | grep -qx failure && { echo "RED CI $ci"; return; }
  # Nothing failed and it is not a success, so it stopped. The annotation is
  # usually the only place the host's own error appears.
  note=$(rd "repos/$R/actions/runs/$id/jobs?per_page=100" --jq '[.jobs[]|select(.conclusion!="success")][0].id' 2>/dev/null)
  case "$note" in ''|null|BLOCKED*) note="" ;; *) note=$(rd "repos/$R/check-runs/$note/annotations" --jq '.[0].message' 2>/dev/null) ;; esac
  case "$note" in BLOCKED*|null) note="" ;; esac
  echo "UNTESTED CI $ci — run $id has $(printf '%s\n' "$steps" | grep -cx null) step(s) that never started and none that failed, so it aborted before testing this commit; re-run it, do not re-push${note:+ — }${note:0:160}"
}

# passed_over <sha> <commits the plans named>: the first named commit that CONTAINS <sha> without
# being it, or empty. Non-empty means no future plan can name <sha>, because a plan names only the
# newest green commit and one newer than this has already been named.
#
# An abbreviation that does not resolve locally makes `--is-ancestor` fail, which yields empty and
# leaves the caller at WAIT. WAIT refuses, so an unreadable name costs a needless wait and never a
# clearance — the direction to fail in.
passed_over() {
  local sha=$1 n
  for n in $2; do
    [ "$n" = "${sha:0:${#n}}" ] && continue
    if git merge-base --is-ancestor "$sha" "$n" 2>/dev/null; then echo "$n"; return; fi
  done
}

# publish_job <release-run-id>: the id of that run's "What to publish" job, empty when the run
# planned nothing, rc=1 with the reason on stdout when the listing could not be read.
#
# A SKIPPED RUN'S MISSING LOG IS A FACT, NOT A FAILED READ, AND MUST NOT BLOCK.
# A Release run that was skipped still LISTS its jobs, so an "is there a job?" test does not catch
# it — the job is there, with conclusion `skipped`. What a skipped job has is no log, and GitHub
# answers `BlobNotFound`/404 for it. That 404 is the expected consequence of something the caller
# can already see (the run planned nothing, so it names no commit), not a failure to read.
#
# Blocking on it cost far more than the cause suggests. Runs are scanned newest-first and a BLOCKED
# returns out of the scan, so ONE skipped run hid every older run behind it — including the run
# carrying the answer. On 2026-11-04 three consecutive skipped plans sat at the top of the list and
# dd0d8db10 read BLOCKED permanently, because a run created after a commit never leaves that
# commit's `created>=` window. The whole merge queue was shut on it.
#
# The conclusion is read in the SAME call as the id, not a second one that could disagree with the
# first. The empty return and the rc=1 return are deliberately different channels: a caller that
# collapsed them would be back to treating a fact as a failure, or worse, the reverse.
publish_job() {
  local j
  j=$(rd "repos/$R/actions/runs/$1/jobs" --jq '.jobs[]|select(.name=="What to publish")|"\(.id) \(.conclusion)"') \
    || { echo "$j"; return 1; }
  j=$(echo "$j" | head -1)
  [ -n "$j" ] || return 0
  case "$j" in *" skipped") return 0 ;; esac
  echo "${j%% *}"
}

# verdict <sha> [nogate]: one line — TAG <vX> | NOMAC <vY> <reason> | COVERED <vY> | RED | UNTESTED | WAIT <why> | BLOCKED <unreadable>
# "nogate" leaves the Swift gate out, for a caller that judges the gate itself (go, through range_gate).
verdict() {
  local sha=$1 s8=${1:0:8} since ci tag
  # MW_TEST_HISTORY=1 / MW_TEST_UNTIL=<iso>: test-only, to replay an old commit's verdict.
  if [ -z "${MW_TEST_HISTORY:-}" ]; then
    for t in $(git tag --contains "$sha" 2>/dev/null | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' | sort -V | head -1); do echo "TAG $t"; return; done
  fi
  # The Swift gate, before anything else, because a green CI says nothing about the iPhone client
  # core: ci.yml's push trigger ignores apps/ios/**, and the gate is a separate workflow. This read
  # has to come before the NOPATH branch below, not after it — an apps/ios-only commit returns NOPATH
  # precisely because it has no CI, and those are the commits whose gate matters most. Nothing in this
  # kit used to read the gate at all, so a main commit whose gate was red, cancelled or never run
  # still earned NOMAC and cleared the merge window.
  if [ "${2:-}" != nogate ]; then
    local gfiles gout grc
    gfiles=$(commit_files "$sha") || { echo "BLOCKED reading the files $s8 changed: ${gfiles#BLOCKED:}"; return; }
    if files_capped "$gfiles"; then
      echo "BLOCKED $s8 changes 300 files or more, GitHub's listing cap, so whether a gate applies to it cannot be read"; return
    fi
    gout=$(require_gates "$sha" "$gfiles"); grc=$?
    gout=$(printf '%s' "$gout" | sed 's/^ *//' | paste -sd'; ' -)
    case $grc in
      1) echo "RED GATE $gout"; return ;;
      2) echo "BLOCKED $gout"; return ;;
      3) echo "WAIT $gout"; return ;;
    esac
  fi
  local cirun
  ci=$(rd "repos/$R/actions/workflows/ci.yml/runs?head_sha=$sha&per_page=5" --jq '[.workflow_runs[]|select(.event=="push")][0]|"\(.id) \(.status) \(.conclusion)"') || { echo "BLOCKED reading CI runs for $s8: ${ci#BLOCKED:}"; return; }
  # The id is only needed to look inside a non-success run; strip it back off so
  # every string below reads exactly as it did before.
  case "$ci" in ""|null*) cirun="" ;; *) cirun=${ci%% *}; ci=${ci#* } ;; esac
  case "$ci" in
    ""|null*)
      # ci.yml's push trigger has paths-ignore (apps/ios/**): such a commit gets no CI and so no Release
      # plan, by design. Only when every changed path is ignored per that commit's own ci.yml.
      local ign files left
      ign=$(git show "$sha:.github/workflows/ci.yml" 2>/dev/null | sed -n '/^  push:/,/^  [a-z_]*:$/p' \
        | sed -n 's/^ *paths-ignore: *\[\(.*\)\].*/\1/p' | tr -d '"'"'"' ' | tr ',' '\n' | grep -E '^[^*]+/\*\*$' | sed 's/\*\*$//')
      files=$(git diff --name-only "$sha^1" "$sha" 2>/dev/null)
      if [ -n "$ign" ] && [ -n "$files" ]; then
        left=$(printf '%s\n' "$files" | while IFS= read -r f; do m=0; while IFS= read -r g; do case "$f" in "$g"*) m=1;; esac; done <<< "$ign"; [ $m = 1 ] || echo "$f"; done)
        [ -z "$left" ] && { echo "NOPATH $(printf '%s** ' $ign | sed 's/ $//') only — ci.yml runs no push CI for it, so no Release plan and no Mac tag, by design"; return; }
      fi ;;
  esac
  case "$ci" in
    "completed success") ;;
    completed*) ci_abort_state "$cirun" "$ci"; return ;;
    ""|null*) echo "WAIT no CI run yet"; return ;;
    *) echo "WAIT CI $ci"; return ;;
  esac
  since=$(date -u -d "@$(git show -s --format=%ct "$sha")" +%Y-%m-%dT%H:%M:%SZ)
  local window=">=$since"; [ -n "${MW_TEST_UNTIL:-}" ] && window="$since..$MW_TEST_UNTIL"
  # Every page: the list grows all day (one scheduled run per 15 min), so page 1 alone loses the run that
  # judged an older commit. The total is read first, so a run created mid-read cannot fake a shortfall.
  local total list got runs scanned=0 namedlist=""
  total=$(rd "repos/$R/actions/workflows/release.yml/runs?created=$window&per_page=1" --jq '.total_count') || { echo "BLOCKED counting Release runs: ${total#BLOCKED:}"; return; }
  list=$(rd --paginate "repos/$R/actions/workflows/release.yml/runs?created=$window&per_page=100" \
      --jq '.workflow_runs[]|"\(.id) \(.event) \(.status) \(.head_sha)"') || { echo "BLOCKED listing Release runs: ${list#BLOCKED:}"; return; }
  got=$(printf '%s\n' "$list" | grep -c .)
  [ "$got" -ge "${total:-0}" ] || { echo "BLOCKED Release-run lookup incomplete for $s8: read $got of $total runs created since $since"; return; }
  # Runs on this commit first (its own workflow_run plan); the rest, newest first, cover schedule runs.
  runs=$(printf '%s\n' "$list" | awk -v s="$sha" '$2!="push" && $3=="completed" && $4==s {print $1}'
         printf '%s\n' "$list" | awk -v s="$sha" '$2!="push" && $3=="completed" && $4!=s {print $1}')
  for r in $runs; do
    scanned=$((scanned + 1))
    # Empty means "this run planned nothing, so it names no commit" — skip it. A failure to READ
    # still BLOCKs, below and unchanged. publish_job() is where those two are told apart.
    j=$(publish_job "$r") || { echo "BLOCKED listing jobs of Release run $r: ${j#BLOCKED:}"; return; }
    [ -n "$j" ] || continue
    log=$(rd --allow-escape-sequences "repos/$R/actions/jobs/$j/logs") || { echo "BLOCKED reading the 'What to publish' log of Release run $r: ${log#BLOCKED:}"; return; }
    log=$(printf '%s' "$log" | sed 's/\x1b\[[0-9;]*m//g')
    notes=$(echo "$log" | grep -oE ' notes=.*' | grep -v '"' | head -1 | sed 's/^ notes=//')
    tag=$(echo "$log" | grep -oE ' tag=v[0-9.]+' | head -1 | sed 's/^ tag=//')
    if [ -n "$tag" ] && git merge-base --is-ancestor "$sha" "$tag" 2>/dev/null; then echo "TAG $tag (run $r)"; return; fi
    case "$notes" in
      "no Mac change since "*"newest green commit $s8"*) echo "NOMAC $(echo "$notes" | awk '{print $5}') (run $r) $notes"; return ;;
      *"already covers the newest green commit $s8"*) echo "COVERED $(echo "$notes" | awk '{print $2}') (run $r)"; return ;;
    esac
    # Which commit this plan DID name. Collected for the NOPLAN test after the loop: a plan only
    # ever names the newest green commit, so a plan naming a commit that contains this one is
    # proof that this one has been passed over for good.
    case "$notes" in
      *"newest green commit "*) namedlist="$namedlist $(printf '%s' "${notes##*newest green commit }" | tr -cd '0-9a-f')" ;;
    esac
  done
  for st in $ACTIVE; do
    n=$(gh run list -R $R --workflow release.yml --status $st --limit 5 --json databaseId --jq length 2>&1) || { echo "BLOCKED listing $st Release runs: $(echo "$n" | tr '\n' ' ' | cut -c1-160)"; return; }
    [ "${n:-0}" -gt 0 ] && { echo "WAIT Release run $st"; return; }
  done
  # NOPLAN vs WAIT: whether the backstop can still reach this commit, or never will.
  #
  # A Release plan names exactly one commit — the newest green one at the moment it runs. So a
  # commit that was green but was buried by newer green commits before any plan named it is never
  # named by a later plan either: the backstop will keep naming the newest. Saying "the 15-min
  # backstop judges it next" to that commit is worse than saying nothing, because it buys a wait
  # instead of an action, and the wait never ends. On 2026-11-04 dd0d8db10 sat in exactly that
  # state — four pushes inside six minutes buried it before a plan ran.
  #
  # The test is a measurement over the plans actually read, not an assumption about ordering: if
  # any plan named a commit that CONTAINS this one, this one has been passed over for good. An
  # abbreviation that does not resolve locally fails `--is-ancestor` and leaves the verdict at
  # WAIT, which refuses — the safe direction.
  local newer; newer=$(passed_over "$sha" "$namedlist")
  if [ -n "$newer" ]; then
    echo "NOPLAN no Release plan has judged ${s8}, and none ever will: read all $got Release runs created since $since (every page); the newest green commit they name is ${newer}, which contains ${s8}, and a plan only ever names the newest green commit"
    return
  fi
  echo "WAIT no Release plan has judged ${s8} yet: read all $got Release runs created since $since (every page), none of the $scanned completed non-push plans names it (the 15-min backstop judges it next)"
}

# range_gate <from>: each gate in checks.sh, judged for what main would ship on ONE commit — the newest
# first-parent commit since <from> whose diff the gate applies to. Every later commit touches none of
# the gate's paths, so that commit's gated content is main's head's. Applying the gate to the head
# commit literally would not work: a head that touches no gated path gets no gate run at all.
#
# An older commit's gate result is printed as a finding, never a refusal. On main a queued gate run is
# cancelled when the next one is created (260 of 264 cancelled runs, median gap one second), so a rule
# that every commit since the tag be green could essentially never be met. A release ships the head's
# tree, and an intermediate red is evidence for blame and bisection, not for ship safety.
#
# What counts as a verdict on the deciding commit does not change: a completed successful run on that
# exact sha. Pending is WAIT; skipped, cancelled, red or absent refuses; anything unreadable is BLOCKED.
# A commit newer than the deciding one whose gate applicability cannot be read is BLOCKED too, because
# then the deciding commit itself is unknown.
#
# Returns 0 satisfied (or no gate applies since <from>), 1 refused, 2 blocked, 3 wait.
range_gate() {
  local ch wf name rc=0 g sha subj files why state decided unasked
  ch=$(changes_since "$1")
  while IFS='|' read -r wf name; do
    [ -n "$wf" ] || continue
    g=0; decided=""; unasked=""
    while read -r sha subj; do
      [ -n "$sha" ] || continue
      if ! files=$(commit_files "$sha"); then why="BLOCKED — cannot read the files it changed: ${files#BLOCKED:}"
      elif files_capped "$files"; then why="BLOCKED — it changes 300 files or more, GitHub's listing cap"
      else why=$(gate_applies "$wf" "$sha" "$files"); case $? in 0) why="" ;; 1) continue ;; esac
      fi
      if [ -n "$why" ]; then
        if [ -z "$decided" ]; then
          echo "    '$name' on ${sha:0:9}: $why, so which commit decides this gate is unknown"
          g=2; decided=$sha
        else echo "    '$name' on ${sha:0:9} (older, a finding, not a refusal): $why"; fi
        continue
      fi
      # landed_gate_state, not gate_state: these commits are already on main, so a pull_request-only
      # gate's run sits on the merge commit's second parent, and a squash has none at all. See its
      # header in checks.sh. NOTASKED is a finding — it does not decide the gate and does not refuse.
      state=$(landed_gate_state "$wf" "$sha")
      case "$state" in
        NOTASKED*) echo "    '$name' on ${sha:0:9}: ${state#NOTASKED }"; unasked=$sha; continue ;;
      esac
      if [ -z "$decided" ]; then
        decided=$sha
        case "$state" in
          PASS*)    echo "    '$name' DECIDED on ${sha:0:9} (the newest commit it applies to): $state" ;;
          PENDING*) echo "    '$name' DECIDED on ${sha:0:9} (the newest commit it applies to): NO VERDICT YET — $state"; g=3 ;;
          BLOCKED*) echo "    '$name' DECIDED on ${sha:0:9} (the newest commit it applies to): $state"; g=2 ;;
          NONE)     echo "    '$name' DECIDED on ${sha:0:9} (the newest commit it applies to): NO VERDICT — no run of $wf exists for this sha at all"; g=1 ;;
          *)        echo "    '$name' DECIDED on ${sha:0:9} (the newest commit it applies to): NO VERDICT — $state"; g=1 ;;
        esac
      else
        case "$state" in
          PASS*) echo "    '$name' on ${sha:0:9} (older): $state" ;;
          *)     echo "    '$name' on ${sha:0:9} (older, a finding, not a refusal): $state" ;;
        esac
      fi
    done <<< "$ch"
    # "applies to no commit" and "applies to commits it cannot be asked about" are different facts,
    # and printing the first for the second is the same substitution this gate's own fix is about.
    if [ -z "$decided" ] && [ -n "$unasked" ]; then
      echo "    '$name': every commit since $1 that it applies to is one it cannot be asked about, named above"
    elif [ -z "$decided" ]; then
      echo "    '$name': applies to no commit since $1"
    fi
    case $g in
      2) rc=2 ;;
      1) [ $rc -eq 2 ] || rc=1 ;;
      3) [ $rc -eq 0 ] && rc=3 ;;
    esac
  done <<< "$GATES"
  return $rc
}

loop_gating() {
  # The relay key goes from the vault straight into a private, minimal-environment ssh-agent (never a file)
  # that lives only for this check.
  local sock apid rc; sock=$(mktemp -u "$K/agent.XXXXXX")
  apid=$(env -i PATH=/usr/bin:/bin ssh-agent -a "$sock" -s | sed -n 's/^SSH_AGENT_PID=\([0-9]*\);.*/\1/p')
  [ -n "$apid" ] || { echo "ssh-agent did not start"; echo "rc=1"; return; }
  opread 'op://Arbos/nlijfp36ed4aqbkp2svh2lefbi/private key?ssh-format=openssh' | SSH_AUTH_SOCK=$sock ssh-add -q -t 180 - 2>/dev/null
  timeout 60 ssh -F "$K/config" -o IdentityAgent="$sock" -o IdentitiesOnly=no -o IdentityFile=none relay 'sudo -n /usr/bin/python3 - <<EOF
import json, os
d="/var/lib/arbos-feedback"
for fid in sorted(os.listdir(d)):
    p=f"{d}/{fid}/state.json"
    if not os.path.isfile(p): continue
    try: st=json.load(open(p))
    except Exception: continue
    if st.get("status")!="gating": continue
    dnm=str(st.get("do_not_merge") or "")
    held_by_operator = bool(dnm) and not dnm.startswith("serialize-on-release")
    print(("OPERATOR-HELD" if held_by_operator else "RELEASABLE"), fid, "#%s"%st.get("issue"), (st.get("branch_head") or "")[:8], dnm[:80])
EOF' 2>&1
  rc=$?
  kill "$apid" 2>/dev/null; rm -f "$sock"
  echo "rc=$rc"
}

check_published() {  # <tag> — appends to nogo
  local tag=$1 out
  out=$(bash "$D/check3.sh" "${tag#v}" 2>&1); echo "$out" | sed 's/^/    /'
  echo "$out" | grep -q '^release: draft=false prerelease=false published_at=2' || nogo+=("$tag release object")
  echo "$out" | grep -q '= GitHub digest' || nogo+=("$tag DMG")
  echo "$out" | grep -q '^feed (authority, release asset): lists' || nogo+=("$tag feed (authority)")
  echo "$out" | grep -q "^latest release: $tag$" || nogo+=("$tag is not GitHub's latest release")
  echo "$out" | grep -q '^feed (mirror, arbos.life): lists' || echo "    (mirror behind or unreachable: soft, not a NO-GO)"
}

# nomac_covers <sha> <covering-sha>: 0 when <covering-sha> has already been judged
# "no Mac change since the tag being released" AND <sha> is an ancestor of it.
#
# WHY AN ANCESTOR MAY BE CLEARED BY A DESCENDANT'S VERDICT. The question this
# window asks is whether main still OWES a Mac release. mac-change.sh answers it
# by diffing the tag against a commit, so "no Mac change since <tag>" on commit T
# means the Mac paths at T are identical to the Mac paths at <tag>. If that holds
# at T, it holds for everything main would ship from T: a release cut there would
# rebuild a byte-identical Mac app. An ancestor of T that touched a Mac path and
# had it reverted before T is a transient that no release ever needed to carry,
# and the debt is about what main owes now, not about what it briefly held.
#
# WHY THIS IS NOT A LOOSENING. <covering-sha> is only ever set from a verdict that
# matched "NOMAC <the current tag>" exactly — a real Release run's recorded answer,
# against the tag being released, not an inference. If genuine debt exists then
# mac-change.sh returns 1 for the tip, so the tip's plan says "Mac sources changed"
# and cuts a tag or waits; it never records NOMAC, nothing ever sets <covering-sha>,
# and every refusal below fires exactly as it did before. The clearing path cannot
# be reached without a current-tag NOMAC somewhere newer on main's first-parent line.
#
# THE DEFECT THIS REPAIRS. The check used to demand that EVERY commit since the tag
# carry a verdict naming THAT tag. A verdict is recorded when its Release run
# finishes, so a commit judged minutes before the next tag publishes is stamped with
# the older tag and can never be re-stamped: nothing re-judges a commit already on
# main, no new commit can land while the window is shut, and the tag cannot move
# without a release that nothing needs. On 2026-11-04 that deadlocked the whole queue
# on 562dc0cd0, whose run finished at 13:35Z carrying "NOMAC v0.6.381" while v0.6.382
# published at 14:00Z — with zero Mac paths changed across the entire range.
#
# The explicit empty check below is belt-and-braces and survives mutation on
# purpose: git already rejects an empty revision, so deleting the line changes
# no answer this function can be asked for. It is kept because it states the
# precondition where a reader looks for it, and because it makes a one-argument
# call return 1 rather than die on an unbound variable under `set -u`.
# THE TWO PRECONDITIONS, STATED IN THE SIGNATURE RATHER THAN ASSUMED.
# <tag> is taken as an argument and not read from the caller's scope so that the anchor is
# asserted and not assumed: a NOMAC anchored at v0.6.381 settles nothing about v0.6.382, and the
# range below is what makes that structural instead of a convention the next edit can lose.
# Ancestry is MEASURED along the first-parent line, not inferred from the order go() happens to
# walk in. changes_since() is `--first-parent`, so every commit this is asked about is on that
# line already; measuring it anyway costs nothing and removes the dependency on that staying true.
# It is also strictly tighter than `merge-base --is-ancestor`, which would clear a commit reachable
# only through a merge's second parent — never a commit go() offers, and not one to start clearing.
nomac_covers() {
  [ -n "${2:-}" ] || return 1
  [ -n "${3:-}" ] || return 1
  git rev-list --first-parent "$3..$2" 2>/dev/null | grep -qx "$1"
}

go() {
  echo "== merge-window GO check $(ts)"
  git fetch -q origin main --tags 2>/dev/null
  local busy tag
  busy=$( { for st in $ACTIVE; do
      gh run list -R $R --workflow release.yml --status $st --limit 20 --json databaseId,event,headBranch --jq ".[]|\"release.yml \(.databaseId) $st \(.event) \(.headBranch)\""
      gh run list -R $R --workflow ci.yml --branch main --status $st --limit 20 --json databaseId,event --jq ".[]|\"ci.yml(main) \(.databaseId) $st \(.event)\""
    done; } 2>&1)
  if [ -n "$busy" ]; then echo "1 in flight: NO"; echo "$busy" | sed 's/^/    /'; nogo+=("CI/Release in flight"); else echo "1 in flight: none"; fi
  tag=$(newest_tag); echo "2 newest tag: $tag"
  check_published "$tag"
  local ch; ch=$(changes_since "$tag")
  if [ -z "$ch" ]; then echo "  main $(git rev-parse --short=9 origin/main) is covered by $tag"
  else
    echo "  main has changes after $tag; each needs a Release plan verdict of 'no Mac change since $tag':"
    local cls covering=""
    while read -r sha subj; do
      v=$(verdict "$sha" nogate); echo "    ${sha:0:9} ${subj:0:70} → $v"
      cls=$(verdict_class "$v")
      # Which classes may clear is verdict_clears_go()'s to say, so that the
      # answer lives in one place and selftest.sh is pinning the real decision
      # rather than a copy of it. This used to be a literal `case` here, which
      # meant the kit had two lists that had to agree about refusal — the exact
      # shape of bug the rest of this file is about.
      #
      # The tag check stays here, and must. verdict_clears_go() is deliberately
      # ignorant of which tag is being released, so it answers for the class and
      # not for this release: a NOMAC naming an OLDER tag says nothing about
      # this one ON ITS OWN, and still falls through to the NO-GO below unless a
      # newer commit that contains it carries a verdict naming THIS tag.
      #
      # changes_since() lists newest first, so `covering` is set by the time any
      # older commit is read. It is assigned here and nowhere else, from the exact
      # "NOMAC $tag" match and from nothing weaker — see nomac_covers() above.
      if verdict_clears_go "$cls"; then
        case "$cls" in
          NOMAC)
            case "$v" in "NOMAC $tag"*) covering=${covering:-$sha}; continue ;; esac
            if nomac_covers "$sha" "$covering" "$tag"; then
              echo "      ^ cleared: ${covering:0:9} is judged 'no Mac change since $tag' and ${sha:0:9} is an ancestor of it, so main owes no Mac release for it"
              continue
            fi ;;
          # NOPLAN needs its OWN arm and must never reach the `*)` below, which clears
          # unconditionally. The clearance here is the same inference NOMAC uses, and it is the
          # whole reason this class exists: "a descendant's NOMAC settles the Mac-release question
          # for everything it contains" does not consult the covered commit's verdict at all.
          # NOMAC relative to $tag means no Mac path changed between $tag and the covering commit,
          # so no commit on the first-parent path between them changed one either. Gating that on
          # the covered commit's own class was a category error — it attached the condition to the
          # wrong commit. Without a covering commit this still refuses, which is the point.
          NOPLAN)
            if nomac_covers "$sha" "$covering" "$tag"; then
              echo "      ^ cleared: no plan will ever name ${sha:0:9}, but ${covering:0:9} is judged 'no Mac change since $tag' and contains it on main's first-parent line, so main owes no Mac release for it"
              continue
            fi ;;
          *) continue ;;
        esac
      fi
      # One NO-GO either way; the words differ because the next action does.
      # "not judged" used to head every one of these, so a commit the kit HAD
      # judged, and judged failed, was reported as missing information with the
      # real verdict demoted into the parenthetical.
      case "$cls" in
        RED)      nogo+=("${sha:0:9} JUDGED AND FAILED CI since $tag — read the run ($v)") ;;
        UNTESTED) nogo+=("${sha:0:9} JUDGED, BUT NOTHING RAN since $tag — re-run the job, the commit is not implicated ($v)") ;;
        REDGATE)  nogo+=("${sha:0:9} has a required check with no successful run since $tag ($v)") ;;
        WAIT)     nogo+=("${sha:0:9} NOT JUDGED YET since $tag — still running ($v)") ;;
        # Distinct from WAIT because waiting is the one thing that cannot help. No plan will
        # name this commit, and no commit newer than it carries a '$tag' NOMAC containing it,
        # so the only moves are a release that covers it or a plan run against it directly.
        NOPLAN)   nogo+=("${sha:0:9} WILL NEVER BE JUDGED since $tag — do not wait for it; nothing newer on main carries a '$tag' no-Mac-change verdict that contains it ($v)") ;;
        BLOCKED)  nogo+=("${sha:0:9} COULD NOT BE READ since $tag, so nothing is inferred ($v)") ;;
        # Name both tags. This refusal used to print only the tag being released,
        # leaving the reader to find the other one inside the parenthetical and work
        # out for themselves that the two differed.
        NOMAC)    nogo+=("${sha:0:9} was judged 'no Mac change since $(echo "$v" | awk '{print $2}')', NOT since $tag, and no commit newer than it on main carries a '$tag' verdict that contains it — so whether main owes a Mac release for ${sha:0:9} is unanswered ($v)") ;;
        *)        nogo+=("${sha:0:9} not judged no-Mac-change since $tag ($v)") ;;
      esac
    done <<< "$ch"
    echo "  required checks since $tag, decided on the newest commit each applies to (older results are findings):"
    range_gate "$tag"
    case $? in
      1) nogo+=("a required check has no successful run on the commit that decides it") ;;
      2) nogo+=("a required check could not be read, so it is not inferred") ;;
      3) nogo+=("a required check is still running on the commit that decides it") ;;
    esac
  fi
  # LOOP_SRC=public (default) needs no vault read and no SSH; LOOP_SRC=host reads the poller's state on the relay.
  if [ "${LOOP_SRC:-public}" = host ]; then
    echo "3 loop items in gating (relay host, read only):"
    lg=$(loop_gating); echo "$lg" | sed 's/^/    /'
    echo "$lg" | grep -q '^rc=0$' || nogo+=("relay check failed")
    echo "$lg" | grep -q '^RELEASABLE' && nogo+=("a loop item the timer can release is gating")
  else
    echo "3 loop landings (public: gate/* runs and ungated feedback branches on GitHub; no credential):"
    lg=$(bash "$D/loop-public.sh" 2>&1 | grep -v '^warning: '); echo "$lg" | sed 's/^/    /'
    echo "$lg" | grep -q '^LOOP-PUBLIC: CLEAR' || nogo+=("a loop landing may be in flight (public check)")
  fi
  for pair in $WATCH_SOLO; do
    fid=${pair%%:*}; rec=${pair#*:}
    live=$(gh api repos/$R/git/refs/heads/feedback/$fid --jq .object.sha 2>/dev/null)
    if [ "$live" = "$rec" ]; then echo "    solo slot $fid: no push (head ${rec:0:8})"
    else echo "    solo slot $fid: PUSHED or changed (recorded ${rec:0:8}, live ${live:-none})"; nogo+=("push to solo slot $fid"); fi
  done
  if [ -n "${1:-}" ]; then
    echo "4 PR #$1 (required checks are read from checks.sh, not inferred):"
    gh pr view "$1" -R $R --json title,headRefName,headRefOid,isDraft,mergeable,baseRefName \
      --jq '"    \(.title)\n    \(.headRefName)@\(.headRefOid[0:9]) base=\(.baseRefName) draft=\(.isDraft) mergeable=\(.mergeable)"'
    gh pr checks "$1" -R $R 2>&1 | awk -F'\t' '{print "    "$1": "$2}' | sort | uniq -c | sed 's/^/  /' | head -30
    # mergeable is about conflicts and draft state, which are facts about the branch. It is not a
    # statement about checks: see the header.
    local ms; ms=$(gh pr view "$1" -R $R --json mergeable,isDraft --jq '"\(.mergeable) \(.isDraft)"')
    case "$ms" in "MERGEABLE false") ;; *) nogo+=("PR #$1 is not mergeable or is a draft ($ms)") ;; esac
    local phead pfiles
    phead=$(gh pr view "$1" -R $R --json headRefOid --jq .headRefOid)
    pfiles=$(pr_files "$1")
    if [ -z "$pfiles" ] || [ "${pfiles#BLOCKED}" != "$pfiles" ]; then
      echo "    required checks: BLOCKED — cannot read #$1's changed files"; nogo+=("PR #$1 required checks unreadable")
    else
      require_gates "$phead" "$pfiles" || nogo+=("PR #$1 has a required check with no passing run on $phead")
    fi
  fi
  echo
  if [ ${#nogo[@]} -eq 0 ]; then echo "VERDICT $(ts): GO — ONE merge (merge commit or squash, not rebase), within ~5 min; then nothing until 'after $( [ -z "$ch" ] && echo "$tag" || git rev-parse --short=9 origin/main)' clears"
  else echo "VERDICT $(ts): NO-GO — ${nogo[*]}"; fi
}

after() {
  local prev=$1 ch n sha subj v tag lasttag
  git fetch -q origin main --tags 2>/dev/null
  echo "== merge-window AFTER check $(ts): last verified point $prev"
  ch=$(changes_since "$prev"); n=$(printf '%s' "$ch" | grep -c .)
  if [ "$n" -eq 0 ]; then echo "VERDICT: WAIT — nothing merged since $prev"; return; fi
  echo "changes since $prev (first-parent, excluding release bumps):"; echo "$ch" | cut -c1-140 | sed 's/^    //; s/^/    /'
  if [ "$n" -ne 1 ]; then echo "VERDICT $(ts): STOP — $n changes since $prev, FOLDED"; return; fi
  read -r sha subj <<< "$ch"
  v=$(verdict "$sha"); echo "Release plan verdict for ${sha:0:9}: $v"
  # Do not fold this onto verdict_class(): `RED GATE ...` reaches RED* by prefix
  # and there is no REDGATE arm here, so classifying first would drop it into the
  # default — the refusal-becomes-a-shrug this case was hardened against.
  # Prerequisite for the fold: add a REDGATE arm, and a selftest case for it, first.
  case "$v" in
    TAG*)
      tag=$(echo "$v" | awk '{print $2}')
      lasttag=$(git tag -l 'v[0-9]*.[0-9]*.[0-9]*' --merged "$tag^" | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' | sort -V | tail -1)
      local earlier; earlier=$(git log --format='%h %s' --first-parent "$lasttag..$prev" 2>/dev/null | grep -v ' release: ' | cut -c1-120)
      echo "tag $tag (previous tag $lasttag); earlier no-tag changes also in its range: ${earlier:+$'\n'}${earlier:-(none)}" | sed '2,$s/^/    /'
      check_published "$tag"
      if [ ${#nogo[@]} -eq 0 ]; then echo "VERDICT $(ts): LIVE — $tag carries exactly one change since $prev; next GO may run (next 'after $tag')"
      else echo "VERDICT $(ts): NOT LIVE YET / STOP — ${nogo[*]}"; fi ;;
    NOMAC*)
      echo "VERDICT $(ts): NO-TAG — the Release plan judged ${sha:0:9} 'no Mac change'; nothing to publish. Next GO may run (next 'after ${sha:0:9}')" ;;
    NOPATH*)
      echo "VERDICT $(ts): NO-TAG — ${sha:0:9} changes ${v#NOPATH }. Next GO may run (next 'after ${sha:0:9}')" ;;
    COVERED*) echo "VERDICT $(ts): STOP — plan says an existing tag already covers ${sha:0:9}; look before the next merge" ;;
    RED*) echo "VERDICT $(ts): STOP — main is red at ${sha:0:9} ($v)" ;;
    # STOP, exactly as RED is, and said separately because the remedy is not
    # the same one. Without this arm UNTESTED would fall to the default below
    # and print WAIT, which is this function telling the operator to sit and
    # wait for a run that has already finished and will never change.
    UNTESTED*) echo "VERDICT $(ts): STOP — main's CI at ${sha:0:9} aborted before it tested anything; re-run that job ($v)" ;;
    # STOP and not WAIT, for the same reason UNTESTED is. after() judges one commit with no
    # notion of a covering descendant, so it cannot do go()'s clearing; what it must not do is
    # send the operator to wait for a plan that will never run.
    NOPLAN*) echo "VERDICT $(ts): STOP — no Release plan will ever judge ${sha:0:9}; a GO can clear it only from a newer commit's no-Mac-change verdict ($v)" ;;
    BLOCKED*) echo "VERDICT $(ts): BLOCKED — cannot read what the verdict needs, so no verdict is inferred: ${v#BLOCKED }" ;;
    WAIT*) echo "VERDICT $(ts): WAIT — $v" ;;
    # Unrecognised. WAIT used to live here as the default, which meant any
    # verdict word nobody had taught this function became an instruction to
    # sit and wait. That is the wrong direction to fail in: the words that
    # reach here describe states this function has never been shown, and a
    # state nobody has reasoned about is not a state to proceed from. Every
    # string verdict() can produce today has an arm above, so this arm is
    # reachable only by a word added without reading this case — exactly the
    # case that must not be allowed to read as progress.
    *) echo "VERDICT $(ts): STOP — unrecognised verdict, so nothing is inferred from it: $v" ;;
  esac
}

# open_pr_headed_by <full-sha>: the number of the open pull request whose HEAD is that sha, or
# empty. rc=1 with the reason on stdout when the listing could not be read.
#
# Both halves of the discriminator are load-bearing. `state == open` alone matches a merged or
# closed pull request that once contained the commit, and the association endpoint lists those
# too. `head.sha == sha` alone matches nothing useful on a sha that is merely *in* a branch. Only
# a commit that is an open pull request's tip is a sha whose one-argument reading is wrong.
open_pr_headed_by() {
  rd "repos/$R/commits/$1/pulls?per_page=100" \
    --jq "[.[]|select(.state==\"open\" and .head.sha==\"$1\")|.number]|first // empty"
}

# open_pr_for_branch <branch>: the number of the open pull request whose HEAD BRANCH is <branch>,
# or empty. rc=1 with the reason on stdout when the listing could not be read.
open_pr_for_branch() {
  rd "repos/$R/pulls?state=open&head=${R%%/*}:$1&per_page=1" --jq '.[0].number // empty'
}

# stack_chain <pr>: the base chain from <pr> upward, newest first, space separated, ending at the
# pull request whose base is main. Three non-ordinary endings, each printed as the last field:
#
#   UNRESOLVED:<branch>  a base branch that no OPEN pull request produces. The walk cannot say
#                        what reaches main, so the caller must refuse rather than report a root.
#   CYCLE                the chain loops back on itself.
#   DEPTH                more hops than any real stack, so the walk stops rather than spinning.
#
# rc=1 with the reason on stdout when a read failed. A failed read is never an ending.
#
# Why a chain and not just the base. Six open stacks are deeper than one hop and two are four
# deep (#2606 → #2602 → #2597 → #2554, #2680 → #2669 → #2668 → #2667). A pre-flight that names
# the immediate base stops inside the stack, which is exactly the case it exists to catch.
stack_chain() {
  local pr=$1 seen="" base next hops=0
  while :; do
    printf '%s ' "$pr"
    base=$(rd "repos/$R/pulls/$pr" --jq '.base.ref') || { echo "BLOCKED reading the base of #$pr: ${base#BLOCKED:}"; return 1; }
    [ "$base" = main ] && return 0
    case " $seen " in *" $pr "*) echo "CYCLE"; return 0 ;; esac
    seen="$seen $pr"
    hops=$((hops + 1))
    [ "$hops" -gt 20 ] && { echo "DEPTH"; return 0; }
    next=$(open_pr_for_branch "$base") || { echo "BLOCKED resolving '$base' to an open pull request: ${next#BLOCKED:}"; return 1; }
    [ -n "$next" ] || { echo "UNRESOLVED:$base"; return 0; }
    pr=$next
  done
}

# checks <sha> [<pr>]: the required-check reading for one head on its own, so it can be checked
# against a known commit without running a GO. With <pr>, the diff is that PR's three-dot diff;
# without it, the commit's diff against its first parent.
#
# Everything this function prints is scoped, and until 2026-11-04 none of it said so. Three
# separate readings overstated themselves, all in the same direction and all in the message
# rather than in the logic:
#
#   1. the one-argument form on an open pull request's head read the wrong diff entirely;
#   2. a stacked pull request's reading covered its own diff, not what reaches main;
#   3. `SATISFIED` claimed a successful run even when no gate had applied and nothing had run.
#
# A reader acts on the last line. If the last line overstates, the reading is worse than no
# reading, because it carries the authority of an instrument.
checks() {
  local sha=$1 pr=${2:-} files full
  # The one place a ref is allowed to be loose, because a person types it. require_gates needs the
  # full sha and refuses anything else, so resolve first rather than letting an abbreviation become
  # a confident wrong answer about whether a run exists.
  full=$(rd "repos/$R/commits/$sha" --jq '.sha') || { echo "BLOCKED resolving '$sha' to a full sha: ${full#BLOCKED:}"; return 2; }
  sha=$full
  if [ -n "$pr" ]; then
    files=$(pr_files "$pr")
  else
    # The one-argument form reads the commit's diff against its FIRST PARENT. On a pull request's
    # head that is the wrong question, and the two answers are not merely different sizes — they
    # can be disjoint. Measured on feca2ce5c, #2682's head: the two-argument form reads 1 file,
    # crates/arbos-agentd/src/web.rs; the one-argument form reads 9, main's incoming side from
    # #2671, and web.rs is not among them. Both happened to reach the same verdict, because
    # neither set trips a filtered path. That coincidence is the hazard, not the reassurance.
    local owner_pr
    owner_pr=$(open_pr_headed_by "$sha") \
      || { echo "BLOCKED checking whether ${sha:0:9} heads an open pull request: ${owner_pr#BLOCKED:}"; return 2; }
    if [ -n "$owner_pr" ]; then
      echo "REFUSE — ${sha:0:9} is the head of open pull request #$owner_pr, so the one-argument form reads the wrong diff for it:"
      printf '    %-22s %s\n' \
        "what you ran"          "checks ${sha:0:9}" \
        "what that reads"       "the commit's diff against its first parent" \
        "what the gate filters" "#$owner_pr's three-dot diff — what merging it actually adds to main" \
        "what to run instead"   "checks ${sha:0:9} $owner_pr"
      return 1
    fi
    files=$(commit_files "$sha")
  fi
  [ -n "$files" ] && [ "${files#BLOCKED}" = "$files" ] || { echo "BLOCKED reading changed files: ${files#BLOCKED:}"; return 2; }
  # GitHub caps a commit's file listing at 300. A list at the cap is "cannot tell", never a short
  # answer that happens to miss the gated path. verdict() has guarded this since it was written;
  # this path did not, which made the operator subcommand the laxer of the two readers.
  if files_capped "$files"; then
    echo "BLOCKED ${sha:0:9} changes 300 files or more, GitHub's listing cap, so whether a gate applies to it cannot be read"
    return 2
  fi
  echo "required checks for ${sha:0:9}, $(printf '%s\n' "$files" | grep -c .) file(s) changed:"
  require_gates "$sha" "$files"; local rc=$?
  local applied=${GATES_APPLIED:-0}

  # The stack reading. A pull request based on anything but main does not merge into main, so its
  # own clean reading is accurate and scoped — and nothing above says so.
  if [ -n "$pr" ]; then
    local chain cls root
    chain=$(stack_chain "$pr")
    cls=$(stack_class "$chain" "$pr")
    root=$(printf '%s' "$chain" | awk '{print $NF}')
    if stack_refuses "$cls"; then
      # The reading above is real and partial, and there is no second reading to complete it.
      case "$cls" in
        UNRESOLVED) echo "    stack: #$pr sits on '${root#UNRESOLVED:}', which no open pull request produces, so what reaches main cannot be determined" ;;
        *)          echo "    stack: the base chain from #$pr does not terminate ($cls), so what reaches main cannot be determined" ;;
      esac
      [ $rc -eq 2 ] || rc=1
    else
      case "$cls" in
        BLOCKED) echo "    stack: $chain"; rc=2 ;;
        SELF)    : ;;  # based on main: the one case where this head's reading is the whole story
        *)
          echo "    stack: #$pr is $(printf '%s' "$chain" | wc -w | tr -d ' ') deep — $(printf '%s' "$chain" | sed 's/ *$//; s/ / → #/g; s/^/#/') — and the reading above covers only #$pr's own diff"
          local rhead rfiles rout rrc
          rhead=$(rd "repos/$R/pulls/$root" --jq '.head.sha')
          if [ "${rhead#BLOCKED}" != "$rhead" ]; then
            echo "    stack root #$root: BLOCKED — ${rhead#BLOCKED:}"; rc=2
          else
            rfiles=$(pr_files "$root")
            if [ -z "$rfiles" ] || [ "${rfiles#BLOCKED}" != "$rfiles" ]; then
              echo "    stack root #$root: BLOCKED — cannot read its changed files"; rc=2
            else
              rout=$(require_gates "$rhead" "$rfiles"); rrc=$?
              case $rrc in
                0) if [ "${GATES_APPLIED:-0}" -eq 0 ]; then
                     echo "    stack root #$root on ${rhead:0:9}: SATISFIED VACUOUSLY — no path-filtered gate applies to it either"
                   else
                     echo "    stack root #$root on ${rhead:0:9}: SATISFIED — every gate that applies to it has passed"
                   fi ;;
                1) echo "    stack root #$root on ${rhead:0:9}: REFUSE — it has a gate with no passing run" ;;
                2) echo "    stack root #$root on ${rhead:0:9}: BLOCKED — a gate could not be read" ;;
                3) echo "    stack root #$root on ${rhead:0:9}: WAIT — a gate on it is still running" ;;
              esac
              printf '%s\n' "$rout" | sed 's/^    /        /'
            fi
          fi
        ;;
      esac
    fi
  fi

  echo "RESULT: $(result_line "$rc" "$applied")"
  return $rc
}

case "${1:-}" in
  go) shift; go "$@" ;;
  after) after "$2" ;;
  verdict) verdict "$2" ;;
  checks) checks "$2" "${3:-}" ;;
  range-gate) git fetch -q origin main --tags 2>/dev/null; range_gate "$2" ;;
  *) echo "usage: merge-window.sh go [<pr>] | after <prev-ref> | verdict <sha> | checks <sha> [<pr>] | range-gate <from-ref>"; exit 2 ;;
esac
