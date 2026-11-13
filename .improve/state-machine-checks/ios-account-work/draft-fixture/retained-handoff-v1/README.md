# Retained draft controller entrypoint follow-up

This separate three-file patch updates the existing public selected-method entrypoint, its aggregate runner's expected draft count and its README. It extracts the actual new private `DraftDMWork` type and capture method, compiles DEBUG fixture dispatch and retains the original nine live checks among eighteen total. The existing public command `run_all.py --only draft_sends` passed 18/18 in 1.238610 seconds; the produced actual-method fixture fingerprint equals the reviewed private 18-case runner fingerprint.

The unchanged `check_draft_mutants.py` needs no adapter change: it imports the unchanged production queue/lifetime API, not AppModel. Its two real-function mutations were rerun: stale attempt and closed-queue reopening each falsified on the first case, accepted one shrink and failed exact counterexample replay. Wrapper duration 5.977973 seconds. These are finite sampled properties, not whole-app proof.

The complete account-work suite is intentionally NOT rerun yet. Historical 296 counts remain historical; no new complete aggregate is claimed. Recompute it only after all final source fixes compose.

Two preparation errors were retained separately: the initial minimal private subset omitted the Xcode project file required by the existing aggregate source-hash guard (fixed by copying the exact unchanged C19 file), and the initial narrow PATH omitted the already-installed Node executable (fixed by including its existing directory). Neither reached an assertion or product effect. Both guards remain intact.

The product two-file patch and its public-v1 evidence remain frozen separately. No active app/simulator snapshot, live product source, Git index or reference was edited.
