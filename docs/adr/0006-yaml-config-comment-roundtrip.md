# ADR-0006 — Config format and comment round-tripping

- Status: Proposed
- Date: 2026-06-08

## Context

The Python original uses `ruamel.yaml` to **preserve user comments** when it
rewrites `config.yaml` (e.g. on `hermes config set`). Go's `yaml.v3` does not
round-trip comments cleanly. Options:

1. A comment-preserving YAML library (less mature in Go).
2. Accept losing comments on programmatic writes.
3. Split config: a hand-edited file (never rewritten) + a machine-managed file
   (rewritten freely, comments irrelevant).

## Decision

*Deferred to the config phase.* Leaning toward **(3)**: never programmatically
rewrite the user's hand-edited file; machine-set values go to a separate managed
file that layers over it. This sidesteps the round-trip problem entirely rather
than depending on an immature library.

## Consequences

To be recorded when accepted. Capturing now so the papercut you flagged is not
rediscovered mid-implementation.
