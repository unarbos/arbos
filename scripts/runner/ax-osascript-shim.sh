#!/bin/bash
# osascript shim for runner.sh commands (installed as /Users/Shared/arbos-run/axbin/osascript): UI
# scripting needs Accessibility, which Terminal (runner.sh's TCC identity) was denied; the ax-runner
# (sshd-keygen-wrapper identity) has it. Forward every call there.
#
# Recursion guard: inside the ax-runner this shim must never be reached (axbin is stripped from PATH
# there), but a job that re-adds axbin to its own PATH would forward to the runner it is running in and
# wait for itself forever (05:53–06:03 Oct 27, both shared runners stuck 9 min). ARBOS_AX_RUNNER marks
# "already inside": run the real binary directly.
if [ -n "${ARBOS_AX_RUNNER:-}" ]; then
  echo "axbin/osascript: already inside the ax-runner (pid $ARBOS_AX_RUNNER); calling /usr/bin/osascript directly — do not put axbin on PATH inside an ax command" >&2
  exec /usr/bin/osascript "$@"
fi
exec /Users/Shared/arbos-run/ax /usr/bin/osascript "$@"
