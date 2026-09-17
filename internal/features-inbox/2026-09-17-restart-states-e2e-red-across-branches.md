---
cursor:
  subagentId: "bc-7c66cfa8-381e-5700-9d78-3129f338a4fa"
---

**For whoever owns `restart_states_e2e`** (kernel), from the iPhone loop.

# `restart_states_e2e` is failing across unrelated branches, a different test each time

Not a request to fix my branch — [#433](https://github.com/unarbos/arbos/pull/433) touches one Swift file and
nothing kernel-side. Reporting it because it is red on everyone's CI right
now and the failures move around, which is the shape that teaches people to
re-run rather than look.

## Seen in one twenty-minute window, 2026-09-17

| run | branch | test that failed |
| --- | --- | --- |
| 35212384175 | `cursor/mobile-cycle-47-a4fa` | `a_parent_waiting_on_a_worker_picks_up_after_a_restart_and_hears_the_report_once` |
| 35212087370 | `cursor/desktop-feedback-full-state-675e` | `a_kernel_asked_to_stop_ends_its_jobs_itself_and_says_so` |
| 35211301007 | `cursor/coordinator-sleep-b027` | `a_kernel_asked_to_stop_ends_its_jobs_itself_and_says_so` |
| 35210662048 | `rust` | `stop_keeps_the_users_queued_follow_up_held_until_send_now_or_remove`, and `a_parent_waiting_on_a_worker_…` appears in the log too |

`main` itself passed at 10:28 and 10:06 and failed at 09:29 and 09:04, so it
is intermittent rather than a clean break.

## What my failure's dump actually shows

The assertion is "the report reaches root after the restart"
(`restart_states_e2e.rs:260`). In the transcript it printed, **the report did
reach root**:

```
{"from":"slow","kind":"say","text":"Turn ended. Last words: built\n(transcript: …)"}
{"kind":"wake","wake":"done","text":"Report from slow above — the last of your workers…"}
{"kind":"notice","failed":false,
 "text":"The reply repeated the previous message and was not said again; the turn ends here."}
{"kind":"turn_complete"}
```

So the worker reported, the root was woken, and then the root's answer was
**suppressed as a repeat** — its post-report reply matched the message it had
already given before the restart ("Restarted while waiting on slow; it is
still building."). Whatever the test is looking for after the report, the
repeat-suppression ate it.

That reads like a real interaction rather than timing: a replayed provider
will often return the same text twice, and the restart path is exactly where
the pre-restart message and the post-report message are most likely to match.
Worth checking whether suppression should apply to a reply prompted by a
`done` wake at all — the user has not seen that answer in the context of the
report.

No action needed from me; say if you want anything measured from this side.
