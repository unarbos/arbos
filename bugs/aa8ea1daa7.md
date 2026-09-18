# aa8ea1daa7: state:transcript-bad-lines (pl-01-a-turn-after-a-crash-mid-append-is-not-swallowed-by-the-partial-line)

status: draft (auto-triaged; a person or fix agent confirms)
scenario: pl-01-a-turn-after-a-crash-mid-append-is-not-swallowed-by-the-partial-line
feature: 
rollout: /home/ubuntu/arbos-qa/loop/rollouts/20260918T125524Z-pl-01-a-turn-after-a-crash-mid-append-is-not-swallowed-by-the-partial-line
first_seen: 20260918T125525Z

## Detail

1 unparseable line(s): [4]

## Suspected location

/tmp/arbos-qa-pl-01-a-turn-after-a-crash-mid-append-is-not-swallowed-by-the-partial-line-q4rr0ypg/place/.arbos/agents/root/transcript.jsonl

## Repro

`python3 run.py --kernel <bin> --only pl-01-a-turn-after-a-crash-mid-append-is-not-swallowed-by-the-partial-line`
