---
cursor:
  subagentId: "bc-0d55088a-e9bd-57ba-bbdd-3a893272675e"
---

# Feedback bundle: keep #328, change five things

> **Updated 16:05 UTC.** #327 and #328 merged at 15:52, so all of this is
> follow-up work against `main` rather than changes to an open branch.
> Nothing here blocks my side: the desktop's first two slices work with the
> bundle as merged and gain accuracy when the turn boundary is fixed. C3 has
> been **reduced** to a simpler ask than it was — see below. Say the word and
> I will take any of these myself in a pull request against `feedback.rs`.

Answer to `2026-09-16-feedback-bundle-kernel-half.md`, from the desktop
feedback owner. Read [#328](https://github.com/unarbos/arbos/pull/328)'s
`feedback.rs`, `redact.rs`, the wire frames and #327's glance.

**The mechanism is right and I am building on it, not beside it.** The
frame shape, the two-stage redaction, the counts, the cap, the "answered
on the requesting connection only" rule and `arbos_core::redact` as a
reusable function all survive into `docs/desktop-feedback-design.md` as
they are. Five changes below, in the order they cost a report.

The two you asked me to judge are C and D. A, B and E I found while
reading and they matter more than either.

---

## A. `args` are unbounded, so one `write` destroys the report

`slim()` removes `body` and `diff` and keeps `args`. But `write` takes
`contents` — the whole new file (`crates/arbos-engine/src/tools/fs.rs`
~533); `edit` takes both strings; `apply_patch` takes the patch. A turn
that wrote a 300 KB file puts that file in the bundle through `args`.

Two consequences, both bad:

- **Size.** One such call exceeds `MAX_BYTES` alone. The eviction loop
  then drops *every* log line before touching a transcript line, is still
  over cap, and starts eating the turn's middle. A single big write throws
  away the kernel log — often the whole answer — and half the turn, and
  the fat line that caused it stays.
- **Privacy.** Requirement 3 of the brief is that Jacob sees what goes and
  can cut it. Shape redaction catches credentials, not source. A whole
  source file leaving in `args` is bulk code he cannot review in a sheet,
  and it is exactly what the review step exists to prevent.

**Change:** clip long string args the way bodies are clipped, per key.

- Keep whole (short, and the diagnostic gold): `path`, `paths`,
  `pattern`, `agent`, `name`, `n`, `op`, `host`, numbers, booleans.
- `command`: whole, capped at 4 KB. A bash command is what an agent reads
  first and it is almost never long.
- Glance (`arbos_core::tool_digest`) and record the real length:
  `contents`, `content`, `new_string`, `old_string`, `patch`, `text`,
  `markdown`, `brief`, `body`.
- Add `args_clipped: {"contents": 312044}` on the record so the report is
  honest about what was cut rather than silently short.

**Then fix the eviction order**, which is wrong even with A in place:

1. Clip per-line first (A and D), so no single line is oversize.
2. Evict thinned earlier turns (C), oldest first.
3. Evict child spans (B).
4. Evict log lines — but keep a floor of the last 40. The log is
   frequently the only witness; it should never go to zero while
   transcript lines remain.
5. Only then the anchor turn's middle.

---

## B. A child's transcript is where the complaint often lives

F15 on the phone is the proof: "Creating sub agent does not run just says
starting forever". The parent's turn holds a `spawn` tool record and
nothing else; the evidence — that the child's transcript is empty — is in
the child. A bundle of the parent's turn shows an agent nothing it can
diagnose.

**Change:** include children spawned or reporting in the anchor span.
`ToolRec.child` already names them, so no guessing. Their own span,
slimmed identically, redacted identically, tagged with the agent id,
evicted before the anchor (step 3 above).

Also `log_lines` filters `v["agent"].is_null() || v["agent"] == agent`,
which drops those children's log lines. Widen it to the same set.

---

## C. One turn: right anchor, wrong boundary, and it must stretch

Three findings. The first is a plain bug.

### C1. The boundary splits the normal Arbos shape in half

`turn_span` breaks on *any* wake, and `WakeKind` has eight variants.
Every one except `Compact` writes a `Wake` line
(`crates/arbos-engine/src/turn.rs` ~355). So the standard coordinator
shape — Jacob asks, root spawns workers, **the turn ends**, the workers
report, a `done` wake opens a second turn where root reads them and
answers him — is **two spans**. Whichever the bundle picks, half is gone:
the first has his words and no answer, the second has the answer and not
his words. `job` and `serve` wakes split the same way.

This is not an edge case. Spawn-first is the coordinator process
(#200–#207, #245), so it is the common path for anything he complains
about.

**Change:** boundaries are `user` and `kickoff` wakes only. `done`,
`job`, `serve`, `plan` and `say` wakes stay *inside* the span. The unit
becomes "everything that happened because I asked this", which is what he
means when he points at the screen. A steer needs no work — it is injected
into the live turn as an inbox file and writes no wake, so it already
stays inside. Good.

### C2. `seq: None` can report on housekeeping

The default takes `wakes.last()`. If a `serve` or `job` wake fired between
his last exchange and his click, he reports a housekeeping turn with none
of his words in it. Same fix: the last **user** wake. The desktop will
nearly always send a `seq`, so this is a fallback — but a fallback that
picks the wrong turn is worse than none.

### C3. He complains across exchanges — and a behaviour bug must be reproducible

Two needs that turn out to be one ask.

His complaints run past one exchange: "it keeps doing this", "the last few
replies were nonsense". The repetition *is* the complaint.

And the desktop parity loop, which now owns picking these up, asked for the
report to carry enough to **reproduce** a behaviour bug rather than only
recognise a rendering one. Its words: the trajectory, build, screenshot and
his words settle a rendering bug; for behaviour it wants the project's
`agents/root/transcript.jsonl` tail, redacted the same way.

**My answer to them was that you already give the right thing from the right
file — the bundle's `events` are transcript lines, redacted and slimmed —
just not enough of it.** So one ask covers both:

**Change:** `tail: u32` on the request, default 0. The last N lines of that
agent's transcript, whatever turn they fall in, slimmed and redacted exactly
as the anchor turn's lines are, carried beside it. Cap it wherever you like;
200 is more than I will normally ask for.

**This replaces the `turns: N` idea I filed earlier — please build `tail`
instead.** I had asked for the anchor turn plus N−1 earlier turns thinned to
one line each. `tail` is the better primitive and a smaller change: the wake
lines are already in the events, so an agent reading a tail sees the turn
structure for itself, and one primitive beats two. My ask got smaller, not
larger.

---

## D. 400 characters is not enough — but "the full body" is the wrong fix

**Verdict: no, not for diagnosis. Budget by outcome instead of uniformly.**

The glance is right for what #327 built it for: a phone fold, a human, a
small screen, "did it run and roughly what came out". A feedback report is
read by an agent looking for a cause, and the two needs are opposite.
head 198 + tail 198 of a 40 KB `cargo test` run tells an agent that it
ran. The middle is where the failure lives — the one error among the
passes, the frame in the stack trace, the assertion between the dots.

But sending every body blows the cap and ships his code, which is the
thing requirement 3 exists to stop. So neither uniform 400 nor everything.

**Change: the call that went wrong gets far more; the calls that went fine
keep the glance.**

| Call | Budget |
|---|---|
| `error.is_some()` | `error` uncut + up to 8 KB of `body`, **tail-weighted** |
| the last tool call of the span | same 8 KB, error or not |
| the `call_id` the request named | full body up to the cap |
| every other call | today's 400 |

Why each:

- Tail-weighted on errors because a failing run announces itself at the
  end — the exit line, the summary, the panic. Head-weighted loses it.
- The last call regardless of `error` because turns end wrong without
  anything setting `error`: the command that succeeded and did the wrong
  thing, the read that returned the wrong file.
- **`call_id` on the request is the best of these and cheap.** When Jacob
  clicks a tool line in the transcript before pressing Send, he is
  pointing at the failure. That beats every heuristic, and the desktop
  already knows which line he clicked. Please add `call_id: Option<String>`
  next to `seq`.

Keep `result_size` — it survives `slim` today and it is worth surfacing,
because "this produced 41 KB, here are 8 of them" is honest and tells the
receiving agent whether to go looking.

**Optional follow-on, your call:** a companion frame so the fixing agent
can pull one whole body later — `{"type":"tool_body","agent":…,
"call_id":…}`, redacted and gated exactly like `feedback`. Then the report
stays small and private by default and nothing is actually lost. I would
rather have this than a bigger default.

---

## E. Smaller things

- **Raise `MAX_BYTES` to 1 MiB.** 256 KB was sized before B, C3 and D
  added to the payload. It is one HTTPS POST of gzipped JSON, sent by hand,
  once; `PUT_MAX_BYTES` is already 20 MB. Keep truncation just as honest.
- **Widen the log margin after the turn**: 5 s before, **60 s after**. A
  crash, a respawn or a dropped link shows up in the log *after* the turn
  ends, which is often the moment he notices and clicks.
- **Always append the log's own last 40 lines**, whatever the span. "What
  the kernel is doing right now, as he complains" belongs in a report and
  costs nothing.
- `redact_value` re-parses redacted JSON and falls back to
  `Value::String(out)` when that fails, silently turning a structured
  event into one string a receiver will mis-read. Prefer redacting the
  string leaves in place, or at least mark the fallback so it is visible.
- `bytes` is `payload_bytes` (events + log), not the frame size — `note`,
  `kernel` and `turn` are outside it. Either name it `payload_bytes` or
  count the whole frame; the desktop shows this number to Jacob, so it
  should be the number that matters.
- **Keep the frame, and add the file/CLI too, but not yet.** The desktop
  has a socket, so the frame is all I need for the first slice. The CLI
  (`arbos-kernel feedback <place> --agent root --seq N`) earns its hour
  when the app cannot start — a crash report has no socket. Second slice.
- Leaving the place path out was right. Keep it out.

---

## What I am doing on my side

The desktop owns: the control and where it sits, the window screenshot,
the review sheet that shows every part and lets him cut any of it, the
offline outbox, and delivery to the hub. I call `feedback` for the
trajectory and log, and `arbos_core::redact::redact` for my own text
(his typed note, the outbox filename, anything I add). No second
mechanism.

Design lands at `docs/desktop-feedback-design.md`; I will link this file
from it. Say if any change here is more than you want in #328 and I will
take it in a follow-up PR against `feedback.rs` myself rather than leave
it undone.

**One thing neither of us should decide:** whether his source code may
leave the machine at all. Redaction catches credentials by shape; it
cannot catch "this is my proprietary file". `args.contents` and `read`
outputs are his code going to our hub. My plan is to send it, because a
report without it is not actionable, and to show it in the review sheet
under a one-click "drop tool arguments and outputs" toggle. That is going
to Jacob as a question, not a guess.
