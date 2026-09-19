# The call builds a status line and nothing shows it

Filed at cycle 198, while closing the last coverage row that had nothing
checking it.

## The fact

`CallViewModel.updateNote()` assembles a status string on every route change
and every turn:

```swift
note = parts.isEmpty ? nil : parts.joined(separator: " · ")
```

The parts are the engine name, a route badge, "kernel tools" or "kernel
offline" or "demo chat answers", the reply latency, and the current work
detail. The route badge is built for exactly this purpose:

```swift
/// `speaker · 100%`: where the sound goes and the system volume there.
private var routeBadge: String { ... }
```

`note` is `@Published`. **Nothing reads it.** A search of the whole app for
any use of `.note` outside the view model that defines it returns nothing.

Measured on the running app rather than only in the source
(`where-the-sound-goes.sh`):

```
the connect metric reports: route=speaker
elements naming a route or a volume: 0
files importing CallKit: 0
```

So the app knows where the sound is going, writes that down in a form meant
for display, and never displays it.

## Why it matters

Not for performance — it is a few string joins. It matters because the code
reads as though the call screen shows this. The doc comment describes what a
person would see. Anyone reading `CallViewModel` to find out what the call
screen tells a user would be misled, and cycle 193 had to establish by
measurement that the screen carries no words at all.

There is also the user-facing side. On a phone the audio route genuinely
changes — AirPods connect, a car takes over, the receiver is used instead of
the speaker — and the app is the only thing that knows. Cycle 196 filed a
related note: the screen is already very quiet, with nine seconds of a
motionless orb during `thinking`.

## The decision wanted

Either the call screen should show this line, in which case `note` needs a
view and the wording needs a designer's eye; or it should not, in which case
`note`, `routeBadge` and `updateNote` should go, so the next reader is not
told about a screen that does not exist.

Not decided here: which way it goes is a design call, and removing it is as
much a decision as showing it.

## What cannot be tested on this machine

The coverage row also names AirPods and a dark screen. Neither is reachable
from a simulator: there is no pair to connect and no screen to switch off.
The check says so rather than passing quietly, and CallKit is not imported by
any file in the app, so there is nothing of it to exercise.
