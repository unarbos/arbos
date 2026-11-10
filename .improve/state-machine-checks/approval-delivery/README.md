# Approval delivery ownership

`WaitDelivery` owns exactly Inline(sender), Polled or Deferred. Decision and park
share one transition function; active Approved publication requires decision and
one-use claim committed together in the actual Store transaction. SQL failure or
an ignored claim write rolls back both. Deferred keeps the existing canonical
one-use rerun. The pending mutex is released before all asynchronous effects.

Five original native semantic failures pin ignored/aborted claim writes, visible
unclaimed polled approval, a competing claimant and receiver loss after published
admission. The final actual approval suite passes40 cases, including12 new cases
and6 existing park controls. The new native finite transition law has64 traces of
16 events; it is sampled coverage, with no exhaustive or Effect proof claim.
`GATES.json` records exact hashes, commands, counts and timings. Use the repository's
normal locked/offline Rust test setup with synthetic HOME/store and an OS-enforced
loopback-only network fence; never point these tests at a real relay or account:

```sh
cargo test --locked --offline -p arbos-agentd 'approval::' -- --test-threads=2
cargo clippy --locked --offline -p arbos-agentd --all-targets -- -D warnings
```

The40-case pass uses identical source in a space-free owned temporary checkout.
Two existing shell-command fixtures fail on checkout paths containing spaces;
their test-only quoting correction is tracked for the final Tests pass. The
recorded earlier36P/2F result uses the pre-amendment38-case source, not the final
40-case source. No assertion or authorization matcher was weakened. Full final
workspace verification is separate.

A reviewer also caught a preservation bug in the first candidate: receiver loss
for Rejected incorrectly suppressed its deferred notification. Closing the receiver
at the actual SQL profile edge produced1P/1 semantic failure; the narrow amendment
restores notification for unclaimed non-approved decisions. Cancelled stays silent.
This candidate regression is distinct from the five original failures.

## Needs the owner

Review the deliberate fail-closed tradeoff before merging: lost inline delivery
after committed Approved admission leaves authority spent and requires fresh
approval. Observers may already have acted on Approved, so it is never restored or
automatically dispatched again. This guarantees at-most-once local admission, not
rollback or exactly-once remote effects. Existing standing/deferred/grace policy,
argument matching and approval scopes are preserved.
