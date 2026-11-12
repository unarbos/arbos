# Deployment request and effect ownership

The deploy hook publishes a minimum requested commit. Its entire read/debounce/write
transaction holds the queue's persistent `.queue.lock`. The worker evaluates a coherent
private snapshot, then acknowledges it under the same short lock. It deletes the queued
file only if its bytes still match that snapshot. A newer request remains queued while
the older deployment completes. Duplicate SHA and first-request deadline behavior stay
the same. Malformed, oversized, duplicate-key or mismatched-kind requests are retained
without reaching an effect.

Automatic worker and feedback-poller publication also bind their decision to a hook
observation. `--observe <kind>` returns a bounded receipt containing the kind, per-kind
revision, queued commit, installed commit and registered default commit. The hook
initializes missing legacy revision metadata under the queue lock. A conditional third
argument is accepted only if that complete observation still matches, after the caller's
ancestry/path checks. Changed or unavailable observations defer instead of falling back
to unconditional publication. Manual two-argument arrival order remains unchanged.

Each actual publish/ack advances a checked revision before mutating the queue. An
absent→queued→acknowledged→absent cycle therefore invalidates an older decision even
when its queue and installed values return to their previous shape. A failed revision
write prevents mutation; a later queue failure leaves a conservatively newer revision.
Malformed/unreadable or exhausted revisions fail closed rather than reset or wrap.
Duplicate-SHA no-ops do not change the debounce times or revision.

These receipts are ephemeral local decisions, not authentication tokens or remote
transactions. Public installed/default values are re-read without git/network work
under the short queue lock. Independent existing status publishers and later repository
or CI changes still have their own owners; the worker's final on-main/CI/deployed checks
remain. The fence does not assert monotonicity for manual/legacy writers that bypass it
or manually remove/restore its revision metadata.

Relay deployment, canary smokes and Caddy reloads hold one persistent descriptor lease
on the state directory's `.lane.lock` for their actual effect. A marker check followed
by a marker write cannot grant admission. The markers remain status observations and
blockers for older participants; their expiry never releases an active descriptor lease.
An effect's failure or cancellation cleans up its own marker and descriptor. The lock
files are never unlinked or replaced by these controllers.

Image activation keeps that same lease after its smoke, through the existing config/
default POST and non200 relay-restart fallback, including the restart notice, grace,
health check and public default marker. Build/registration precede this interval;
box rollout follows it. The activation callback advances the queue revision before
changing the default, so an automatic decision cannot reuse its older receipt in the
gap before the public default marker is written. A failed revision write stops before
config/default mutation. Restart/config failures retain their prior failure barrier
even though this body is now called through a conditional lease callback.

`deploy-state.sh` has two separate transition functions used at these boundaries:

- Request receipt: `Observed → Acknowledged | Superseded`; only `Acknowledged` can
  remove a queued request.
- Effect admission: `Checking → Inspecting → Granted → Released`, or `Deferred`.
  A busy lock or fresh conflicting observation defers admission.

These states do not replace systemd unit state, manual image holds, per-box pins,
quota or independent server observations.

A canary's default 900-second admission budget now ends in `Deferred`: no smoke,
default switch or image hold, and the request remains queued. Previously the worker
ran a canary despite continued contention. This safety change can delay a roll under
long relay/reload activity. Unexpected smoke execution failures also stop the attempt;
external status75 is normalized to76 because75 is reserved for admission deferral.
Existing status1 image-failure/manual-hold and status2 inconclusive policies are preserved.

Dry tick establishes its private state/queue copy before observing or expiring any
lane marker. Its existing shared repository fetch behavior is unchanged.

Both existing installation paths install the shared helper beside their entry points:
the deploy-hook installer supplies it for hook/worker, and relay-server supplies it for
refresh-mac-download. Exclusive admission assumes every participating installed entry
point uses this helper. A running older process has no descriptor lease; fresh markers
remain a compatibility precaution, not a proof of atomic exclusion with old code.
No install, deployment, reload, tag or service operation is performed by the tests.

## Local checks

```sh
python3 -I -B scripts/deploy/test_deploy_lanes.py
python3 -I -B scripts/deploy/test_deploy_publication.py
python3 -I -B scripts/deploy/test_image_activation.py
node --test harness/arbos-pi-ext/deploy-state-laws.test.ts
python3 -I -B scripts/deploy/tests/check_state_mutants.py --record-dir /tmp/arbos-deploy-mutations
```

The stdlib adapters execute selected actual function bodies over generated files and
explicit command collaborators. Linux uses the installed `flock`/GNU `stat`; macOS
uses a small stdlib proxy for those CLI contracts and its real kernel `flock` operation.
The macOS sandbox denies network, deployment configuration/state reads and writes
outside the generated fixture. Every fixture process group has a bounded lifetime.

The generated checks call only the actual pure transition/receipt definitions in an empty-PATH
restricted Bash process through a bounded JSONL bridge. They assert superseded receipt
refusal, lock-before-grant, terminal refusal and admission bound to every observed field,
with positive grant/ack/publication controls. They
do not source the worker's top level, import a provider SDK or execute filesystem/PID/
deployment effects. Adapter tests separately cover descriptor ownership and the actual
callback/default-switch boundary.
