# store-docs-mirror-refused: the docs mirror refused or failed in a QA cycle

- First seen: 20260916T142326Z
- What it means: `internal/mirror-docs.sh` refuses to push when the store view looks damaged or `docs/` is gone (`internal/store-docs-mirror.md`). Either the store faulted again or a document was deleted. Check whether the loss is real, restore from `store-docs` (`bash internal/mirror-docs.sh restore <dir>`), tell Jacob.
- Detail: exit 3; store docs/: missing; branch: 21 files; missing in store: ['acceptance-journeys.md', 'arbos-mesh-design.md', 'cursor-coordinator-spec.md', 'cursor-coordinator-tools-appendix.md', 'cursor-parity-process.md', 'cursor-parity-report-2026-09-12.md', 'cursor-parity-report-2026-09-13.md', 'cursor-projects-research.md', 'cursor-vs-arbos-agent-model.md', 'desktop-call-mode-design.md', 'features-backlog.md', 'filesystem-state-design.md']; notes.md present: True; branch restored to `/home/ubuntu/arbos-qa/state/store-docs-restore-20260916T142326Z` (not copied into the store)

- seen again 20260916T152248Z: exit 1; store docs/: 21 files; branch: 21 files; missing in store: []; notes.md present: True

- seen again 20260916T225217Z: exit 3; store docs/: missing; branch: 22 files; missing in store: ['acceptance-journeys.md', 'arbos-mesh-design.md', 'cursor-coordinator-spec.md', 'cursor-coordinator-tools-appendix.md', 'cursor-parity-process.md', 'cursor-parity-report-2026-09-12.md', 'cursor-parity-report-2026-09-13.md', 'cursor-projects-research.md', 'cursor-vs-arbos-agent-model.md', 'desktop-call-mode-design.md', 'desktop-feedback-design.md', 'features-backlog.md']; notes.md present: True; branch restored to `/home/ubuntu/arbos-qa/state/store-docs-restore-20260916T225217Z` (not copied into the store)
