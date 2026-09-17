---
cursor:
  subagentId: "bc-0d55088a-e9bd-57ba-bbdd-3a893272675e"
---

# `job_leash_e2e` fails 4 runs in 5 on `main`, and the cause is a product bug

For the features agent, from the desktop feedback owner. Found while checking a
red kernel job on my own pull request; it is not mine, and it is not a timing
flake either.

## What happens

`job_leash_e2e::a_kernel_asked_to_stop_ends_its_jobs_itself_and_says_so`
(added by `ddd8e4e4`, on `main`) fails on a clean `main` checkout, run alone,
**four times in five**:

```
$ for i in 1 2 3 4 5; do cargo test -p arbos-kernel --test job_leash_e2e \
      a_kernel_asked_to_stop_ends 2>&1 | grep '^test result'; done
FAILED   FAILED   FAILED   ok   FAILED
```

It passes when the whole 12-test file runs (12/12), which is why it reads as
flaky and why `main`'s own CI is green.

The panic, at `job_leash_e2e.rs:621`:

```
assert!(killed.contains("the kernel was stopped"), "{killed}");
   →  killed by the kernel
```

## The cause is two writers of one file

The job's `killed` marker is written from two places, and whichever arrives
first wins:

| Where | What it writes |
| --- | --- |
| `crates/arbos-kernel/src/serve.rs:845` | `killed: the kernel was stopped and ended its jobs with it` |
| `crates/arbos-engine/src/jobs.rs:401` | `killed by the kernel` |

On a stop, both paths fire: the kernel's own shutdown writes the first, and the
leash's reaper writes the second. The test asserts the first, so it fails
whenever the reaper gets there first.

**This is not only a test problem.** The recorded reason a job ended is
nondeterministic, so a person or an agent reading `killed` cannot tell "the
kernel was stopped deliberately" from "the leash reaped it" — which is exactly
the distinction the file exists to record. Two other tests assert the *other*
wording (`jobs.rs:1064`, `tools/bash.rs:1075`), so the two spellings are both
load-bearing today and cannot simply be unified without deciding which is true.

My reading, for what it is worth: the stop path should win, because it knows
more — it knows the kernel was asked to stop, while the reaper only knows the
process is going. So the reaper should not overwrite an existing marker, or the
stop path should write last and win.

## Why I did not fix it

It is your area, the fix is a decision about which reason is authoritative
rather than a mechanical change, and three tests depend on the current
spellings. Better named than patched by someone passing through.

It will hold unrelated pull requests the way the `403`-in-a-timestamp flake did
(#335) — it held mine at
[#431](https://github.com/unarbos/arbos/pull/431) for a full CI cycle while I
proved it was not mine.
