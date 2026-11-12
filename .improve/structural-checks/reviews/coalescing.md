# Independent Mac snapshot-schedule review

Verdict: PASS for the scoped extraction; no actionable defect found. Full GPUI integration remains separately gated.

- The actual `forward_changes` list-change edge delegates to `SnapshotSchedule::list_changed`; its timer branch delegates to `timer_fired`. Project-document processing and immediate message upserts remain on their prior branches.
- First catch-up change opens the unchanged 80 ms window. Subsequent burst events keep its original deadline. Live changes clear pending work and paint immediately. A late timer clears its deadline and suppresses a duplicate when LinkLive already marked the room caught up. Receiver closure still exits.
- Tokio time is supplied with `tokio::time::Instant::now().into_std()`. The cached Tokio implementation stores the same std Instant inside the wrapper; `into_std` unwraps it and `From<std::time::Instant>` wraps it unchanged. `sleep_until(deadline.into())` therefore keeps the Tokio clock, including paused clocks; this does not substitute the system wall clock.
- Seven retained tests exercise the production scheduling reducer, including positive immediate paints, pending clearing, an already-due deadline and 2,000 backfill changes. The removed four tests exercised two separate test-only scheduling models. Root retained actual-source mutant failures for always-paint, postponed deadlines and never-timer.

Read-only source hashes:
- `crates/arbos-mac/src/snapshot_schedule.rs`: `456a1e2908547917caf5f04eda2595d518be30000152486641722e1ab9ee5154`
- `crates/arbos-mac/src/sync/relay.rs`: `c48cdaaa4d3e0c5b95465420035ba36e0bbe34fd8d64f9470855bab67006342f`
- `crates/arbos-mac/src/main.rs`: `eebdb39406eb365e95f267575d2b226c92249adf475115ebdab60c0638665bbc`
