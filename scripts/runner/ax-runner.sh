#!/bin/bash
# Second shared command runner for the EC2 Mac rigs: executes /Users/Shared/arbos-run/ax-cmd-*.sh in order
# (same protocol as runner.sh: .log, .rc, then renamed .done). Started from an ssh session via
# `launchctl asuser 502`, so macOS charges its TCC checks to /usr/libexec/sshd-keygen-wrapper — the one
# client with Accessibility (UI scripting, CGEvent clicks). runner.sh stays under Terminal, the one client
# with Screen Recording. Use the `ax` helper to run a click step here from a runner.sh command.
#
# Guards (2026-10-27 05:53–06:03: a rig's shoot.sh put axbin on PATH inside an `ax` command, so the
# osascript inside this runner resolved to the forwarding shim and queued itself behind itself — both
# shared runners deadlocked for 9 min):
#  - every job runs with ARBOS_AX_RUNNER=<this pid> in its environment and axbin stripped from PATH; the
#    shim refuses to forward when ARBOS_AX_RUNNER is set (it is already inside the runner);
#  - a watchdog kills a job that runs longer than AX_JOB_MAX_S (default 600) and writes rc 124 + a log line.
cd /Users/Shared/arbos-run || exit 1
AX_JOB_MAX_S=${AX_JOB_MAX_S:-600}
echo "ax-runner up pid $$ user $(id -un) $(date -u)" > ax-runner.status
clean_path() { echo "$PATH" | tr ':' '\n' | grep -vx '/Users/Shared/arbos-run/axbin' | paste -sd: -; }
while true; do
  for f in $(ls ax-cmd-*.sh 2>/dev/null | sort); do
    base=${f%.sh}
    ( export ARBOS_AX_RUNNER=$$ PATH; PATH=$(clean_path); exec bash "$f" ) > "$base.log" 2>&1 < /dev/null &
    job=$!
    waited=0
    while kill -0 "$job" 2>/dev/null; do
      sleep 1; waited=$((waited + 1))
      if [ "$waited" -ge "$AX_JOB_MAX_S" ]; then
        pkill -TERM -P "$job" 2>/dev/null; kill -TERM "$job" 2>/dev/null; sleep 2
        pkill -KILL -P "$job" 2>/dev/null; kill -KILL "$job" 2>/dev/null
        echo "ax-runner: job $f killed by the watchdog after ${AX_JOB_MAX_S}s $(date -u +%FT%TZ)" | tee -a "$base.log" >> ax-runner.status
        logger -t ax-runner "watchdog killed $f after ${AX_JOB_MAX_S}s"
        break
      fi
    done
    wait "$job" 2>/dev/null; rc=$?
    [ "$waited" -ge "$AX_JOB_MAX_S" ] && rc=124
    echo "$rc" > "$base.rc"; mv "$f" "$base.done"
  done
  sleep 0.2
done
