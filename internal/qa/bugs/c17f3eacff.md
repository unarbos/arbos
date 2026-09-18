# c17f3eacff: sw-05-task-doubled (sw-05-failed-migration-rename-doubles-crons-and-tasks)

status: draft (auto-triaged; a person or fix agent confirms)
scenario: sw-05-failed-migration-rename-doubles-crons-and-tasks
feature: 
rollout: /home/ubuntu/arbos-qa/loop/rollouts/20260917T130251Z-sw-05-failed-migration-rename-doubles-crons-and-tasks
first_seen: 20260917T130256Z

## Detail

the pending task was queued more than once: ['20260917T130251.000Z-user-000.md', '20260917T130251.000Z-user-001.md']

## Suspected location

(fill in)

## Repro

`python3 run.py --kernel <bin> --only sw-05-failed-migration-rename-doubles-crons-and-tasks`
