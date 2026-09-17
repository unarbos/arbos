# 1efe7d6a7f: silent-wait (rw-10c-a-long-wait-for-the-checkpoint-tree-is-said-to-the-person)

status: draft (auto-triaged; a person or fix agent confirms)
scenario: rw-10c-a-long-wait-for-the-checkpoint-tree-is-said-to-the-person
feature: 
rollout: /home/ubuntu/arbos-qa/loop/rollouts/20260917T102133Z-rw-10c-a-long-wait-for-the-checkpoint-tree-is-said-to-the-person
first_seen: 20260917T102140Z

## Detail

0.1s between the person's message and the first tool with nothing on any client-visible surface naming the wait (frames seen: {'plan': 2, 'turn': 1, 'status': 1, 'event/tool': 1}; status: ['']; notices: []) — the silent stall shape (st-01), with correct data underneath

## Suspected location

arbos-engine turn.rs — the checkpoint wait (#419 at 2daa555d) is logged to stderr only

## Repro

`python3 run.py --kernel <bin> --only rw-10c-a-long-wait-for-the-checkpoint-tree-is-said-to-the-person`
