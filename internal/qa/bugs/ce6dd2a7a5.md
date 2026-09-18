# ce6dd2a7a5: pl-01-typed-line-swallowed-by-the-partial-line (pl-01-a-turn-after-a-crash-mid-append-is-not-swallowed-by-the-partial-line)

status: draft (auto-triaged; a person or fix agent confirms)
scenario: pl-01-a-turn-after-a-crash-mid-append-is-not-swallowed-by-the-partial-line
feature: 
rollout: /home/ubuntu/arbos-qa/loop/rollouts/20260918T125558Z-pl-01-a-turn-after-a-crash-mid-append-is-not-swallowed-by-the-partial-line
first_seen: 20260918T125558Z

## Detail

the line typed after the crash is on no readable event — it is inside an unparseable line, appended onto the partial one: '{"ts": 1789736158007, "kind": "assistant", "text": "hal{"ts":1789736158361,"kind":"wake","wake":"user","text":"AFTER-CRA'. `drop_partial_line` repairs only the process whose own write failed; after a crash nobody runs it, so the first append lands on a headless line and every reader skips both

## Suspected location

arbos-core files.rs append_events — cut a headless last line before appending, not only when this process's write failed

## Repro

`python3 run.py --kernel <bin> --only pl-01-a-turn-after-a-crash-mid-append-is-not-swallowed-by-the-partial-line`
