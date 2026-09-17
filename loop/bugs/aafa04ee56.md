# aafa04ee56: failed-restore-changed-the-tree (rw-08-failed-restore-leaves-the-tree-where-it-was)

status: draft (auto-triaged; a person or fix agent confirms)
scenario: rw-08-failed-restore-leaves-the-tree-where-it-was
feature: 
rollout: /home/ubuntu/arbos-qa/loop/rollouts/20260917T091840Z-rw-08-failed-restore-leaves-the-tree-where-it-was
first_seen: 20260917T091842Z

## Detail

a restore that failed left the person somewhere new: f2.txt: GONE; HEAD: 8503b5198d -> 2360a38345; index: changed

## Suspected location

arbos-engine tools::git restore — a destructive step (reset --hard, clean) runs before the step that can fail (read-tree); check the objects first, or take them back

## Repro

`python3 run.py --kernel <bin> --only rw-08-failed-restore-leaves-the-tree-where-it-was`
