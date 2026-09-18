---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# One question, two answers — what is left after #562 is a routing choice in the GPT-Live engine, not a segmentation fault

**For:** the voice gateway owner, and Jacob for the choice.
**From:** the features agent (kernel), 2026-09-18 07:15 UTC. Answers the iPhone loop's re-check on `2026-09-18-one-question-two-answers-across-a-pause.md` after #562.

## Where it stands

The loop's re-check says the split is gone — every run returns one `transcript.final` with the whole question — and that one transcript still draws **two spoken answers**:

```
reply: Right now, nothing's in progress; all recent workers have finished.
event response.done reason=completed playing=true
phase listening
reply: We just finished having two workers each run a timed sleep command …
```

I read `voice_server/openai_live.py` for how one question becomes two replies. It is by design, and the design contradicts itself in this one case.

## The mechanism

1. GPT-Live hears the caller directly and speaks at once. The engine sets `HOLD_MODEL_WHILE_DECIDING = False` ("never delay its first word") and `model_voice = "full"`.
2. Its instructions say: *"You are given three stores … which project this is, the chat the caller is looking at, and what the main agent and its sub-agents are doing. Answer those from the stores."* So for *"what are we working on right now?"* it answers from its brief. That is reply 1 — from a brief that is as fresh as the last `Project update:` append, which is why it read "nothing's in progress".
3. Our Whisper transcript lands (~1.2 s after the caller stops, with the join window) and `_route_final` starts `_ensure_delegated`: after 2.5 s, if no `session.delegation.created` was seen and the text is not allowlisted small talk (`routing.is_small_talk`: anything naming work, agents, code, or longer than seven words), the gateway delegates the question to the kernel itself — logged as *"model took a work question itself; delegating ourselves"*. The kernel's answer goes back as `session.commentary.append`, and GPT-Live speaks it. That is reply 2.

So: the instructions tell the model to answer project-state questions from its stores; the backstop says every non-small-talk question must reach the kernel. Both fire; the caller hears both. Before #562 the same happened per half-question, on top of the split, which is why it read as one fault.

## Two ways to one answer, and what each costs

**A. Trust the model's store answer; backstop only silence and false checks.** In `_ensure_delegated`, delegate only when the model *did not answer* (no `session.output_transcript.delta` since the final) or said it would check without delegating. One reply, at the model's speed. Cost: the answer to "what are we working on" is the brief's, which lags the kernel by however often `Project update:` is appended — the loop's run 2 shows the gap (reply 1 wrong-ish, reply 2 right). A staler answer, once.

**B. Hold the model's first word until our transcript decides, then drop its own answer to a work question and delegate at once.** Set `HOLD_MODEL_WHILE_DECIDING = True` for this engine (the mechanism exists — `model_hold`, `_release_model`), and in `_route_final`: small talk → release the hold; anything else → drop the held audio, delegate now (no 2.5 s grace), and let the model speak only the kernel's answer. One reply, the kernel's. Cost: every small-talk answer starts ~1.2 s later (the hold is the Whisper latency, now including the join window), which is exactly the first-word delay the engine was set up to avoid. Without the hold, B produces a cut fragment of reply 1 (its first second) before the kernel's answer — worse than either.

**Not an option:** cancelling the model's in-flight reply from the gateway — the Live API events this engine uses (`session.input_audio.append`, `*.append`, `session.close`) include nothing that stops a response.

## What I did not do

Neither change is mine to make alone: A changes what the caller is told (the brief's word over the kernel's), B changes the engine's first-word latency for every exchange — both are the voice owner's calls and one of them is Jacob's taste. The join (#562) stands on its own and is done.

If B is chosen, the harness can hold it: `tests/mock_openai.py` plays the model; a scenario with a work question can assert `response_dones = 1` (the expectation #562 added) and that the one reply carries the kernel's words; a small-talk scenario asserts the model's own reply still plays. If A is chosen, the same scenario asserts one reply and that the kernel was **not** asked.

The loop's own re-check is the measure either way: `one-breath-one-answer.sh <cycle> <runs>`.
