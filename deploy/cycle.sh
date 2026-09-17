#!/usr/bin/env bash
# One QA cycle on ArbosLife. Everything lives under $ROOT (default ~/arbos-qa).
#   1. fetch origin/rust, build the release kernel if HEAD changed
#   2. pull inbox notes from the qa-results branch
#   3. run the suite (model scenarios under the daily budget)
#   4. drop passed rollouts older than 30 days
#   5. publish bugs, kickoff history, spend, and run index to qa-results
set -euo pipefail
ROOT="${ARBOS_QA_ROOT:-$HOME/arbos-qa}"
# Pause switch: a file state/PAUSED-until-<UTC ISO> stops every run until that instant
# (Jacob, 2026-09-13 21:50 UTC). Remove the file or wait it out to resume.
qa_paused() {
  for f in "$ROOT"/state/PAUSED-until-*; do
    [ -e "$f" ] || continue
    until=$(basename "$f" | sed 's/^PAUSED-until-//'); until_s=$(date -u -d "$until" +%s 2>/dev/null || echo 0)
    if [ "$(date -u +%s)" -lt "$until_s" ]; then echo "== paused until $until (remove $f to resume)"; return 0; fi
    rm -f "$f"
  done
  return 1
}
if qa_paused; then exit 0; fi
# Mirror the store (docs/, notes.md, internal/ within the boundary in internal/store-docs-mirror.md
# and the branch README) to the orphan branch store-docs. Pushes only on change; safe to run
# concurrently. A non-zero exit is a finding, not a nuisance: the script refuses to push when the
# store looks damaged, so a refusal means the store has faulted again or a document was deleted.
# mirror-alarm.py records it, names what is missing against the branch, stages a restore under
# state/ (never into the store), and drafts a bug. Run at the start and the end of every cycle, so
# the window between a write and its copy is under an hour — two losses in five hours on 2026-09-16.
mirror_store() {
  local when="$1" MIRROR mrc
  MIRROR="${ARBOS_QA_STORE_ROOT:-/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983}/internal/mirror-docs.sh"
  if [ -f "$MIRROR" ]; then
    set +e
    REPO="$ROOT/repo" timeout 10m bash "$MIRROR"
    mrc=$?
    set -e
    if [ "$mrc" -ne 0 ]; then
      echo "-- mirror-docs ($when): exit $mrc (a refusal is an alarm)"
      python3 "$ROOT/deploy/mirror-alarm.py" "$ROOT/loop" "$mrc" "$MIRROR" "$ROOT/repo" || echo "-- mirror-alarm failed"
    fi
  else
    echo "-- mirror-docs ($when): store not mounted here or the script is gone; skipped"
    [ -d "${ARBOS_QA_STORE_ROOT:-/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983}/internal" ] && python3 "$ROOT/deploy/mirror-alarm.py" "$ROOT/loop" 3 "$MIRROR" "$ROOT/repo" || true
  fi
}

# 0. Mirror the store first: the tightest window for whatever was written since the last cycle.
mirror_store start
BUDGET_USD="${ARBOS_QA_BUDGET_USD:-20}"  # raised from 10 on 2026-09-16: the journey + call mode + desktop set hit $10 by 14:30 UTC
JOBS="${ARBOS_QA_BUILD_JOBS:-8}"
LOG="$ROOT/logs/cycle-$(date -u +%Y%m%dT%H%M%SZ).log"
mkdir -p "$ROOT/logs" "$ROOT/state"
exec > >(tee -a "$LOG") 2>&1
echo "== cycle start $(date -u +%FT%TZ) root=$ROOT budget=\$$BUDGET_USD"

# Secrets: two values, 0600, never echoed.
set -a; . "$ROOT/secrets.env"; set +a
# The loop never depends on this host's global git config (signing helpers, aliases, fsmonitor).
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
if [ -z "${ARBOS_QA_SYSTEM_TOOLCHAIN:-}" ]; then
  export RUSTUP_HOME="$ROOT/toolchain/rustup" CARGO_HOME="$ROOT/toolchain/cargo"
  # No system C compiler on ArbosLife: zig (user-space tarball) is cc, ar and
  # the linker ($CARGO_HOME/config.toml points the linker at toolchain/bin/cc).
  export PATH="$CARGO_HOME/bin:$ROOT/toolchain/bin:$PATH"
  export CC="$ROOT/toolchain/bin/cc" AR="$ROOT/toolchain/bin/ar"
fi
# qa-020 stopgap until #73/#74 are on every tracked branch: kernels under
# test shell out to `kill -9 -<pid>`; procps reads that as kill(-1). The
# shim inserts `--` so a negative pid is a process group, never "everyone".
export PATH="$ROOT/shim:$PATH"
"$ROOT/deploy/kill-shim.sh" install "$ROOT/shim"
export ARBOS_QA_STAGING="$ROOT/staging"

# 1. build
cd "$ROOT/repo"
git fetch -q origin rust
git checkout -q --detach origin/rust
SHA=$(git rev-parse --short=12 HEAD)
KERNEL="$ROOT/repo/target/release/arbos-kernel"
if [ ! -x "$KERNEL" ] || [ "$(cat "$ROOT/state/built-sha" 2>/dev/null || true)" != "$SHA" ]; then
  echo "-- building arbos-kernel at $SHA"
  nice -n 19 cargo build --release -p arbos-kernel -j "$JOBS"
  echo "$SHA" > "$ROOT/state/built-sha"
else
  echo "-- kernel already built at $SHA"
fi

# 2. inbox notes published by cloud agents (see publish.sh / sync.sh)
"$ROOT/deploy/publish.sh" pull || true

# 3. run
cd "$ROOT/loop"
rm -rf "$ROOT/staging"; mkdir -p "$ROOT/staging"
set +e
nice -n 10 timeout 50m python3 run.py --kernel "$KERNEL" --kernel-branch rust --with-model --budget-usd "$BUDGET_USD"
echo "-- run.py exit $?"
set -e
rm -rf "$ROOT/loop/__pycache__"

# 3a. Tracked branches: whole suite against each, with every inbox gate open
# (an integration branch carries every feature). Space-separated list.
for branch in ${ARBOS_QA_TRACK_BRANCHES:-main}; do
  slug=$(echo "$branch" | tr '/' '-')
  if ! git -C "$ROOT/repo" fetch -q origin "$branch" 2>/dev/null; then
    echo "-- track $branch: not on origin (merged or deleted); skipped"; continue
  fi
  sha=$(git -C "$ROOT/repo" rev-parse FETCH_HEAD)
  wt="$ROOT/repo-track/$slug"
  if [ -d "$wt/.git" ] || [ -f "$wt/.git" ]; then
    git -C "$wt" checkout -q --detach "$sha" || { echo "-- track $branch: checkout failed"; continue; }
  else
    mkdir -p "$ROOT/repo-track"
    git -C "$ROOT/repo" worktree add -q --detach "$wt" "$sha" || { echo "-- track $branch: worktree failed"; continue; }
  fi
  echo "-- track $branch: building ($(git -C "$wt" rev-parse --short=12 HEAD))"
  if ! (cd "$wt" && CARGO_TARGET_DIR="$ROOT/target-track-$slug" nice -n 19 cargo build --release -p arbos-kernel -j "$JOBS" 2>&1 | tail -2); then
    echo "-- track $branch: build failed"; continue
  fi
  # fp-* (subscriptions/ engine, PR #104/#106) run only where the kernel source has it.
  fileplan=off
  git -C "$wt" grep -q -e "subscriptions" -- crates/arbos-kernel/src/hooks.rs 2>/dev/null && fileplan=on
  set +e
  nice -n 10 timeout 50m python3 run.py --kernel "$ROOT/target-track-$slug/release/arbos-kernel" --kernel-branch "$branch" --integration --fileplan "$fileplan" --with-model --budget-usd "$BUDGET_USD"
  echo "-- track $branch: run.py exit $?"
  set -e
done
rm -rf "$ROOT/loop/__pycache__"

# 3b. inbox notes name the branch a feature lives on. Build that branch in
# its own worktree (shared inbox target dir) and run the note's scenario
# against it, newest three notes first. A branch that is gone (merged,
# deleted) is skipped.
python3 - "$ROOT/loop/inbox" <<'PYEOF' > "$ROOT/state/inbox-branches"
import os, re, sys
d = sys.argv[1]
notes = sorted((f for f in os.listdir(d) if f.endswith(".md")), reverse=True)[:3] if os.path.isdir(d) else []
for f in notes:
    text = open(os.path.join(d, f), errors="replace").read()
    m = re.search(r"(?i)branch\s+`([^`]+)`", text)
    feature = re.sub(r"^\d{4}-\d{2}-\d{2}-", "", f[:-3])
    if m:
        print(feature, m.group(1))
PYEOF
while read -r feature branch; do
  [ -n "${branch:-}" ] || continue
  if ! git -C "$ROOT/repo" fetch -q origin "$branch" 2>/dev/null; then
    echo "-- inbox $feature: branch $branch is not on origin; skipped"; continue
  fi
  # FETCH_HEAD lives in the main repo, not in a worktree: resolve it first.
  sha=$(git -C "$ROOT/repo" rev-parse FETCH_HEAD)
  wt="$ROOT/repo-inbox/$feature"
  if [ -d "$wt/.git" ] || [ -f "$wt/.git" ]; then
    git -C "$wt" checkout -q --detach "$sha" || { echo "-- inbox $feature: checkout failed"; continue; }
  else
    mkdir -p "$ROOT/repo-inbox"
    git -C "$ROOT/repo" worktree add -q --detach "$wt" "$sha" || { echo "-- inbox $feature: worktree failed"; continue; }
  fi
  echo "-- inbox $feature: building $branch ($(git -C "$wt" rev-parse --short=12 HEAD))"
  if ! (cd "$wt" && CARGO_TARGET_DIR="$ROOT/target-inbox" nice -n 19 cargo build --release -p arbos-kernel -j "$JOBS" 2>&1 | tail -3); then
    echo "-- inbox $feature: build failed"; continue
  fi
  set +e
  nice -n 10 timeout 15m python3 run.py --kernel "$ROOT/target-inbox/release/arbos-kernel" --kernel-branch "$branch" --with-model --budget-usd "$BUDGET_USD" --only "inbox:$feature"
  set -e
done < "$ROOT/state/inbox-branches" || echo "-- inbox step ended early"
rm -rf "$ROOT/loop/__pycache__"

# 3b2. Desktop stack (#105 `cursor/project-panel-94d6` and whatever ARBOS_QA_DESKTOP_BRANCH
# names): build the kernel and the gpui app from that head and run the desktop-tagged
# scenarios under Xvfb (multitasking audit items 1, 4, 14, 17-20, 23, 24; qa-030). Only where
# the app builds (X11 dev libs, mesa-vulkan-drivers): ARBOS_QA_DESKTOP=1 on the QA VM, unset on
# ArbosLife.
if [ "${ARBOS_QA_DESKTOP:-0}" = 1 ] && command -v Xvfb >/dev/null 2>&1; then
  for branch in ${ARBOS_QA_DESKTOP_BRANCH:-main}; do
    slug=$(echo "$branch" | tr '/' '-')
    if ! git -C "$ROOT/repo" fetch -q origin "$branch" 2>/dev/null; then
      echo "-- desktop $branch: not on origin; skipped"; continue
    fi
    sha=$(git -C "$ROOT/repo" rev-parse FETCH_HEAD)
    wt="$ROOT/repo-track/desktop-$slug"
    if [ -d "$wt/.git" ] || [ -f "$wt/.git" ]; then
      git -C "$wt" checkout -q --detach "$sha" || { echo "-- desktop $branch: checkout failed"; continue; }
    else
      mkdir -p "$ROOT/repo-track"
      git -C "$ROOT/repo" worktree add -q --detach "$wt" "$sha" || { echo "-- desktop $branch: worktree failed"; continue; }
    fi
    echo "-- desktop $branch: building kernel + app ($(git -C "$wt" rev-parse --short=12 HEAD))"
    if ! (cd "$wt" && CARGO_TARGET_DIR="$ROOT/target-desktop-$slug" nice -n 19 cargo build --release -p arbos-kernel -j "$JOBS" 2>&1 | tail -1); then
      echo "-- desktop $branch: kernel build failed"; continue
    fi
    if ! (cd "$wt/desktop" && CARGO_TARGET_DIR="$ROOT/target-desktop-$slug/desktop" nice -n 19 cargo build -j "$JOBS" 2>&1 | tail -1); then
      echo "-- desktop $branch: app build failed"; continue
    fi
    app=""
    for cand in "$ROOT/target-desktop-$slug/desktop/debug/arbos-desktop" "$ROOT/target-desktop-$slug/desktop/debug/cydonia"; do
      [ -x "$cand" ] && { app="$cand"; break; }
    done
    [ -n "$app" ] || { echo "-- desktop $branch: no app binary"; continue; }
    fileplan=off
    git -C "$wt" grep -q -e "subscriptions" -- crates/arbos-kernel/src/hooks.rs 2>/dev/null && fileplan=on
    set +e
    ARBOS_DESKTOP_BIN="$app" ARBOS_DESKTOP_DRIVER="${ARBOS_QA_DRIVER_DIR:-$wt/desktop/driver}" \
      nice -n 10 timeout 60m python3 run.py --kernel "$ROOT/target-desktop-$slug/release/arbos-kernel" --kernel-branch "$branch" --integration --fileplan "$fileplan" --tag desktop --with-model --budget-usd "$BUDGET_USD"
    echo "-- desktop $branch: run.py exit $?"
    # The acceptance journey (docs/acceptance-journeys.md) ran inside the desktop-tagged set; say its score
    # here so every cycle log carries it, and the pass rate over the last ten runs.
    if [ -f journey-history.jsonl ]; then
      python3 - <<'PY'
import json
runs = [json.loads(l) for l in open("journey-history.jsonl") if l.strip()]
if runs:
    last = runs[-1]
    marks = " ".join(f"{s}{'✓' if v == 'pass' else ('?' if v == 'unverified' else '✗')}" for s, v in last["steps"].items())
    print(f"-- journey: {last['score']}/8 pass, {len(last['unverified'])} unverified, {len(last['failed'])} fail — {marks}")
    tail = runs[-10:]
    rate = {s: sum(1 for r in tail if r["steps"].get(s) == "pass") for s in last["steps"]}
    print("-- journey pass rate, last %d runs: %s" % (len(tail), " ".join(f"{s} {n}/{len(tail)}" for s, n in rate.items())))
PY
    fi
    set -e
  done
  rm -rf "$ROOT/loop/__pycache__"
fi

# 3c. Call mode (standing goal): the voice gateway's scripted harness, no GPU and no model,
# against the head of the narrator branch. A red scenario becomes bugs/call-<scenario>.md.
CALL_BRANCH="${ARBOS_QA_CALL_BRANCH:-main}"
if [ "${ARBOS_QA_CALL_MODE:-1}" = 1 ] && git -C "$ROOT/repo" fetch -q origin "$CALL_BRANCH" 2>/dev/null; then
  sha=$(git -C "$ROOT/repo" rev-parse FETCH_HEAD)
  wt="$ROOT/repo-track/call-mode"
  if [ -d "$wt/.git" ] || [ -f "$wt/.git" ]; then
    git -C "$wt" checkout -q --detach "$sha" || echo "-- call-mode: checkout failed"
  else
    mkdir -p "$ROOT/repo-track"
    git -C "$ROOT/repo" worktree add -q --detach "$wt" "$sha" || echo "-- call-mode: worktree failed"
  fi
  vs="$wt/voice-server"
  if [ -f "$vs/pyproject.toml" ]; then
    if [ ! -x "$vs/.venv/bin/python" ]; then
      echo "-- call-mode: creating the harness venv"
      if command -v uv >/dev/null 2>&1; then (cd "$vs" && uv venv -q .venv && uv pip install -q -p .venv/bin/python -e .) || echo "-- call-mode: venv failed"
      else (cd "$vs" && python3 -m venv .venv && .venv/bin/pip install -q -e .) || echo "-- call-mode: venv failed"; fi
    fi
    if [ -x "$vs/.venv/bin/python" ]; then
      echo "-- call-mode: harness on $CALL_BRANCH ($(git -C "$wt" rev-parse --short=12 HEAD))"
      set +e
      (cd "$vs" && rm -rf tests/out && nice -n 10 timeout 15m .venv/bin/python -m tests.run > "$ROOT/logs/call-mode-$(date -u +%Y%m%dT%H%M%SZ).log" 2>&1)
      echo "-- call-mode: tests.run exit $?"
      python3 "$ROOT/deploy/call-mode-collect.py" "$vs/tests/out/report.json" "$ROOT/loop" "$CALL_BRANCH" "$sha"
      # The pod's public endpoints, probed the way their clients use them (kernel = WebSocket).
      [ -f "$HOME/.ssh/arbos_agents" ] && timeout 120 "$vs/.venv/bin/python" "$ROOT/deploy/pod-health.py" "$ROOT/loop" || echo "-- pod-health: skipped or failed"
      set -e
    fi
  else
    echo "-- call-mode: no voice-server/ on $CALL_BRANCH"
  fi
fi

# 4. retention: passed rollouts go after 30 days; breaks stay
python3 - "$ROOT/loop/rollouts" <<'EOF'
import json, os, shutil, sys, time
root = sys.argv[1]
cutoff = time.time() - 30 * 86400
for name in os.listdir(root):
    d = os.path.join(root, name)
    if not os.path.isdir(d) or os.path.getmtime(d) > cutoff:
        continue
    try:
        status = json.load(open(os.path.join(d, "result.json"))).get("status")
    except Exception:
        status = "unknown"
    if status == "pass":
        shutil.rmtree(d, ignore_errors=True)
        print(f"-- retention: removed {name}")
EOF

# 5. publish
"$ROOT/deploy/publish.sh" push || echo "-- publish failed (kept locally)"

# 6. Mirror the store again at the end of the cycle.
# Two or more clients' views of the store, compared: the per-client fault (2026-09-17 05:35) is visible only this way.
python3 - "${ARBOS_QA_STORE_ROOT:-/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983}/internal/qa/store-probes" "$ROOT/loop/store-probe-qa-vm.jsonl" <<'PY' || true
import glob, json, os, sys, time
probes_dir, mine = sys.argv[1], sys.argv[2]
rows = {}
for f in glob.glob(os.path.join(probes_dir, "*.jsonl")) + [mine]:
    try:
        last = [json.loads(l) for l in open(f) if l.strip()][-1]
        rows[last.get("machine", os.path.basename(f))] = last
    except Exception:
        pass
def ts(r):
    try: return time.mktime(time.strptime(r["ts"], "%Y-%m-%dT%H:%M:%SZ")) - time.timezone
    except Exception: return 0
now = time.time()
fresh = {m: r for m, r in rows.items() if now - ts(r) < 1200}
if len(fresh) < 2:
    print(f"-- store views: {len(fresh)} fresh probe(s) ({', '.join(sorted(fresh)) or 'none'}); a second reader on another machine is needed to see a per-client fault (internal/store-probe-second-reader.md)")
else:
    whole = {m for m, r in fresh.items() if r.get("docs_dir") and r.get("write_ok")}
    broken = {m for m in fresh if m not in whole}
    if whole and broken:
        print("!! STORE VIEWS DISAGREE: " + ", ".join(f"{m} sees the store {'whole' if m in whole else 'empty/unwritable'}" for m in sorted(fresh)))
        for m in sorted(fresh): print("   " + json.dumps(fresh[m]))
    else:
        print(f"-- store views agree across {len(fresh)} machines: {', '.join(sorted(fresh))} ({'whole' if whole else 'all broken'})")
PY
# The second readers' verdicts (store-watch branch, one file per client): a FAULT from any client in the last
# 40 minutes is an alarm here, and two clients disagreeing is the per-session shape named in the fault report.
git -C "$ROOT/repo" fetch -q origin "+refs/heads/store-watch:refs/remotes/origin/store-watch" 2>/dev/null || true
python3 - "$ROOT/repo" <<'PY' || true
import json, subprocess, sys, time
repo = sys.argv[1]
def git(*a): return subprocess.run(["git", "-C", repo, *a], capture_output=True, text=True).stdout
files = [l for l in git("ls-tree", "-r", "--name-only", "origin/store-watch").splitlines() if l.startswith("readers/") and l.endswith(".jsonl")]
now = time.time(); latest = {}
for f in files:
    rows = [json.loads(l) for l in git("show", f"origin/store-watch:{f}").splitlines() if l.strip()]
    if rows: latest[rows[-1].get("client", f)] = rows[-1]
def age(r):
    try: return now - (time.mktime(time.strptime(r["ts"], "%Y-%m-%dT%H:%M:%SZ")) - time.timezone)
    except Exception: return 1e9
fresh = {c: r for c, r in latest.items() if age(r) < 2400}
faults = {c: r for c, r in fresh.items() if r.get("verdict") == "FAULT"}
if faults:
    print("!! STORE SECOND READER FAULT: " + "; ".join(f"{c} at {r['ts']}: {r.get('reason','')[:100]}" for c, r in faults.items()))
    if len(fresh) > len(faults):
        print("!! STORE VIEWS DISAGREE (per-session shape): " + ", ".join(f"{c}={r.get('verdict')}" for c, r in sorted(fresh.items())))
print(f"-- store second readers ({len(fresh)} fresh of {len(latest)}): " + ", ".join(f"{c}={r.get('verdict')}@{r['ts'][11:16]}" for c, r in sorted(fresh.items())) + ("" if len(fresh) >= 2 else " — a second machine's view is needed to see a per-client fault"))
PY
# Everything the runners shouted (!! SKIPPED / BUDGET BOUND / MODULE MISSING / HEADLINE NOT RUN) in one
# block at the end, so a cycle that measured less than it claims cannot look green in the log's tail.
if grep -q '^!!' "$LOG" 2>/dev/null; then
  echo "== ALARMS this cycle:"
  grep -h '^!!' "$LOG" | sort | uniq -c | sed 's/^/   /'
else
  echo "== alarms this cycle: none (every registered scenario ran or was skipped for a stated non-budget reason)"
fi
mirror_store end
echo "== cycle end $(date -u +%FT%TZ)"
