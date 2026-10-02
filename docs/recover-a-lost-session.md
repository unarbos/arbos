# Getting a session back

Written after a morning spent believing two long Codex runs had been destroyed. They
had not been. The reason it took an hour to establish that is that there are three
different things called "a session" on this setup, they live in three different
places, and the failure that hid them looked the same as the failure that would
have lost them.

So: which of the three is it, where does it keep its work, and what brings it back.

## Which one is it

| What you were looking at | Where the work lives | Can a dead process lose it |
| --- | --- | --- |
| The **ChatGPT / Codex app**, dark with orange buttons, composer reading "Work on …", header naming a `computeinstance-…` | OpenAI's cloud | No. Nothing on your machine hosts it |
| The **Codex CLI** (`codex`, `codex exec`) in a terminal or under the voice server | `$CODEX_HOME/sessions`, default `~/.codex/sessions` | No, once a turn has been written |
| An **Arbos chat** — the Arbos desktop app or the Arbos iPhone app, white page, FiraCode | `<project>/.arbos/agents/<id>/transcript.jsonl` | No. Transcripts are on disk and sessions resume by id |

The quickest tell is the colour. Arbos is a white page in FiraCode and has no orange
in it and no button anywhere that says "Retry". If you are looking at orange, you are
looking at OpenAI's app and the work is in OpenAI's cloud.

A `computeinstance-…` name is the second tell. That is an OpenAI cloud container, not
a host you can ssh to and not a name the Arbos hub would hand out.

## "Codex could not open this task. Try again in a moment."

This is OpenAI's app failing to open a task it holds server-side. It is not a
process on your machine and it is not Arbos. Two things follow:

- **The task is where it was.** Opening is a read. A read that fails has not
  changed anything.
- **A live activity still counting — `1h 49m` in the dynamic island — means the run
  was still going** at the moment the screen could not be opened. The app had lost
  the task, not the cloud.

What to do, cheapest first:

1. Open <https://chatgpt.com/codex> in a browser. The task list is server-side, so
   a browser sees what the app cannot. Check the archive as well as the open list.
2. Force-quit the app and reopen it. The failure is in the client's copy.
3. If a task is genuinely gone from the web list, it is OpenAI's to recover and
   support is the only route. Nothing local will help.

## The Codex CLI

Every turn is appended to a rollout file as it happens, so a killed process loses
at most the turn that was in flight.

```sh
# Newest first, with the first user message of each, so you can tell them apart.
ls -t "${CODEX_HOME:-$HOME/.codex}/sessions"/*.jsonl | head -20

# What a given one was about.
head -3 "${CODEX_HOME:-$HOME/.codex}/sessions/<file>.jsonl"

# Pick one up where it stopped.
codex resume                       # choose from a list
codex exec resume <thread_id> "carry on"
```

The voice server holds a thread id for the length of one phone call and then drops
it (`voice-server/voice_server/codex.py`). The rollout file outlives it, so a call
that ended is still recoverable from disk even though the server has forgotten the
id.

## An Arbos chat

The transcript is an append-only log, one event per line, under the project:

```sh
ls ~/path/to/project/.arbos/agents/                        # every agent is a session
wc -l ~/path/to/project/.arbos/agents/root/transcript.jsonl
arbos-kernel attach --place ~/path/to/project --agent root
```

For a project served to the phone through the hub, ask the kernel itself rather
than trusting the hub's roster:

```sh
deploy/mobile/kernel.py '<machine>/<project>' hello        # which build is serving it
deploy/mobile/kernel.py '<machine>/<project>' history 200  # the transcript over the wire
```

If a turn went wrong rather than the process dying, `rewind` walks back through the
per-turn git commits — with the kernel stopped:

```sh
arbos-kernel rewind <place> --list
arbos-kernel rewind <place> --back 1 --files
```

### When the app will not attach to a kernel that is plainly running

This is the Arbos failure that looks most like lost work, and it is not.

A kernel from a different build than the app is a kernel the app will not trust:
`crates/arbos-update/src/kernel.rs::same_commit` decides, and the reason is a real
incident — on 2026-09-17 a survivor 223 commits behind was attached to after an
update and sent frames it had never heard of. The desktop says **"Kernel from
another build"** in the status bar; click it to restart the kernel on the build the
app ships. The phone says the project's kernel is an older build.

Installing an Arbos update stops the local kernels the window started, by SIGTERM,
which is the shutdown they are written for: they end their turns, drop the place
lock, and the new app starts new ones. **A remote place's kernel is not touched** —
it runs from its own host's binary — but the tunnel to it goes down and is redialled.

Either way the transcript is on disk and the session resumes by id. What a restart
costs is the seconds of a turn that was in flight.

## What to collect if none of this finds it

Enough to tell which of the three it was, which is the thing that was missing:

```sh
ps ax | grep -iE 'codex|arbos-kernel' | grep -v grep
ls -lt "${CODEX_HOME:-$HOME/.codex}/sessions" 2>/dev/null | head
ls -d ~/*/.arbos/agents/* 2>/dev/null | head
```
