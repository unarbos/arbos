# befe538cd9: mt-13-done-file-after-wait (mt-13-spawn-wait-gives-result-once)

status: draft (auto-triaged; a person or fix agent confirms)
scenario: mt-13-spawn-wait-gives-result-once
feature: 
rollout: /cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/qa/rollouts/20260913T205747Z-mt-13-spawn-wait-gives-result-once
first_seen: 20260913T205827Z

## Detail

1 extra root turn(s) after a spawn wait=true (the done file followed the tool result)

## Suspected location

arbos-kernel: done file after wait=true

## Repro

`python3 run.py --kernel <bin> --only mt-13-spawn-wait-gives-result-once`
