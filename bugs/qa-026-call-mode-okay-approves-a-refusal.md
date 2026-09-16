# qa-026: call mode allows a command on "Okay, wait, don't run that" (safety)

- Feature: call mode, voice gateway narrator (`voice-server/`, PR #100 `cursor/call-mode-narrator-cf3f` @ `d9007f8`; also what the pod gateway ran until 17:24 UTC)
- Severity: high. The safety list says the gateway never auto-approves; here it approved on a refusal.
- Status: fixed in [#111](https://github.com/unarbos/arbos/pull/111); pod gateway redeployed with the fix 2026-09-13 17:24 UTC.

## Repro

`cd voice-server && source .venv/bin/activate && python -m tests.run approval-okay-is-not-consent approval-question-keeps-it-open`

1. The caller says "Clean the build directory." The agent asks to run `bash: rm -rf target && cargo build`.
2. The narrator speaks "Arbos wants to run bash: rm -rf target && cargo build. Allow?"
3. The caller says **"Okay, wait, don't run that."** (or **"Sure, but what does that command do exactly?"**)

## Expected

Denied (the sentence refuses), or the approval stays open (the sentence asks a question). Never allowed.

## Actual

`Allowed.` The kernel received `approve {allow: true}` and the command ran; the agent then reported "Cleaned the build directory and rebuilt".

Rollouts: `voice-server/tests/out/approval-okay-is-not-consent/` (frames.jsonl: `narrator.say Allowed.` at 7.7 s), `approval-question-keeps-it-open/`.

## Cause

`voice_server/narrator.py`: `_YES = ^\W*(yes|yeah|sure|ok(?:ay)?|fine|...)\b` was checked first, on the first word only. "Okay" and "sure" open most spoken sentences. Negation later in the utterance was never looked at.

## Fix (#111)

`consent(text)`: a deny word anywhere wins; a yes counts only when the utterance starts with a yes-word, is at most 8 words, and carries no hold-off or question word (`but`, `first`, `wait`, `what`, `why`, `?`). Otherwise the approval stays open and the words go to the agent. A yes is also ignored until the approval line has started playing (the caller cannot allow a question they have not heard).

Regression scenarios: `approval-okay-is-not-consent`, `approval-question-keeps-it-open`.
