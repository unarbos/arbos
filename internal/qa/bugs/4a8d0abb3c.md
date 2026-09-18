# 4a8d0abb3c: turn-looping (inbox:goals-coordinator)

status: draft (auto-triaged; a person or fix agent confirms)
scenario: inbox:goals-coordinator
feature: goals-coordinator
rollout: /home/ubuntu/arbos-qa/loop/rollouts/20260918T184402Z-inbox:goals-coordinator
first_seen: 20260918T184903Z
kernel: arbos-kernel 0.2.0 00cc5ba89968 protocol 1

## Detail

feature goals-coordinator: the same tool (read) was called the last 6 times running at the 300 s ceiling; the turn is going round rather than getting on

## Suspected location

(fill in)

## Repro

`python3 run.py --kernel <bin> --only inbox:goals-coordinator`
