# edcfddb4ae: mt-01-typed-not-a-steer (mt-01-typed-while-running-steers)

status: draft (auto-triaged; a person or fix agent confirms)
scenario: mt-01-typed-while-running-steers
feature: 
rollout: /home/ubuntu/arbos-qa/loop/rollouts/20260916T213815Z-mt-01-typed-while-running-steers
first_seen: 20260916T213831Z

## Detail

no kind = steer inbox file within 3 s of typing during a running turn; the line waits for turn_complete

## Suspected location

desktop composer: send while running must steer by default

## Repro

`python3 run.py --kernel <bin> --only mt-01-typed-while-running-steers`
