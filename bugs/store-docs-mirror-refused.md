# store-docs-mirror-refused: the docs mirror refused or failed in a QA cycle

- First seen: 20260916T142326Z
- What it means: `internal/mirror-docs.sh` refuses to push when the store view looks damaged or `docs/` is gone (`internal/store-docs-mirror.md`). Either the store faulted again or a document was deleted. Check whether the loss is real, restore from `store-docs` (`bash internal/mirror-docs.sh restore <dir>`), tell Jacob.
- Detail: exit 3; store docs/: missing; branch: 21 files; missing in store: ['acceptance-journeys.md', 'arbos-mesh-design.md', 'cursor-coordinator-spec.md', 'cursor-coordinator-tools-appendix.md', 'cursor-parity-process.md', 'cursor-parity-report-2026-09-12.md', 'cursor-parity-report-2026-09-13.md', 'cursor-projects-research.md', 'cursor-vs-arbos-agent-model.md', 'desktop-call-mode-design.md', 'features-backlog.md', 'filesystem-state-design.md']; notes.md present: True; branch restored to `/home/ubuntu/arbos-qa/state/store-docs-restore-20260916T142326Z` (not copied into the store)

- seen again 20260916T152248Z: exit 1; store docs/: 21 files; branch: 21 files; missing in store: []; notes.md present: True

- seen again 20260916T225217Z: exit 3; store docs/: missing; branch: 22 files; missing in store: ['acceptance-journeys.md', 'arbos-mesh-design.md', 'cursor-coordinator-spec.md', 'cursor-coordinator-tools-appendix.md', 'cursor-parity-process.md', 'cursor-parity-report-2026-09-12.md', 'cursor-parity-report-2026-09-13.md', 'cursor-projects-research.md', 'cursor-vs-arbos-agent-model.md', 'desktop-call-mode-design.md', 'desktop-feedback-design.md', 'features-backlog.md']; notes.md present: True; branch restored to `/home/ubuntu/arbos-qa/state/store-docs-restore-20260916T225217Z` (not copied into the store)

- seen again 20260917T005557Z: exit 1; store docs/: 22 files; branch: 22 files; missing in store: []; notes.md present: True

- seen again 20260917T065715Z: exit 3; store docs/: missing; branch: 25 files; missing in store: ['acceptance-journeys.md', 'arbos-mesh-design.md', 'cursor-coordinator-spec.md', 'cursor-coordinator-tools-appendix.md', 'cursor-parity-process.md', 'cursor-parity-report-2026-09-12.md', 'cursor-parity-report-2026-09-13.md', 'cursor-projects-research.md', 'cursor-vs-arbos-agent-model.md', 'desktop-call-mode-design.md', 'desktop-feedback-design.md', 'features-backlog.md']; notes.md present: True; branch restored to `/home/ubuntu/arbos-qa/state/store-docs-restore-20260917T065715Z` (not copied into the store)

- seen again 20260917T072253Z: exit 1; store docs/: 25 files; branch: 25 files; missing in store: []; notes.md present: True

- seen again 20260917T074553Z: exit 1; store docs/: 25 files; branch: 25 files; missing in store: []; notes.md present: True

- seen again 20260917T094406Z: exit 3; store docs/: 0 files; branch: 25 files; missing in store: ['acceptance-journeys.md', 'arbos-mesh-design.md', 'cursor-coordinator-spec.md', 'cursor-coordinator-tools-appendix.md', 'cursor-parity-process.md', 'cursor-parity-report-2026-09-12.md', 'cursor-parity-report-2026-09-13.md', 'cursor-projects-research.md', 'cursor-vs-arbos-agent-model.md', 'desktop-call-mode-design.md', 'desktop-feedback-design.md', 'features-backlog.md']; notes.md present: False; branch restored to `/home/ubuntu/arbos-qa/state/store-docs-restore-20260917T094406Z` (not copied into the store)

- seen again 20260917T095235Z: exit 3; store docs/: 0 files; branch: 25 files; missing in store: ['acceptance-journeys.md', 'arbos-mesh-design.md', 'cursor-coordinator-spec.md', 'cursor-coordinator-tools-appendix.md', 'cursor-parity-process.md', 'cursor-parity-report-2026-09-12.md', 'cursor-parity-report-2026-09-13.md', 'cursor-projects-research.md', 'cursor-vs-arbos-agent-model.md', 'desktop-call-mode-design.md', 'desktop-feedback-design.md', 'features-backlog.md']; notes.md present: False; branch restored to `/home/ubuntu/arbos-qa/state/store-docs-restore-20260917T095235Z` (not copied into the store)

- seen again 20260917T095910Z: exit 3; store docs/: 0 files; branch: 25 files; missing in store: ['acceptance-journeys.md', 'arbos-mesh-design.md', 'cursor-coordinator-spec.md', 'cursor-coordinator-tools-appendix.md', 'cursor-parity-process.md', 'cursor-parity-report-2026-09-12.md', 'cursor-parity-report-2026-09-13.md', 'cursor-projects-research.md', 'cursor-vs-arbos-agent-model.md', 'desktop-call-mode-design.md', 'desktop-feedback-design.md', 'features-backlog.md']; notes.md present: False; branch restored to `/home/ubuntu/arbos-qa/state/store-docs-restore-20260917T095910Z` (not copied into the store)

- seen again 20260917T101414Z: exit 3; store docs/: 0 files; branch: 25 files; missing in store: ['acceptance-journeys.md', 'arbos-mesh-design.md', 'cursor-coordinator-spec.md', 'cursor-coordinator-tools-appendix.md', 'cursor-parity-process.md', 'cursor-parity-report-2026-09-12.md', 'cursor-parity-report-2026-09-13.md', 'cursor-projects-research.md', 'cursor-vs-arbos-agent-model.md', 'desktop-call-mode-design.md', 'desktop-feedback-design.md', 'features-backlog.md']; notes.md present: False; branch restored to `/home/ubuntu/arbos-qa/state/store-docs-restore-20260917T101414Z` (not copied into the store)

- seen again 20260917T102919Z: exit 3; store docs/: 0 files; branch: 26 files; missing in store: ['acceptance-journeys.md', 'arbos-mesh-design.md', 'call-mode-work-sound.md', 'cursor-coordinator-spec.md', 'cursor-coordinator-tools-appendix.md', 'cursor-parity-process.md', 'cursor-parity-report-2026-09-12.md', 'cursor-parity-report-2026-09-13.md', 'cursor-projects-research.md', 'cursor-vs-arbos-agent-model.md', 'desktop-call-mode-design.md', 'desktop-feedback-design.md']; notes.md present: False; branch restored to `/home/ubuntu/arbos-qa/state/store-docs-restore-20260917T102919Z` (not copied into the store)

- seen again 20260917T104424Z: exit 3; store docs/: 0 files; branch: 26 files; missing in store: ['acceptance-journeys.md', 'arbos-mesh-design.md', 'call-mode-work-sound.md', 'cursor-coordinator-spec.md', 'cursor-coordinator-tools-appendix.md', 'cursor-parity-process.md', 'cursor-parity-report-2026-09-12.md', 'cursor-parity-report-2026-09-13.md', 'cursor-projects-research.md', 'cursor-vs-arbos-agent-model.md', 'desktop-call-mode-design.md', 'desktop-feedback-design.md']; notes.md present: False; branch restored to `/home/ubuntu/arbos-qa/state/store-docs-restore-20260917T104424Z` (not copied into the store)

- seen again 20260917T115010Z: exit 3; store docs/: 0 files; branch: 27 files; missing in store: ['acceptance-journeys.md', 'arbos-mesh-design.md', 'call-mode-work-sound.md', 'cursor-coordinator-spec.md', 'cursor-coordinator-tools-appendix.md', 'cursor-parity-process.md', 'cursor-parity-report-2026-09-12.md', 'cursor-parity-report-2026-09-13.md', 'cursor-projects-research.md', 'cursor-vs-arbos-agent-model.md', 'desktop-call-mode-design.md', 'desktop-feedback-design.md']; notes.md present: False; branch restored to `/home/ubuntu/arbos-qa/state/store-docs-restore-20260917T115010Z` (not copied into the store)

- seen again 20260917T120516Z: exit 3; store docs/: 0 files; branch: 27 files; missing in store: ['acceptance-journeys.md', 'arbos-mesh-design.md', 'call-mode-work-sound.md', 'cursor-coordinator-spec.md', 'cursor-coordinator-tools-appendix.md', 'cursor-parity-process.md', 'cursor-parity-report-2026-09-12.md', 'cursor-parity-report-2026-09-13.md', 'cursor-projects-research.md', 'cursor-vs-arbos-agent-model.md', 'desktop-call-mode-design.md', 'desktop-feedback-design.md']; notes.md present: False; branch restored to `/home/ubuntu/arbos-qa/state/store-docs-restore-20260917T120516Z` (not copied into the store)

- seen again 20260917T120649Z: exit 3; store docs/: 0 files; branch: 27 files; missing in store: ['acceptance-journeys.md', 'arbos-mesh-design.md', 'call-mode-work-sound.md', 'cursor-coordinator-spec.md', 'cursor-coordinator-tools-appendix.md', 'cursor-parity-process.md', 'cursor-parity-report-2026-09-12.md', 'cursor-parity-report-2026-09-13.md', 'cursor-projects-research.md', 'cursor-vs-arbos-agent-model.md', 'desktop-call-mode-design.md', 'desktop-feedback-design.md']; notes.md present: False; branch restored to `/home/ubuntu/arbos-qa/state/store-docs-restore-20260917T120649Z` (not copied into the store)

- seen again 20260917T224400Z: exit 2; store docs/: 28 files; branch: 28 files; missing in store: []; notes.md present: True

- seen again 20260918T032350Z: exit 1; store docs/: 29 files; branch: 29 files; missing in store: []; notes.md present: True

- seen again 20260918T034758Z: exit 1; store docs/: 29 files; branch: 29 files; missing in store: []; notes.md present: True

- seen again 20260918T040423Z: exit 1; store docs/: 29 files; branch: 29 files; missing in store: []; notes.md present: True

- seen again 20260918T042110Z: exit 2; store docs/: 29 files; branch: 29 files; missing in store: []; notes.md present: True

- seen again 20260918T042839Z: exit 2; store docs/: 29 files; branch: 29 files; missing in store: []; notes.md present: True

- seen again 20260918T043743Z: exit 2; store docs/: 29 files; branch: 29 files; missing in store: []; notes.md present: True

- seen again 20260918T045417Z: exit 1; store docs/: 29 files; branch: 29 files; missing in store: []; notes.md present: True

- seen again 20260918T051016Z: exit 2; store docs/: 29 files; branch: 29 files; missing in store: []; notes.md present: True

- seen again 20260918T052637Z: exit 1; store docs/: 29 files; branch: 29 files; missing in store: []; notes.md present: True

- seen again 20260918T054356Z: exit 1; store docs/: 29 files; branch: 29 files; missing in store: []; notes.md present: True

- seen again 20260918T060146Z: exit 2; store docs/: 29 files; branch: 29 files; missing in store: []; notes.md present: True

- seen again 20260918T061833Z: exit 1; store docs/: 29 files; branch: 29 files; missing in store: []; notes.md present: True

- seen again 20260918T063436Z: exit 2; store docs/: 29 files; branch: 29 files; missing in store: []; notes.md present: True

- seen again 20260918T065057Z: exit 1; store docs/: 29 files; branch: 29 files; missing in store: []; notes.md present: True

- seen again 20260918T073816Z: exit 129; store docs/: 29 files; branch: 29 files; missing in store: []; notes.md present: True

- seen again 20260918T075500Z: exit 1; store docs/: 29 files; branch: 29 files; missing in store: []; notes.md present: True

- seen again 20260918T081118Z: exit 1; store docs/: 29 files; branch: 29 files; missing in store: []; notes.md present: True

- seen again 20260918T082756Z: exit 1; store docs/: 29 files; branch: 29 files; missing in store: []; notes.md present: True

- seen again 20260918T090123Z: exit 1; store docs/: 29 files; branch: 29 files; missing in store: []; notes.md present: True

- seen again 20260918T101537Z: exit 2; store docs/: 30 files; branch: 30 files; missing in store: []; notes.md present: True

- seen again 20260918T123436Z: exit 1; store docs/: 32 files; branch: 32 files; missing in store: []; notes.md present: True

- seen again 20260918T135638Z: exit 2; store docs/: 35 files; branch: 35 files; missing in store: []; notes.md present: True

- seen again 20260918T145358Z: exit 1; store docs/: 36 files; branch: 36 files; missing in store: []; notes.md present: True
