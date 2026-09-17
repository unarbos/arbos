# adfdbf4fe2: state:transcript-unended-turn (disk-full)

status: draft (auto-triaged; a person or fix agent confirms)
scenario: disk-full
feature: 
rollout: /home/ubuntu/arbos-qa/loop/rollouts/20260913T173740Z-disk-full
first_seen: 20260913T173741Z

## Detail

wake at line 1 has no later turn_complete/interrupted; needs_serve() will refire it on every kernel start

## Suspected location

/tmp/arbos-qa-disk-full-w3r7txeb/place/.arbos/agents/root/transcript.jsonl

## Repro

`python3 run.py --kernel <bin> --only disk-full`
