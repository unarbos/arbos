# ea194aedc5: sleep-ran-with-workers (co-01-bare-sleep-with-workers-is-refused-and-the-report-arrives)

status: draft (auto-triaged; a person or fix agent confirms)
scenario: co-01-bare-sleep-with-workers-is-refused-and-the-report-arrives
feature: 
rollout: /home/ubuntu/arbos-qa/loop/rollouts/20260917T125357Z-co-01-bare-sleep-with-workers-is-refused-and-the-report-arrives
first_seen: 20260917T125513Z

## Detail

`sleep 75` with a worker running was not refused: error='' output='None'

## Suspected location

arbos-engine tools/bash.rs bare_sleep_secs / children_count (#432)

## Repro

`python3 run.py --kernel <bin> --only co-01-bare-sleep-with-workers-is-refused-and-the-report-arrives`
