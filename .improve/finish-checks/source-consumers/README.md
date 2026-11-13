# Actual source consumer contracts

The existing macOS Swift CI job runs this small check after selecting Xcode and before building Rust/FFI. It compiles actual production call expressions/callback bodies with inert local collaborators, then executes with a fresh environment and network denied. It needs only Python3, installed Swift and macOS sandbox-exec; no new job, dependency, account, app or device is used.

```sh
python3 .improve/finish-checks/source-consumers/check.py --tree . --out /tmp/arbos-source-consumers-new
```

The output path must be new. Source/fixture hashes and exact assertion counts are emitted. The checks fail if the expected source boundaries move or source changes during execution.

The six row input-contract checks are extended with23 actual callback checks (29 total). The unmodified closeSearch/openSearchResult, RowView.openProfile and status-row onPicture bodies execute with the real ChatAccountContext, WorkLifetime and search rules. Controls cover current/fixture owners, result-row suppression, retired/unavailable/cancelled work, close/reset/blur before the unchanged0.05s reveal, and retirement before a queued reveal. Scheduler and reference-backed view-state collaborators avoid real UI/timing. Five negative controls detect a missing row argument, lost result suppression, late reveal after retirement, unguarded picture action and missing close. This is selected-source contract coverage, not SwiftUI rendering; native model/renderer and existing UI behavior tests remain separate and unchanged. Only four brittle formatting assertions are migrated.
