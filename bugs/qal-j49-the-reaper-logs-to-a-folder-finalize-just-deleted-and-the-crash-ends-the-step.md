# qal-j49 — the reaper logs to a folder `finalize()` just deleted, and the crash ends the whole step

- **status**: fixed in the rig (this loop's defect)
- **found**: 2026-09-20 07:50, asking why the lock family never ran in cycle 21
- **cost**: cycle 21's tracked step died at `rw-03` with 141 scenarios done; everything registered after it never ran — the whole `lk-*` family, `sw-*`, `fm-01`

## What happens

`Recorder` writes to a fast local staging folder and `finalize()` copies the rollout to the store:

```python
def finalize(self):
    self.final_dir.mkdir(parents=True, exist_ok=True)
    copy_tree_contents(self.dir, self.final_dir)
    shutil.rmtree(self.dir, ignore_errors=True)   # staging/ is gone
    return self.final_dir                          # …but self.log_path still points into it
```

`run_one` then keeps writing:

```python
final = rec.finalize()
...
leaked = reap_scratch(scratch)
if leaked:
    rec.log(f"reaped {leaked} process(es) still running under the scratch folder: …")
```

`Recorder.log` does `open(self.log_path, "a")`, and append mode does **not** create a missing
parent, so it raises `FileNotFoundError` — inside the scenario loop, which has no handler:

```
File "run.py", line 2107, in run_one
    rec.log(f"reaped {leaked} process(es) still running under the scratch folder: " + …)
File "run.py", line 164, in log
    with self._lock, open(self.log_path, "a") as f:
FileNotFoundError: [Errno 2] No such file or directory:
  '/home/ubuntu/arbos-qa/staging/20260920T072643Z-rw-03-rewind-with-files-keeps-the-h…'
```

The whole `run.py` dies and every scenario after that one is simply not run.

## Why it hid

Two things kept it quiet:

1. **It only fires when a scenario leaks a process.** `if leaked:` — most do not, so the step
   usually finishes and nobody sees it.
2. **It exits 1, which is what a healthy step with breaks also exits.** `cycle.sh` reads 124 as
   truncation and prints an alarm; 1 reads as "ran, had breaks". So a step that lost a third of its
   library reports the same status as one that completed.

The only thing that caught it was the `!! RUN CRASHED` alarm added on 2026-09-18, which printed the
exception and how many scenarios had finished. Without that line the traceback would have scrolled
past inside a 40,000-line cycle log.

## What it cost, measured

Cycle 21's tracked step: **141 scenarios ran, then it died at `rw-03`.** Cycle 19's equivalent
step, same half (B), ran 169. The 28 that vanished include both of the guards I had been reporting
on every cycle:

| missing | what it guards |
|---|---|
| `lk-01`…`lk-04` | the held-place family; `lk-04` is `qal-j40`'s only standing check |
| `fm-01` | the stale checkpoint sidecar |
| `sw-*` | the notes-page and undo-mark family |

I spent the first part of this investigation believing the half split had hidden `lk-04` — it had
not; the step had died before reaching it.

## The fix

`finalize()` now follows its own files:

```python
self.dir = self.final_dir
self.log_path = self.final_dir / "driver.log"
self.frames_path = self.final_dir / "frames.jsonl"
self.sent_path = self.final_dir / "sent.jsonl"
```

Verified directly — construct a recorder, finalize, then log:

```
post-finalize log: OK (no exception)
driver.log in the rollout holds 2 line(s):
   … before finalize
   … after finalize — this is the line that used to crash the whole step
```

The reaper's line now lands in the rollout where it belongs, instead of at a path that no longer
exists.

## What I would keep from this

**A step that dies is not a step that ran, and exit 1 cannot tell you which.** `cycle.sh` alarms on
124 and says nothing about 1. Any crash inside the scenario loop therefore costs an arbitrary tail
of the library and reports success. The `!! RUN CRASHED` alarm covers the symptom; the cheaper
structural answer would be for `run_one` to catch its own exceptions per scenario, so one bad
scenario costs one scenario.

And the narrower lesson, which caught me twice today: **when something expected is missing, find
where the run stopped before explaining why the thing was excluded.** I had a perfectly good theory
about the half split and it was irrelevant.
