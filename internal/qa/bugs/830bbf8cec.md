# 830bbf8cec: up-01-exec-attempted-onto-an-unusable-file (up-01-a-swap-still-in-progress-is-not-a-failed-restart)

status: draft (auto-triaged; a person or fix agent confirms)
scenario: up-01-a-swap-still-in-progress-is-not-a-failed-restart
feature: 
rollout: /home/ubuntu/arbos-qa/loop/rollouts/20260918T125915Z-up-01-a-swap-still-in-progress-is-not-a-failed-restart
first_seen: 20260918T125947Z
kernel: arbos-kernel 0.2.0 42cb9751ace8 protocol 1

## Detail

the kernel tried to exec onto the start path while it held nothing usable (['restarting onto /tmp/arbos-qa-up-01-a-swap-still-in-progress-is-not-a-failed-restart-lullgv1q/up01/bin/arbos-kernel (git']); an empty or half-written file must be waited for, not jumped onto

## Suspected location

arbos-kernel serve.rs reexec_onto_new_binary — the settled check (#453)

## Repro

`python3 run.py --kernel <bin> --only up-01-a-swap-still-in-progress-is-not-a-failed-restart`
