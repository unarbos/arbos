# Worker lines lead with the plan, and are named two ways

Filed at cycle 200, from looking hard at a chat after four workers finished.

## What is on the phone

Seven worker lines were on screen. Counted from the accessibility tree, not
from the picture:

| | count |
| --- | --- |
| worker lines visible | 7 |
| "Last words" beginning with a plan (`Steps:`, `1.`, `I will`) | **6** |
| "Last words" beginning with the answer | 1 |
| named with a space — `w003613 rivers` | 5 |
| named with a hyphen — `w003613-sky` | **2** |

So the useful part of a finished worker's line — the sentence it was asked
for — is usually the part that is cut off. The one line that shows an answer
reads well:

> w003613-sky · Turn ended. Last words: The sky stretched wide and pale blue
> overhead, streaked with thin clouds drifting slowly toward the horizon.

The other six read like this:

> w004050 rivers · Turn ended. Last words: Steps: 1) Read project context and
> notes — done, no rivers-specific info found. 2) Compose one sentence about
> rivers. No files created per rules.…

A first reading of the screenshot suggested the four lines were identical.
They are not — the texts differ. What they share is that they open with
procedure, and the phone shows the opening.

## Where it comes from

Not the app. The phone renders what the kernel sends: `voice-server` parses
the same shape, `^Turn (ended badly|ended)\.\s*Last words:\s*`, and the mock
kernel builds it as `Turn {status}. Last words: {child.last_words}`. The
naming is the kernel's too — the agent directory is `.arbos/agents/w002958-
rivers/`, hyphenated, while the goal as typed was `w002958 rivers`, so the
two forms are the agent id and the goal text arriving in the same list.

## The two questions

**Last words.** The label promises the end of what the worker said and
delivers the beginning of its final message. When a worker opens that message
with its plan — six times in seven here — the line says nothing a reader
wanted. Showing the tail instead would match the label, but which end to show
is a design call, and it may be better fixed where the note is written than
where it is drawn.

**One list, two names.** Five lines name the goal and two name the agent id.
Whichever is right, a single list should not use both.

Neither is changed here. Both originate outside the iPhone app, and this loop
does not ship product guesses.
