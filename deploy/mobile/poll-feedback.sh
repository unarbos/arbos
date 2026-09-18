#!/bin/bash
# The 15-minute poll: the poller from the repo (main once deploy/feedback has merged, the PR branch until then), creds from the vault, state in ~/mobile-feedback (mirrored to the project store by the loop).
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
# The checkout this script lives in, not a second one in $HOME. `cd
# ~/arbos-tools` is how the loop lost forty cycles of tooling twice (M-238,
# M-324): the home copy usually works and silently lags. Here it did not
# even exist, the `cd` failed, the `&&` chain stopped, and the run carried
# on using whatever directory it happened to be started from — which worked
# only because that was a checkout too.
HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/../.." && pwd)
cd "$REPO" || exit 1
# Credentials come from the vault file when there is one and from the
# environment otherwise. Saying which beats a `source` that fails into a
# line of shell noise above a report that looks fine.
if [ -f ~/.op-env ]; then
  . ~/.op-env
  echo "creds: ~/.op-env" >&2
else
  echo "creds: the environment (no ~/.op-env on this machine)" >&2
fi
. deploy/feedback/asc-env.sh || exit 1
FEEDBACK_OUT=~/mobile-feedback exec python3 deploy/feedback/asc-feedback.py "$@"
