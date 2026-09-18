# cd9b0bdb65: af-01-line-lost-in-silence (af-01-folder-renamed-under-a-running-kernel)

status: draft (auto-triaged; a person or fix agent confirms)
scenario: af-01-folder-renamed-under-a-running-kernel
feature: 
rollout: /home/ubuntu/arbos-qa/loop/rollouts/20260917T130259Z-af-01-folder-renamed-under-a-running-kernel
first_seen: 20260917T130310Z

## Detail

the line typed after the rename went nowhere; the kernel stopped itself (stderr only) and nothing a person could read says the folder moved or the line was dropped

## Suspected location

arbos-kernel serve: on a lost store, a last notice into the moved folder's transcript, or the desktop's respawn telling the user

## Repro

`python3 run.py --kernel <bin> --only af-01-folder-renamed-under-a-running-kernel`
