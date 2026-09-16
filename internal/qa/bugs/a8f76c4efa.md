# a8f76c4efa: mt-29-no-rewound (mt-29-rewind-latency)

status: draft (auto-triaged; a person or fix agent confirms)
scenario: mt-29-rewind-latency
feature: 
rollout: /home/ubuntu/arbos-qa/loop/rollouts/20260916T153035Z-mt-29-rewind-latency
first_seen: 20260916T153059Z

## Detail

no rewound frame: {'type': 'error', 'agent': 'root', 'detail': 'rewind: the agent is running; stop the turn first'}

## Suspected location

(fill in)

## Repro

`python3 run.py --kernel <bin> --only mt-29-rewind-latency`
