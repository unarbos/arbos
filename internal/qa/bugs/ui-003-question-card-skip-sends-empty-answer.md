# ui-003: Question card "Skip" sends an empty answer; the model says it is blocked

status: new
severity: medium (Skip is a first-class button; two of three times the agent stopped with "your answer came through empty")
scenario: internal/parity/ui_pass.py phase Q (`ask-skip`, `ask-skip follow-through`)
found: UI QA pass 2026-09-13, `cursor/release-integration-52cd` @ 67dcb85 (on afa582a the card auto-resolves before it can be clicked, see qa-021)
feature: question card (`desktop/src/view/detail.rs::questions`, `ChatSession::skip_ask`/answer path; kernel `ask`/`answer` frames, PR #81)
fingerprints: none

## Repro

1. Send `Before doing anything else, use your ask tool to ask me ONE multiple-choice question with exactly two options, 'alpha' and 'beta', about which name to use. Wait for my answer, then reply with the chosen name.`
2. When the card shows, click **Skip** (`ask-skip`) without selecting an option.

## Expected

The card closes and the model is told the user skipped the question (e.g. an `answer` with a skipped marker), so it can carry on with a default or say "skipped".

## Actual

The card closes (`state.questions` → null). The kernel resolves the ask with an empty string. Model replies:
- run 1: `I'm blocked because your answer came through empty, so I don't know whether to use alpha or beta.`
- run 2: `I'm waiting for your choice: alpha or beta.`
- run 3: `alpha` (the model guessed).

## Suspected location

`detail.rs::questions` Skip → `chat.skip_ask` sends `Frame::Answer { text: "" }`. The kernel's ask tool returns `""` to the model with no note that the user declined.

## Evidence

- `media/qa-ui/integration-67dcb85/105-v-ask-card-0.png` → `media/qa-ui/integration-67dcb85/106-v-ask-after-skip-0.png` → `media/qa-ui/integration-67dcb85/107-v-ask-skip-settled-0.png`
- `media/qa-ui/integration-67dcb85/110-v-ask-skip-settled-1.png`
- `media/qa-ui/integration-67dcb85/035-ask-skip-answer.png` (the guess case)
