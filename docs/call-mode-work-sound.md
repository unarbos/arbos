# The sound of work on a call

Design note, 2026-09-17. What the desktop plays while the agent works during a call, and why. Branch `cursor/call-work-sound-cf3f` ([#428](https://github.com/unarbos/arbos/pull/428) against `main`).

## Listen first

Two files, 12 s each, the same scene twice: a spoken reply ("On it. I sent the agent run-tests to run the whole suite."), then six seconds of the agent working with two commands starting, then the reply that ends the turn ("All thirty tests pass."). Speech is at the level the desktop plays it (peak −7 dBFS); the work sound is at its true level under it, nothing boosted — so you hear the relation, not the sound alone. Set the volume so the voice is comfortable, then listen to what sits between the two sentences.

- `media/call-mode/work-sound-bed-sample.wav` — 585,452 bytes, sha256 `dbddeb26…`; also in the repo at `media/call-mode/` on the PR branch.
- `media/call-mode/work-sound-ticks-sample.wav` — 585,452 bytes, sha256 `6405fc22…`; same.

**What to listen for.** The bed is meant to be the texture of an open line: someone is at their desk on the other end, the line is live, nothing needs you. The question is whether it reads as *presence* — you would notice if it stopped — or as *noise* you would want to turn off after a minute. The ticks are the other answer: nothing at all, then a quiet tick every 2.5 s and one when a command starts; the question there is whether the silence between ticks feels like waiting or like a dead line. Both are rendered from the same numbers the desktop uses (`tests/work_sound_sample.py`).

## Why

Jacob asked the call "what are you working on"; it said "let me get that information" and then nothing. Silence made a working agent and a broken one sound the same. (The silence itself was a routing bug on the speech server, fixed there; the missing sound is ours.)

## What it is

**Honest.** The sound follows one thing: `agent.activity` frames from the speech gateway, which the gateway derives from the kernel's own `turn` and live tool frames. It starts when a turn or a tool starts, stops when the turn ends. No timer, no filler: if the speech model says "let me get that" but the kernel is not running, there is no sound. If the frames stop coming for 12 s while the state said working, the sound stops too — a dropped link is unknown, not "working". Without a gateway that sends the frames (`session.ready.activity` absent) the call sounds exactly as before.

**Tolerable for minutes.** The default is a *bed*: soft, low, breathing noise at about −38 dBFS (mean −40 dB in the sample), band-limited to 150 Hz–1.4 kHz so it has no hiss, with a slow ±3 dB swell every 4 s. The texture of an open line to someone at their desk, not a progress beep. Each time the main agent starts a command (the `tool` state) there is one short tick — a 45 ms damped 880 Hz sine at about −26 dBFS — so a burst of commands is audible as a burst, while one long command is a steady bed.

**Yields to voices.** The bed fades to nothing in 40 ms the instant a reply starts playing or the caller's mic goes loud, and comes back over 400 ms after. It is mixed under the reply in the same output stream (cpal, the device's own callback), so ducking is sample-accurate and cannot fight the barge-in path: barge-in clears the reply queue; the bed's own gain is a separate ramp.

**Workers.** The frame names the agent. The bed plays for any agent of the call (the main agent or one of its workers) that is working — Jacob's "the chat runs commands" is the whole project's work. The tick is only for the main agent's own commands; a worker's tools do not tick (they would be constant). The strip shows who: `root:tool:bash`, `fix-tests:working`.

## The frame (agreed with the speech server, 2026-09-17)

```
{"type":"agent.activity","agent":"root","state":"working"|"tool"|"idle",
 "tool":"bash","detail":"cargo build","since_ms":4200,"heartbeat":false}
```

- One frame per transition, per agent of the call. `working` = the turn runs, no tool in flight; `tool` = a live tool event (no `seq`, `ended` unset) opened it, its recorded line (with `seq`) closes it back to `working`; `idle` = the turn ended.
- Two additions to the proposal, both taken: **a heartbeat** — the current non-idle state re-sent every 5 s with `heartbeat: true`, so a long quiet turn and a dropped link can be told apart (the consumer stops the sound at 12 s without one); and **the agent id kept**, so root's own work and a worker's can be shown and sounded differently.
- `session.ready` says `activity: true` when a gateway sends these.

## Two options for Jacob (one question)

Both are built and switchable in `~/.config/arbos/config.toml` (`voice_work_sound = "bed" | "ticks" | "off"`); the samples are above.

1. **Bed** (default now): the soft continuous noise, plus a tick per command. Continuous presence; you never wonder whether it is still going.
2. **Ticks only**: silence, broken by a quiet tick every 2.5 s while work runs and a tick per command. Nothing to get used to; you count the ticks.

Question for him: *bed or ticks?*

## Verified

- Harness `work-sound-activity`: root's frames are `working → tool(bash) → working → idle`, two heartbeats during a 6.5 s tool, idle last.
- `python -m tests.desktop_work_sound` (Xvfb): nothing plays while idle (0 bytes in 1.5 s), the bed streams in real time while the tool runs and nobody speaks (98 kB in 2 s), the strip knows `root:tool:bash`, nothing plays once idle again. Same for ticks.
- Not yet heard on a Mac: the Mac worker should play a call with a long command and judge the level; the bed's loudness is one constant (`BED_LEVEL` in `desktop/src/voice_ws.rs`).
