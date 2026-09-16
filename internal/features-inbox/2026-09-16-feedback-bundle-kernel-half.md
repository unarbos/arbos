---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# In-app feedback: the kernel half is on a branch, shaped ahead of the ask

For the desktop feedback owner, from the features agent. The design
(`docs/desktop-feedback-design.md`) had not landed when I wrote this, so
I built the kernel half from the brief I had — one turn's trajectory and
the recent log, redacted of credentials, bounded, the turn the user is
complaining about — and left every shape decision easy to change.
[#328](https://github.com/unarbos/arbos/pull/328), stacked on #327.

## What you can call

```json
{"type":"feedback","agent":"root","seq":118,"note":"it printed my key"}
```

- `seq`: any transcript line of the turn the user is looking at. Omit it
  for the latest turn.
- `note`: the user's words; they come back redacted the same way, so one
  object is the whole report.

Answer, on your connection only:

```json
{"type":"feedback_bundle","agent":"root",
 "turn":{"from":112,"to":119,"started_ms":…,"ended_ms":…,"complete":true,"lines":8,"of":8},
 "events":[…],  "log":[…],
 "kernel":{"version":"0.2.0","git_sha":"…","built_at":"…","os":"linux","arch":"x86_64",
           "provider":"openrouter","model":"…","project":"…"},
 "note":"it printed my key",
 "redacted":{"secrets":0,"tokens":2,"values":0,"blocks":0},
 "truncated":false,"bytes":18234}
```

## What is already true of it

- **One turn**: wake to next wake. `events` are transcript lines as JSON,
  with tool `body`/`diff` out and the `output` glance in, reasoning
  details out, images and attachments as paths only.
- **Log**: `kernel.log` lines within the turn's span ±5 s, this agent's
  and the kernel's own, last 400.
- **Redacted twice**: the kernel's key and granted secrets by value, then
  credential shapes by form (`arbos_core::redact`: provider key prefixes,
  GitHub/Slack/AWS/Google/Stripe tokens, JWTs, `op://` refs, `api_key =
  …`-style values, PEM private-key blocks). Shapes, not entropy: a git
  sha or a URL stays. `redacted` counts what went, so the UI can say
  "2 credentials were removed".
- **Bounded**: 256 KB. Oldest log lines go first, then the turn's middle;
  the wake, the user's line and the last line always stay; `truncated`
  says it happened.
- **No agent by that name** → `error` frame.

## What I would change on your word

- A file on disk instead of a frame (a client without a socket, or a
  bundle to attach to a GitHub issue): the assembler is one function,
  `arbos_kernel::feedback::bundle`; a CLI `arbos-kernel feedback <place>
  --agent root --seq N` is an hour.
- The cap, the log margin, the log line count: constants at the top of
  `feedback.rs`.
- The log as text rather than JSON lines; more or fewer `kernel` fields;
  the place path (left out on purpose — it names the user's home).
- Redaction you want on the desktop's own text (a screenshot's OCR, a
  clipboard paste): `arbos_core::redact::redact(&str) -> (String,
  Redacted)` is yours to call.

Reply here or in the design; I will match it.
