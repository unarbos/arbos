# Phase 17 — Opt-in coding approval policy

Back to [overview](./overview.md). Status: done.

## Goal

Turn the wired approval seam into a real, configurable coding `ApprovalPolicy`
that gates the mutating tools, defaulting OFF so pi's full-privileges behavior
(D2) is preserved, and enableable per run.

## Changes (as implemented)

- `internal/agent/pi/approval.go`: `CodingApprovalPolicy` (a `ports.ApprovalPolicy`)
  that requires approval for `write`, `edit`, and `bash` (configurable via a
  `Gated` set) and approves read-only tools. Off unless `Options.Approval` is set.
- `cmd/arbos`: a `-approve` flag sets `Options.Approval = CodingApprovalPolicy{}`.
  The one-shot loop handles `ApprovalRequest` with a stdin `y/N` prompt; a
  non-tty/EOF denies, so unattended runs stay safe. The control seam already
  carries approval requests/responses for interactive frontends.

## Verification

Static gate green, no new lints. Runtime: a read-only `ls` ran unattended, a
`write` emitted an `ApprovalRequest`; denying blocked the file, approving created
it.
