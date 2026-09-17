# 9c8bf3b8ac: journey-J6 (journey-linux)

status: draft (auto-triaged; a person or fix agent confirms)
scenario: journey-linux
feature: 
rollout: /home/ubuntu/arbos-qa/loop/rollouts/20260917T201311Z-journey-linux
first_seen: 20260917T201608Z

## Detail

workers existed but no worker/archived row is shown after relaunch

## Suspected location

docs/acceptance-journeys.md

## Repro

`python3 run.py --kernel <bin> --only journey-linux`
