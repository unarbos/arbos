#!/bin/bash
# merge-window.sh go [<pr-number>]  — pre-merge check: GO or NO-GO, with every reason. Fails closed.
# merge-window.sh after <prev-ref>  — post-merge check. <prev-ref> = the last verified point: the tag (or,
#                                     after a no-tag merge, the commit) the previous GO/AFTER cleared.
#                                     Prints WAIT, LIVE (new tag published), NO-TAG (Release plan judged it
#                                     "no Mac change", nothing to publish) or STOP.
# merge-window.sh verdict <sha>     — the Release plan's verdict for one commit on main.
# merge-window.sh checks <sha>  — the required-check reading for one head, on its own. Read only.
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

# verdict <sha>: one line — TAG <vX> | NOMAC <vY> <reason> | COVERED <vY> | RED | WAIT <why> | BLOCKED <unreadable>
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
  local gfiles gout grc
  gfiles=$(commit_files "$sha") || { echo "BLOCKED reading the files $s8 changed: ${gfiles#BLOCKED:}"; return; }
  if [ "$(printf '%s\n' "$gfiles" | grep -c .)" -ge 300 ]; then
    echo "BLOCKED $s8 changes 300 files or more, GitHub's listing cap, so whether a gate applies to it cannot be read"; return
  fi
  gout=$(require_gates "$sha" "$gfiles"); grc=$?
  gout=$(printf '%s' "$gout" | sed 's/^ *//' | paste -sd'; ' -)
  case $grc in
    1) echo "RED GATE $gout"; return ;;
    2) echo "BLOCKED $gout"; return ;;
    3) echo "WAIT $gout"; return ;;
  esac
  ci=$(rd "repos/$R/actions/workflows/ci.yml/runs?head_sha=$sha&per_page=5" --jq '[.workflow_runs[]|select(.event=="push")][0]|"\(.status) \(.conclusion)"') || { echo "BLOCKED reading CI runs for $s8: ${ci#BLOCKED:}"; return; }
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
    completed*) echo "RED CI $ci"; return ;;
    ""|null*) echo "WAIT no CI run yet"; return ;;
    *) echo "WAIT CI $ci"; return ;;
  esac
  since=$(date -u -d "@$(git show -s --format=%ct "$sha")" +%Y-%m-%dT%H:%M:%SZ)
  local window=">=$since"; [ -n "${MW_TEST_UNTIL:-}" ] && window="$since..$MW_TEST_UNTIL"
  # Every page: the list grows all day (one scheduled run per 15 min), so page 1 alone loses the run that
  # judged an older commit. The total is read first, so a run created mid-read cannot fake a shortfall.
  local total list got runs scanned=0
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
    j=$(rd "repos/$R/actions/runs/$r/jobs" --jq '.jobs[]|select(.name=="What to publish")|.id') || { echo "BLOCKED listing jobs of Release run $r: ${j#BLOCKED:}"; return; }
    j=$(echo "$j" | head -1)
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
  done
  for st in $ACTIVE; do
    n=$(gh run list -R $R --workflow release.yml --status $st --limit 5 --json databaseId --jq length 2>&1) || { echo "BLOCKED listing $st Release runs: $(echo "$n" | tr '\n' ' ' | cut -c1-160)"; return; }
    [ "${n:-0}" -gt 0 ] && { echo "WAIT Release run $st"; return; }
  done
  echo "WAIT no Release plan has judged ${s8} yet: read all $got Release runs created since $since (every page), none of the $scanned completed non-push plans names it (the 15-min backstop judges it next)"
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
    while read -r sha subj; do
      v=$(verdict "$sha"); echo "    ${sha:0:9} ${subj:0:70} → $v"
      case "$v" in "NOMAC $tag"*|NOPATH*) ;; *) nogo+=("${sha:0:9} not judged no-Mac-change since $tag ($v)") ;; esac
    done <<< "$ch"
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
    BLOCKED*) echo "VERDICT $(ts): BLOCKED — cannot read what the verdict needs, so no verdict is inferred: ${v#BLOCKED }" ;;
    *) echo "VERDICT $(ts): WAIT — $v" ;;
  esac
}

# checks <sha> [<pr>]: the required-check reading for one head on its own, so it can be checked
# against a known commit without running a GO. With <pr>, the diff is that PR's three-dot diff;
# without it, the commit's diff against its first parent.
checks() {
  local sha=$1 files
  if [ -n "${2:-}" ]; then files=$(pr_files "$2"); else files=$(commit_files "$sha"); fi
  [ -n "$files" ] && [ "${files#BLOCKED}" = "$files" ] || { echo "BLOCKED reading changed files: ${files#BLOCKED:}"; return 2; }
  echo "required checks for ${sha:0:9}, $(printf '%s\n' "$files" | grep -c .) file(s) changed:"
  require_gates "$sha" "$files"; local rc=$?
  case $rc in
    0) echo "RESULT: SATISFIED — every gate that applies to this head has a successful run on it" ;;
    1) echo "RESULT: REFUSE — a gate that applies to this head has no successful run on it" ;;
    2) echo "RESULT: BLOCKED — a gate could not be read, so nothing is inferred" ;;
    3) echo "RESULT: WAIT — a gate that applies to this head is still running" ;;
  esac
  return $rc
}

case "${1:-}" in
  go) shift; go "$@" ;;
  after) after "$2" ;;
  verdict) verdict "$2" ;;
  checks) checks "$2" "${3:-}" ;;
  *) echo "usage: merge-window.sh go [<pr>] | after <prev-ref> | verdict <sha> | checks <sha> [<pr>]"; exit 2 ;;
esac
