# Bounded independent UI/control review

Scoped verdict: PASS for the final frozen UI/control/reset implementation and its
21 combined controller/helper checks. This is source/evidence review, not new
SDK/device execution. UI-FROZEN.json identifies the final eight files, including
the LocalView reset, same-control account-origin fence and compact event errors.
Backend relay.rs is separately owned and not certified by this review.

BindUi precedes source work. UiControl clones share one lifetime, so local
signout/auth renewal synchronously invalidates queued control receipts. UiSession
separates Waiting from the active backend ticket. Commands retain that backend;
call commands use the attempt's original backend rather than current UI binding.
Fixture sources retain bare commands and explicitly reject local envelopes.
Queued account/control events are admitted before payload application. A genuine
SessionStarted resets account view/cache/form state; same-owner starts preserve it.
Consecutive revocation messages retain admission until NeedsPairing clears it.
Local signout captures the previous backend, closes UI admission before queueing
SignOut, then clears views. Separate call retirement owns finite cleanup.

The initial review found one real gap in control-only account callbacks: backend
A→B adoption may retain the same UI control. Connect-status and the delayed opt-in
rig callback now require both captured control and backend account. Actual same-
control callback regressions failed before this correction; the generated law
and actual-function deletion mutant also exercise it. Browser/auth timeout work
appropriately remains control-scoped. Ordinary quote/silence timers check current
state rather than dispatching stale private account commands.

LocalView is authenticated GET /local/devices data, not global host telemetry;
it now resets with transcripts, private forms and caches before an empty B can
paint A data. Settings, updater/window ownership, notification permission policy
and monotonic view/request IDs remain global. Pending notification coalescing
resets; no withdrawal of already-delivered OS notifications is claimed. New
account fields still require explicit constructor/reset classification, documented
in the reset inventory. The apply panic-boundary comment is restored to apply.

Matrix credentials use an owner-bound API and expected-owner refresh before the
first await; startup/listeners/snapshot publication also check account+attempt.
TaskOwner's Open/Closed admission, abort/detach and future disposal keep destruction
outside its mutex. SessionEvents retains one immutable backend ticket; the
already-admitted method avoids re-locking its caller's account guard. Cold send
errors are boxed without logging payloads or suppressing diagnostics.

Seven actual native helper laws execute1792seeded traces plus protocol recovery;
eight isolated actual-function mutants falsify, shrink and replay, including
deleting the account-origin check. These are finite checks and do not certify
every GUI callback, Keychain/OS notification interaction or physical media path.
Full backend spawn wiring, source/coordinator RAII retirement, control/account
lock order, call membership cleanup and complete native composition remain their
own gates. No production service, real credential or physical rig was exercised.
