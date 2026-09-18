# d1caaaa735: state:subscription-due-past-its-period (ck-01-a-subscription-survives-a-clock-jump-without-a-storm-or-being-stranded)

status: draft (auto-triaged; a person or fix agent confirms)
scenario: ck-01-a-subscription-survives-a-clock-jump-without-a-storm-or-being-stranded
feature: 
rollout: /home/ubuntu/arbos-qa/loop/rollouts/20260918T160211Z-ck-01-a-subscription-survives-a-clock-jump-without-a-storm-or-being-stranded
first_seen: 20260918T160239Z
kernel: arbos-kernel 0.2.0 cea8b902eecf protocol 1

## Detail

next_due is 240.0 h ahead on a 30s period: a clock set backwards leaves this, and it will not run for that long (qal-j38)

## Suspected location

/tmp/arbos-qa-ck-01-a-subscription-survives-a-clock-jump-without-a-storm-or-being-stranded-apaax4g3/place/.arbos/agents/root/subscriptions/0003-future.toml

## Repro

`python3 run.py --kernel <bin> --only ck-01-a-subscription-survives-a-clock-jump-without-a-storm-or-being-stranded`
