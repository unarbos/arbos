#!/usr/bin/env bash
# Is the turn slowdown the kernel's, or the provider's?
#
# Cycle 11 ran ordinary-task in 361.7 s where the four cycles before it took 21-29 s, and
# secrets-leak-hunt in 501.3 s against 15-35 s. Two candidates: the Jev controller commits that
# landed between the 15:01 and 17:01 cycles (every turn now makes a controller call before the chat
# model), or provider rate limiting, which is demonstrably present today.
#
# Running all of one kernel then all of the other cannot separate them: the provider's mood changes
# over the hour. Alternating does — both builds meet the same conditions, run for run.
set -uo pipefail
cd "$HOME/arbos-qa" && set -a && . ./secrets.env && set +a && cd /tmp/j42

NEW="$HOME/arbos-qa/target-desktop-main/release/arbos-kernel"    # fba8688d92d2, has the Jev work
OLD="$HOME/arbos-qa/target-track-main/release/arbos-kernel"      # cea8b902eecf, before it
SCEN="${1:-ordinary-task}"

echo "## $SCEN, alternating new/old, 4 rounds"
echo "   new = $("$NEW" --version 2>&1 | head -1)"
echo "   old = $("$OLD" --version 2>&1 | head -1)"
for round in 1 2 3 4; do
  for arm in new old; do
    bin=$NEW; [ "$arm" = old ] && bin=$OLD
    out=$(ARBOS_QA_NS_WRAP="$HOME/arbos-qa/deploy/ns-wrap.sh" timeout 12m python3 run.py \
            --kernel "$bin" --kernel-branch main --only "$SCEN" --with-model 2>&1 \
          | grep -E '^\[(pass|break)\]' | head -1)
    secs=$(echo "$out" | grep -oE '\([0-9.]+s' | tr -d '(s')
    verdict=$(echo "$out" | grep -oE '^\[(pass|break)' | tr -d '[')
    printf '   round %s  %-4s  %8ss  %s\n' "$round" "$arm" "${secs:-?}" "${verdict:-none}"
  done
done
