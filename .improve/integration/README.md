# Review and reproduce the phase composition

All ten phase PRs target the recorded upstream main base independently. Review
them in the order listed in [the report](../REPORT.md). Nothing was merged or
deployed. Their overlapping files are deliberate: a blanket ours/theirs conflict
resolution can discard ownership, privacy or performance fixes.

[The versioned assembler](assembly/README.md) pins immutable phase3–9 code inputs.
It reads Git blobs, applies guarded private-index changes, materializes a new local
source tree and records every source hash/mode. It never fetches, merges, commits,
updates refs, pushes, runs the app or contacts a network. Run only after reviewing
the pins and assets; missing inputs, moving labels or unknown conflicts fail.

The important overlaps are explicit:

- Phase4 carries Phase3 policies and shared test repairs. Twelve differing
  supersets have exact source/diff guards; identical blob/mode copies carry once.
- Phase6 upload concurrency layers onto the owned/local-fixture-aware AppModel.
  Its standalone main-based file cannot replace that ownership implementation.
- Phase6 relay/API and Phase7 people changes preserve earlier lifetime/privacy
  behavior through narrowly reviewed deltas.
- The canonical redaction patch combines the Phase8 credential-boundary repair
  with Phase6 cached matchers, preserving actual expected outputs.
- Common Swift/Rust runtime fences remain identical where required. The new
  identical Phase3/4 consumer workflow carries once; Phase9 retains all tests
  while adding only the three free CI fixes.

The recipe produces source, rather than generated FFI binaries, SDKs or caches.
Build artifacts have their own pinned ABI/native receipts. Its Phase9 code pin
contains historical report metadata; this later publication supplies the final
report/recipe/results without changing those tested product-source inputs.
Compilation, tests and benchmark receipts are separate from assembly success.
See [source projection](../verification/source-projection/README.md) for the exact
relationship to the completed Linux and native executions.

Synthetic local profiles do not establish live-user chat-open/send latency.
Telemetry remains owner-deferred for discussion with Jacob (const).
