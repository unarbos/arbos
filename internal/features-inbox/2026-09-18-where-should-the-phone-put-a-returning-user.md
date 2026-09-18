# Where should the phone put a returning user?

Measured at cycle 75 of the iPhone loop, on `main` at `d61ec9e0`. Scenario:
`deploy/mobile/scenarios/what-a-returning-user-sees.sh`. Stills:
`media/mobile/cycle-75/`.

## What the phone does today

| he leaves | what happens | he comes back to |
| --- | --- | --- |
| in a project chat | Home, 120 s, reopen — iOS kept the process | **the same chat**, last three lines identical |
| in a project chat | the process is gone, as it is after a night | **the projects list** |

## Why this is a question and not a bug report

The app knows which project he was in: `settings.kernelTarget` persists, and
is what the list's composer uses to name a project. So landing on the list
after a cold launch is a choice the code makes, not a limit it is under.

Two defensible answers, and the loop should not pick one alone:

* **Back into the chat.** He uses one project at a time, and a phone picked
  up after a night is usually picked up to continue. The desktop persists
  and restores workspace state (`restore_panel`,
  `desktop/src/model/workspace.rs`), which leans this way.
* **The list.** A night is long enough that the roster is the more useful
  first screen, and a chat restored onto a stale link has its own problems —
  though cycle 70's work means the link now explains itself when it is down.

## What would settle it

What the Mac desktop does with the *active project* on a cold start, which I
did not establish — only that it restores panels and tabs within a project.
If the desktop reopens where you were, the phone should match it, and that
is a small change: the chat is already reachable from a persisted target.
