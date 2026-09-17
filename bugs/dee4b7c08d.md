# dee4b7c08d: uw-01b-undo-silent (uw-01-unwritable-undo-mark-refuses-rather-than-using-an-older-turns)

status: draft (auto-triaged; a person or fix agent confirms)
scenario: uw-01-unwritable-undo-mark-refuses-rather-than-using-an-older-turns
feature: 
rollout: /home/ubuntu/arbos-qa/loop/rollouts/20260917T125240Z-uw-01-unwritable-undo-mark-refuses-rather-than-using-an-older-turns
first_seen: 20260917T125243Z

## Detail

arm (b): `undo` neither restored to turn two's start nor said why it refused — HEAD 461de789c7d8, said ['restored 461de789c7d8162a72c104567dcb2e9e7b984b3a']

## Suspected location

(fill in)

## Repro

`python3 run.py --kernel <bin> --only uw-01-unwritable-undo-mark-refuses-rather-than-using-an-older-turns`
