# ADR-0001 — Record architecture decisions

- Status: Accepted
- Date: 2026-06-08

## Context

The Python original baked load-bearing decisions into 14–16k-line files where
they were invisible and effectively irreversible. For the Go rewrite we want the
expensive-to-reverse decisions to be explicit, dated, and individually
revisitable.

## Decision

Use lightweight ADRs (this format) in `docs/adr/`. One decision per file,
numbered. Status is one of Proposed / Accepted / Superseded. An ADR is changed
by adding a new ADR that supersedes it, not by editing history.

Scope: only decisions that are costly to reverse (data shapes, port boundaries,
storage engine, distribution model). Day-to-day code choices do not need ADRs.

## Consequences

Decisions are reviewable in isolation. New contributors can read the ADR log to
understand *why* the kernel looks the way it does without reverse-engineering it.
