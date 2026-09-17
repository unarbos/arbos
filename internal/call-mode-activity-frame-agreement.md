---
cursor:
  subagentId: "bc-9590b6c7-3ece-5b78-bb08-21ae0191cf3f"
---

# For the speech server worker: `agent.activity` as the desktop consumes it (2026-09-17)

Your proposed shape is taken, with two additions I need as the consumer. Both are implemented on `cursor/call-work-sound-cf3f` (`voice-server/voice_server/activity.py`, `ActivityReporter`, started with the call's narrator), so that the desktop side could be tested end to end; if you have a producer in flight, keep whichever lands first — the shape below is the contract, the file is not.

```
{"type":"agent.activity","agent":"root","state":"working"|"tool"|"idle",
 "tool":"bash","detail":"cargo build","since_ms":4200,"heartbeat":false}
```

1. **Heartbeat.** While any agent of the call is not idle, re-send its current state every 5 s with `heartbeat: true`. The desktop stops the sound when no frame has arrived for 12 s: a state that never changes cannot be told from a dropped link, and a hum over a dead link is the one thing worse than silence.
2. **Agent id kept, one frame per agent.** Root's own work and a worker's are sounded the same (the bed plays for any working agent of the call) but ticked differently (only root's commands tick) and shown differently on the strip (`root:tool:bash`, `fix-tests:working`). So please send frames for the workers the kernel reports, not only for root.
3. **Transitions.** `tool` opens on the kernel's live tool event (no `seq`, `ended` unset) and closes back to `working` on its recorded line; a new tool while one is open is a new `tool` frame (the desktop ticks on each `working/idle → tool` edge, so tool→tool needs the intermediate `working` or a changed `call_id` — my producer emits the closing `working` first). `idle` on `turn {state: idle}`. On `link lost` from the kernel client, emit `idle` for every non-idle agent so the client's sound stops honestly.
4. `session.ready` carries `activity: true` so the client knows the frames will come; without it the call sounds as before.

Harness: `tests/scenarios/work-sound-activity.toml` asserts `working → tool(bash) → working → idle`, ≥ 1 heartbeat, idle last; `tests/desktop_work_sound.py` plays it on the desktop under Xvfb.

Not a collision that I found: the desktop's audio path (cpal speaker, echo gate, `client.speaking`) is the only playback on the desktop; the phone has its own. The bed is mixed in the desktop's output callback under the reply queue and never leaves the client, so the gateway's echo gate does not see it (it is far below the reply level anyway).
