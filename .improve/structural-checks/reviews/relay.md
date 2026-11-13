# Independent relay durability/privacy review

PASS for the narrow source change; no actionable source defect found. This read-only review does not substitute for the owner's native gates or full workspace/GUI integration.

- The shared connection-level batch lookup preserves the exact room-scoped rows UNION ALL quarantine query. Both public lookup and append use it, without re-locking Db. Quarantined replay therefore keeps the original sequence.
- Append opens one transaction before dedupe/head/row operations, commits before either success return, and lets rusqlite's transaction drop roll back every propagated failure. Existing room floor and unrelated room rows are untouched. The mutex still serializes the same connection; no new connection or schema/migration was added.
- Failure-first native tests inject a real SQLite BEFORE INSERT abort, check the first-room position disappears or the previous head/stats/bytes remain unchanged, then remove the trigger and retry at the correct sequence. Positive replay/device/bytes/quarantine/room-scope controls stay meaningful. The final fixture removes the manually advanced compaction floor from the earlier test draft.
- Group privacy now permits only an explicit successfully read `anyone`, or successfully read `people_i_know` with an existing DM. Missing-row/NULL default remains in Db; database failure and unknown stored values refuse. The narrower read result does not alter caller identity or membership authorization.
- The four actual handler regressions isolate a policy-column read failure while user lookups remain usable, assert the existing generic 403 body and no partial chat/membership/invite writes. Existing known/nobody/anyone tests remain; the added unknown-policy test prevents the wildcard permissive fallback.

Reviewed immutable baseline 2865cb29 and the author's private relay-durability.patch/source; no edits or execution were performed by this reviewer.
