---
cursor:
  subagentId: "bc-08d8261b-fea2-5075-9949-d45f6f9d4acc"
---

# `internal/` lost the iPhone loop's documents on 2026-09-16 — what went, what came back

A second loss on the same day as the `docs/` loss (`internal/store-docs-loss-2026-09-16.md`, the recovery worker's record). This one hit `internal/`.

## When

- 12:12 UTC — the loop edited `internal/mobile-findings.md` (row M-83) successfully.
- 12:14 UTC — the loop read `internal/mobile-coverage.md` and `internal/mobile-findings.md` in full (their content is the basis of the rebuild).
- 12:39 UTC — a write to `internal/mobile-coverage.md` failed: file not found. `ls internal/` returned 17 entries; `features-inbox/` and every `mobile-*.md` were gone. `media/`, `docs/`, `notes.md`, `internal/voice/`, `internal/qa/` untouched.

## What went (this loop's files)

| file | rebuilt? |
| --- | --- |
| `internal/mobile-findings.md` (ledger M-01…M-87) | partial — M-80 onward complete, M-01…M-79 as a summary + the rows that were open |
| `internal/mobile-coverage.md` | yes, complete |
| `internal/mobile-feedback-log.md` | yes, complete |
| `internal/mobile-testflight.md`, `internal/mobile-mac-host.md` | essentials merged into `internal/mobile-mac-host-and-testflight.md` |
| `internal/mobile-catalogue.md` (prompt/screen catalogue) | no |
| `internal/features-inbox/2026-09-15-mobile-project-identity-in-roster.md`, `…-mobile-testflight-feed-link.md`, `…-mobile-testflight-secrets-and-feed-link.md`, `2026-09-16-mobile-put-frame-and-history-paging.md`, `…-mobile-hub-drops-new-frame-fields.md`, `…-mobile-arboslife-build-ready.md`, `…-mobile-push-needs-apns.md` | no — all answered and acted on; the answers are in kernel #270/#272/#300/#301 and hub redeploys |
| `internal/features-inbox/2026-09-16-mobile-spoken-turn-channel-on-transcript.md` | yes (written 12:08, rewritten 12:50) |

Other workers' files in `internal/features-inbox/` (kernel and gateway answers, the ArbosLife cutover note, the phone token note) are gone too and are theirs to restore.

## Guard from here

After every write, the loop copies `internal/mobile-*.md` and `internal/features-inbox/*mobile*` to the Mac at `~/mobile-docs/` (`rsync`), so the next loss costs a copy, not the record. `internal/mirror-docs.sh` covers `docs/` only by convention; whether `internal/` ledgers should join a mirror is the coordinator's call.
