---
cursor:
  subagentId: "bc-2a1318aa-e675-52f4-b3ab-94cb9415aa39"
---

# "run it" typed into a yielding `bash` is read as "run it again" (seen driving #362)

From the layout loop, cycle 23, driving the #362 kernel with the desktop's halves of Jacob's stuck screen.

## What happened

Root turn, keyed, `gemini-2.5-flash`. Prompt: run `python3 slow.py` yourself with bash (a script that sleeps 2m30 and prints `done`). While it ran, "run it" was typed three times, three seconds apart.

The yield works as #362 says: each `bash` gave up its wait within ~2 s of a steer landing, the steer was read at that boundary, and the transcript shows one `user` line per steer (`~/journeys/stuck-233203/.arbos/agents/root/transcript.jsonl`, lines 13–22).

But the model read each "run it" as an instruction to run the command **again**: j2 yielded → the turn started j3 (`python3 slow.py`), j3 yielded → j4, j4 yielded → j5 ran to the end. Four copies of `slow.py` ran at once; the panel showed four process rows; the reply at the end was "The `python3 slow.py` commands completed. They did not produce any output." — three "job jN exited" notices under it. On Jacob's screen this would have been four bubble sorts.

The kernel's dedup did not apply — rightly: each steer had been consumed before the next arrived, so none was a *pending* duplicate. (The pending case was driven separately, three "run it" into a streaming reply: one `user` line, one "Already queued" notice, as designed.)

## Ask

The result the yielding `bash` hands the model says *"The user said something while it ran — it follows this result. Answer them, then follow the command with `await`."* The model took "run it" as new work rather than as words about the command already running. Two small things would help, either or both:

1. Name the running command in that sentence, so the words are read against it: *"`python3 slow.py` is still running as job j2. The user said something while it ran — it follows this result. It is most likely about this command: answer them, and follow the command with `await j2` rather than starting it again."*
2. Log a `steer_read_during_job` line with the job id, so a repeat start of the same command within a few seconds of a yield is visible in the kernel log for QA.

Not urgent beside the two halves that shipped; filed so the four-bubble-sorts shape is a known one.
