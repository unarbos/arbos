# AUX-H9 shared Mac heavy-slot ownership

Risk: **High**. Shared heavy-job capacity and GUI exclusivity are operational
safety boundaries: two successful grants or a canceled, unreturned grant can
launch simultaneous heavy work, exhaust the shared host, or misdirect GUI input.
Affected production files are `scripts/runner/arbos-heavy-slot.sh`,
`gui-lock.sh`, and `xcrun-slot-guard.sh`. This candidate has not been installed
or exercised against a runner, simulator, app, or real shared lock.

Original storage coupled an implicit requesting/publishing/granted lifetime to
visible directories/files, request-PID filenames, an external owner PID, and
queue markers. Publication could precede complete metadata; acquisition was
not serialized against reclaim or another acquisition. Signal cleanup did not
own the publication effect. Reclaim could forget an unremoved lease while the
boot guard continued to admit it. These are actionable local defects, rather
than a missing-test or remote-service blocker.

The candidate uses three explicit, small runtime-tagged protocols:

- Heavy admission: `New -> Asking | Queued -> Claiming:<origin>:<path> ->
  Prepared -> Verified -> Published -> Granted:<path>`. Preparing requires
  bounded decimal owner/time plus a nonempty name, owner observations gate
  verification and grant, and refusals/unavailability/abandonment/timeouts/
  cancellation are terminal. Effects remain in the requester shell; only pure
  transition calculations use command substitution.
- Lease observation/retirement: live exempt/capped/uncapped, malformed held,
  expired/dead, retiring, retired/reclaimed, and cleanup-failed states feed the
  actual `lease_counts` projection. Retirement revokes boot authority before
  best-effort deletion. If both marker and deletion fail on a live lease, it
  remains held/counting; the implementation does not pretend it was revoked.
- GUI: Free/Stale/Mine/Other observations gate Taking/Waiting or
  Releasing/Denied, with explicit terminal outcomes. A canceled renewal restores
  the previous inode and mtime only if the current inode is its own publication.
  The boot guard separately constructs Owned/ExplicitLive admission from actual
  validated lease observations, and Denied/Passthrough/Authorized are terminal.

A permanent mutex inode sits outside removable leases. Native `lockf` on macOS
(or `flock` on Linux) excludes other processes while observing, publishing,
reclaiming, releasing, or rolling back. Function-scoped FD9 is released before
queue sleeps. The OS proof tests real inherited-FD ownership after the `lockf`
child exits and checks that the permanent inode survives. Unsupported hosts
without either primitive fail closed. The test's stdlib flock shim validates
adapter invocation; an actual Linux host/executable has not been tested here.

Day-one regressions pin complete publication, two-acquirer capacity and GUI
exclusion, dead/invalid owner rejection, storage failure, directory-only boot
escape rejection, canceled publication rollback, renewal identity preservation,
failed retirement/removal accounting, direct release path validation, and
missing primitive behavior. Controls preserve FIFO/fair/P0/priority/release
policy, real uncapped grants, 300/301-second idle boundary, lease exemptions,
no-wait/bounded waits, original argv passthrough, and release statuses. Final
isolated controller suite: **34/34 PASS**, 0 skips/errors, **15.980s**, hash-pinned
actual source before and after. Original expanded batch: **10 FAIL / 7 PASS**;
additional failure-first batches are individually retained and overlap.

Literal defaults and ten existing policy helpers are unchanged (CALIBRATION).
No priority, polling, timeout, simulator, release, owner-refusal-signal, or load
threshold has been recalibrated. Runtime-tagged Bash states are validated at
transitions, not compile-time algebraic data types. Native scripts and their
callers stay Bash3.2-compatible and add no package dependency.

Caller audit found only Mac-oriented consumers: iOS/release/capture workflows,
runner.sh, feedback rust-gate/mac-enter-smoke, and ios-perf/perf-rig. Existing
GITHUB_ACTIONS bypass and explicit **live** lease bearer escape remain; this is
shared-host coordination, not a new cross-user security principal system.
World-writable shared storage, PID reuse, already-dispatched physical work,
SIGKILL between publication and response, and atomic delivery of stdout plus
process exit remain actual ceilings. No claim of arbitrary host-failure-proof
or exactly-once grant delivery is made. Owner death and normal trappable
cancellation are checked at the actual affected edges.

Portable retained test command (macOS):
`python3 scripts/runner/tests/test_slot_lifecycle.py`.
Generated transition laws and actual-function mutant controls are a separate
retained check, not a substitute for these OS/controller regressions.
