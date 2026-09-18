# 0ab7ccab21: driver-exception (ck-01-a-subscription-survives-a-clock-jump-without-a-storm-or-being-stranded)

status: draft (auto-triaged; a person or fix agent confirms)
scenario: ck-01-a-subscription-survives-a-clock-jump-without-a-storm-or-being-stranded
feature: 
rollout: /home/ubuntu/arbos-qa/loop/rollouts/20260918T140152Z-ck-01-a-subscription-survives-a-clock-jump-without-a-storm-or-being-stranded
first_seen: 20260918T140220Z
kernel: arbos-kernel 0.2.0 a8678ac16636 protocol 1

## Detail

FileNotFoundError: [Errno 2] No such file or directory: '/tmp/arbos-qa-ck-01-a-subscription-survives-a-clock-jump-without-a-storm-or-being-stranded-ufaxxt3y/place/.arbos/agents/root/subscriptions/0002-overdue.toml'

## Suspected location

(fill in)

## Repro

`python3 run.py --kernel <bin> --only ck-01-a-subscription-survives-a-clock-jump-without-a-storm-or-being-stranded`
