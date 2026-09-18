# 2eae626fbf: no-yield-to-the-report (co-04-attached-command-yields-to-a-workers-report-and-its-result-still-arrives)

status: draft (auto-triaged; a person or fix agent confirms)
scenario: co-04-attached-command-yields-to-a-workers-report-and-its-result-still-arrives
feature: 
rollout: /home/ubuntu/arbos-qa/loop/rollouts/20260917T125600Z-co-04-attached-command-yields-to-a-workers-report-and-its-result-still-arrives
first_seen: 20260917T125614Z

## Detail

the attached command did not yield to the worker's report (turn took 12.2s; record: {"ts": 1789649773169, "kind": "tool", "name": "bash", "call_id": "replay_2", "step": 2, "paths": ["/tmp/arbos-qa-co-04-attached-command-yields-to-a-workers-report-and-its-result-still-arrives-lpwl7o2w)

## Suspected location

arbos-engine tools/bash.rs — yield on a worker's done (#432)

## Repro

`python3 run.py --kernel <bin> --only co-04-attached-command-yields-to-a-workers-report-and-its-result-still-arrives`
