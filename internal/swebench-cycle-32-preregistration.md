---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# SWE-bench loop, cycle 32: pre-registration (written before the run)

Written 2026-09-18 16:20 UTC, before the run.

## What this cycle is

New material for the account: the next ten never-run instances (`fresh10d.txt`: 4 "<15 min", 6 "15 min–1 hour") at `-r 2`, 20 rollouts, every failure read against the account. Not a measurement.

## Kernel and a condition worth stating

Kernel `a8678ac16636` (cycles 30–31's). `main` has moved (fba8688d) but the four new commits are all Jev — the router model that picks the next mechanical step and, since a47c5104, ends the turn when it fails. **Jev is off under this harness**: `jev_enabled` is on by default only when the provider is OpenRouter, and the harness sets `provider = custom` (the interception endpoint). Checked in cycle 31's traces: 772 provider calls, every one `purpose: turn`, none `jev`. So the loop has measured, and keeps measuring, the one-model loop — which since dcd8dba6 (01:23 today) is no longer what a desktop user on OpenRouter runs. Rebuilding for the Jev commits would change nothing here; the kernel stays.

Network cut, sweep, one reproduction, $8 cap. Cap $22.
