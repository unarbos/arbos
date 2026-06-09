# ADR-0007 — Self-update via atomic binary swap

- Status: Proposed
- Date: 2026-06-08

## Context

The Python original's `hermes update` is `git pull` + reinstall, which also
enables an "edit a .py and restart" dev loop. A single Go binary changes this:
update becomes "download new binary + atomic replace," which is simpler and
safer, but loses the edit-from-source loop.

## Decision

*Deferred to the distribution phase.* Plan:

- **Release**: download the new binary to a temp path, verify checksum/signature,
  atomically rename over the running binary (POSIX rename; Windows needs the
  move-on-reboot or rename-old dance).
- **Dev**: a `--dev` / source mode that runs via `go run ./cmd/arbos` (or an
  embedded interpreter for the script-tool tier) to retain fast iteration.

## Consequences

To be recorded when accepted. Simpler, integrity-checkable updates; explicit dev
mode replaces the implicit edit-and-restart loop.
