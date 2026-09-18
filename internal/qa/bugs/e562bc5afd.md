# e562bc5afd: sw-02-undo-silent (sw-02-stale-undo-mark-resets-past-committed-work)

status: draft (auto-triaged; a person or fix agent confirms)
scenario: sw-02-stale-undo-mark-resets-past-committed-work
feature: 
rollout: /home/ubuntu/arbos-qa/loop/rollouts/20260917T130248Z-sw-02-stale-undo-mark-resets-past-committed-work
first_seen: 20260917T130249Z

## Detail

`undo` neither restored to turn two's start nor said why it refused: HEAD 5278f3b359a6, said ['restored 5278f3b359a635f03abfd96d7ca6ebfbd4ae21c5']

## Suspected location

(fill in)

## Repro

`python3 run.py --kernel <bin> --only sw-02-stale-undo-mark-resets-past-committed-work`
