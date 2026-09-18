# d258c0dfcb: driver-exception (pl-01-a-turn-after-a-crash-mid-append-is-not-swallowed-by-the-partial-line)

status: draft (auto-triaged; a person or fix agent confirms)
scenario: pl-01-a-turn-after-a-crash-mid-append-is-not-swallowed-by-the-partial-line
feature: 
rollout: /home/ubuntu/arbos-qa/loop/rollouts/20260918T133817Z-pl-01-a-turn-after-a-crash-mid-append-is-not-swallowed-by-the-partial-line
first_seen: 20260918T133818Z
kernel: arbos-kernel 0.2.0 cecd48e1bd76 protocol 1

## Detail

NameError: name 'evs' is not defined

## Suspected location

(fill in)

## Repro

`python3 run.py --kernel <bin> --only pl-01-a-turn-after-a-crash-mid-append-is-not-swallowed-by-the-partial-line`
