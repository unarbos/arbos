#!/bin/bash
# The 15-minute poll: the poller from the repo (main once deploy/feedback has merged, the PR branch until then), creds from the vault, state in ~/mobile-feedback (mirrored to the project store by the loop).
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
source ~/.op-env
cd ~/arbos-tools && git fetch -q origin && (git checkout -q origin/main -- deploy/feedback 2>/dev/null || git pull -q)
. deploy/feedback/asc-env.sh || exit 1
FEEDBACK_OUT=~/mobile-feedback exec python3 deploy/feedback/asc-feedback.py "$@"
