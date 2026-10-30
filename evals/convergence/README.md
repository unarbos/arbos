# Grok convergence loop

Measures how far the Arbos agent is from Grok Bot on twelve *traits* (the way it talks: speed, ack first, cadence, formatting, offer, threading, busy reply, status line, hidden worker, stop, when to delegate, no handle), picks the largest gap, runs one change at a time as an A/B behind a kill switch, and keeps it only when it clears the noise. Design and ledger live in the project store (`docs/grok-convergence-loop.md`, `docs/grok-convergence-ledger.md`). Owner bc-c608fb3e. Box-sharing rules for the lt-eval sandbox: unarbos/arbos#1736.

Everything here reuses the long-task harness (`../long-tasks`): its rules (`lib/parity.py`), driver, box access and judge. Nothing is forked.

## Files

| file | what |
|---|---|
| `corpus/grok/<id>.jsonl` | the clean Grok Bot corpus, one message per line (`ts`, `role user|grok|worker`, `text`, `msg_id`, `reply_to`, `thread`, `status_line`, `attachments`, `chat_kind`, `participants`). Clean data only. Seed: the three baselines pj1–pj3. |
| `corpus/index.json` | per conversation: source, linked task, per-prompt trait tags; Grok's pass share per measured trait |
| `traits.py` | the trait rules applied to a Grok conversation — the same regexes and limits as `lib/parity.py` applies to Arbos |
| `ingest.py` | store `corpus/grok/raw/*.jsonl` (or markdown `--- N ---` blocks) → clean corpus + index. Drops arrive PII-scrubbed; the pass here (emails, phones, participant names, URL ids, attachment names, platform ids) is a safety net and writes no mapping anywhere. `--from-baselines` seeds from `../long-tasks/baselines/grok/*.md`. |
| `build_tasks.py` | corpus → `../long-tasks/tasks/convergence/cv-<id>.json` (parity-style scripted chats whose shapes are Grok's own turn) + `../long-tasks/baselines/grok/cv-<id>.md` for the judge. Prompts that need the user's file or would message someone outside are written `replayable: false` and skipped. |
| `ab.py` | A/B or A/A runner: both arms on the lt-eval box in projects `conv-a` / `conv-b`, one prompt at a time, order alternating per prompt. Arm B = `--arm-b-model vendor/name[:effort]` (PATCH `models.talking_model`, restored after) and/or `--arm-b-switch name=value` (box flag file `/work/.arbos/<name>`, removed for arm A). Refuses to start when the estimate plus the reserve for the day's remaining scheduled runs would cross the relay cap. Writes `scoreboard/<run>.json|.md`. |
| `noise.py` | noise band per trait: from an A/A run (`--ab RUN`) or from repeat parity runs (default: the four 2026-10-24/25 parity scorecards); floor 1/n. Writes `noise.json`. |
| `decide.py` | keep/drop verdict for an A/B run against `noise.json`, and the ledger row (`--write-ledger` appends it to the store ledger). |

## In the scheduled runs

`scripts/evals/long-tasks/run-suite.sh --subset convergence` runs the subset (the `cv-*` tasks plus the parity replays for steer, stop, parallel asks and busy — ≈ 10 tasks, ≈ 15 min, ≈ $1.5) and the scoreboard gets a `summary.convergence` block: per trait, Arbos pass share, Grok's share (measured from timestamps where the corpus has them, else *reference* — Grok's turn is the target — or *threshold* — the duplex design's §4.5 limits), the gap in percentage points, the largest gap, and cost per turn. The run report gains a "Grok convergence" section and `SUMMARY.md` a convergence table across runs. The full run already includes these tasks, so every 15:00 / 21:00 UTC scoreboard carries the block.

## One cycle by hand

```
python3 evals/convergence/ingest.py                      # new drops, if any (store corpus/grok/raw/)
python3 evals/convergence/build_tasks.py                 # new replay tasks + baselines
python3 evals/convergence/ab.py --cycle 1 --arm-b-model anthropic/claude-sonnet-5.5:low
python3 evals/convergence/decide.py --ab <run> --target ack_first --hypothesis "…(model)" --change "… · <kill switch>" --write-ledger
```

Secrets as for the long-task harness (`OP_SERVICE_ACCOUNT_TOKEN` → relay admin token, OpenRouter key, SSH key); state in `LT_STATE_DIR` (default `/tmp/lt-conv`, never another harness's `/tmp/lt`).
