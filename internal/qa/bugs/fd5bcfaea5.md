# fd5bcfaea5: af-03-wrong-explanation (af-03-desktop-folder-renamed-under-the-window)

status: draft (auto-triaged; a person or fix agent confirms)
scenario: af-03-desktop-folder-renamed-under-the-window
feature: 
rollout: /home/ubuntu/arbos-qa/loop/rollouts/20260917T072040Z-af-03-desktop-folder-renamed-under-the-window
first_seen: 20260917T072148Z

## Detail

the chat explains a renamed folder as 'this agent is archived: its history stays, but it takes no more messages' — nothing was archived; the user's line went nowhere

## Suspected location

desktop session.rs: a kernel that stopped because its store is gone is drawn as an archived agent

## Repro

`python3 run.py --kernel <bin> --only af-03-desktop-folder-renamed-under-the-window`
