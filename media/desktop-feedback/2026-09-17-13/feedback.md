# 2026-09-17-13 — Increase font size needs to be closer to cursor in size.

- Sent: 2026-09-17 10:17 UTC
- App: 0.2.0 (1335) commit `f9b6089`
- Kernel: 0.2.0 `f9b6089c0b80` built 2026-09-17T09:03Z
- Machine: macos/aarch64, project ``
- Model: openrouter anthropic/claude-fable-5.1
- Report id: `20260917T101700Z-9c30`

## What he wrote

Increase font size needs to be closer to cursor in size.

## What came with it

- Screenshot: saved beside this file
- Trajectory: 21 lines, 7 tool calls, 1 failed (turn 74–94)
- Kernel log: 72 lines
- Transcript tail: 73 lines
- The app's own view: included
- Tool arguments and outputs: **he removed it**

**4 credentials removed** — kernel `{"blocks": 0, "secrets": 0, "tokens": 1, "values": 0}`, app `{"tokens": 1, "values": 2, "blocks": 0}`.

## The calls that failed

- `search` — OpenRouter web returned no sources; DuckDuckGo answered with a bot check (CAPTCHA) instead of results. Set EXA_API_KEY, BRAVE_API_KEY or TAVILY_API_KEY, or run the model through OpenRouter (its web plugin searches with the same key).

## What it was

_to fill in: the cause, not the symptom_

## Which build carries the fix

_written by `desktop-feedback.py fixed` once the pull request has merged green_
