# qa-016: the kernel's model API key is in every bash environment and lands in the transcript unredacted

status: fixed on the features branch `cursor/secrets-door-b027` (verified 2026-09-13 01:45Z: transcript and live frames clean; raw value remains only in the job's out.log on disk). Open on `rust`.
severity: high (security: any `env`, `printenv`, or crash dump in a bash tool call writes the provider key into transcript.jsonl and every attached client)
scenario: secrets-leak-hunt
rollout: /cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/qa/rollouts/20260913T013833Z-secrets-leak-hunt (rust; key scrubbed from the copy) and .../20260913T014500Z-secrets-leak-hunt@secrets-door (branch)
fingerprints: a1e1bec1bd 12051ee92a b8f9cd0d13 8cf7bf7b3c

## Repro

Kernel started with `OPENROUTER_API_KEY` in its environment (how every QA and desktop run sets the key). Prompt: run `env | sort` with bash.

## Expected

The key never appears in a tool result; bash inherits a scrubbed environment or the result is redacted.

## Actual (rust)

The `tool` event body holds `OPENROUTER_API_KEY=sk-or-...` in full; the live `event` frame carried it to the client. A granted test secret also went through plain and in every encoding tried (base64, spaced, reversed, hex, halves).

## On `cursor/secrets-door-b027`

Plain value and the API key are replaced by `[REDACTED:...]` in the transcript and in the live stream. Encoded forms (base64, spaced, reversed, hex, halves) still get through; the job's `out.log` on disk holds the raw value. Both are listed in the feature note as known.

## Suspected location

`crates/arbos-engine/src/tools/bash.rs` (child environment = kernel environment); no redaction step between tool output and `append_events`.
