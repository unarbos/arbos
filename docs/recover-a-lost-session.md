# Getting a session back

Written after a morning spent believing two long Codex runs had been destroyed —
and after a first draft of this page got where they live wrong. There are three
different things called "a session" on this setup, they live in three different
places, and the failure that hid them looked the same as the failure that would
have lost them.

So: which of the three is it, where does it keep its work, and what brings it back.

## Which one is it

| What you were looking at | Where the work lives | Can a dead process lose it |
| --- | --- | --- |
| The **ChatGPT app's Codex tab** on the phone, dark with orange buttons, composer reading "Work on …", header naming a host or a `computeinstance-…` | The Mac running the **ChatGPT desktop app** — or an SSH host that Mac is connected to | No. The phone is a remote control; the run and its rollout are on the host |
| The **Codex CLI** (`codex`, `codex exec`) in a terminal or under the voice server | `$CODEX_HOME/sessions`, default `~/.codex/sessions` | No, once a turn has been written |
| An **Arbos chat** — the Arbos desktop app or the Arbos iPhone app, white page, FiraCode | `<project>/.arbos/agents/<id>/transcript.jsonl` | No. Transcripts are on disk and sessions resume by id |

The quickest tell is the colour. Arbos is a white page in FiraCode and has no orange
in it and no button anywhere that says "Retry". If you are looking at orange, you are
looking at OpenAI's app.

A `computeinstance-…` name is the second tell. It is a `Host` alias from
`~/.ssh/config` on the Mac that hosts the connection: the ChatGPT desktop app lists
concrete aliases from that file as SSH hosts, starts `codex` on them over ssh, and the
phone reaches them through the Mac. It is not a name the Arbos hub would hand out.

## "Codex could not open this task. Try again in a moment."

How the phone's Codex tab works (OpenAI's own description, under *Remote
connections* in the Codex docs): the phone sends prompts and approvals through a
relay to a **host** — a Mac or PC with the ChatGPT desktop app running, awake,
online, signed in to the same account and workspace, with **Remote Control**
turned on and that phone paired to it by QR code. The host runs the thread.
Files, shell, credentials and MCP servers are the host's. If the thread's project
is on an SSH host, the desktop app runs `codex` there over ssh and the phone still
talks to the Mac.

So "could not open this task" is the phone failing to reach, or failing to attach to,
a thread on the host. Nothing about it is Arbos, and nothing about it is the cloud.
Two things follow:

- **The task is where it was.** Opening is a read. A read that fails has not
  changed anything on the host.
- **A live activity still counting — `1h 49m` in the dynamic island — means the run
  was still going** at the moment the screen could not be opened. The phone had
  lost its view of the thread, not the thread.

Known ways to get exactly this screen:

- The desktop app on the host is not running, the Mac is asleep, or it is offline.
  The phone lists threads it has seen before, but cannot open any of them.
- Remote Control is off on the host. **Signing out of ChatGPT on the Mac turns
  Remote Control off** and it stays off after signing back in; the pairings survive,
  the switch does not.
- The thread is actively running *and owned by the desktop window*. The phone can
  list it but not attach to it ("thread already has an active writer",
  openai/codex#40558). It opens once the turn ends.
- A second Codex remote-control server is up on the same Mac — the Codex CLI's
  remote control alongside the desktop app's — and one of them gets HTTP 409
  "Remote app server already online" (openai/codex#39547).
- For a `computeinstance-…` thread: the SSH host behind the name has gone away or
  no longer answers `ssh <alias>` from the Mac.

What to do, on the host Mac, cheapest first:

1. Check the ChatGPT desktop app is open and signed in. If it was quit, open it.
2. **Settings → Connections → Control this Mac or PC.** Remote Control must be on
   and the phone listed. If the phone is missing, **Add**, scan the QR code with
   the phone, confirm the same account and workspace. If **Add** errors, restart
   the desktop app and try again. Turn on **keep this computer awake** while you
   are there; a Mac that sleeps drops every remote session.
3. `ps ax | grep -i codex` — a `codex app-server` under the ChatGPT app is the
   thread still running. Leave it alone. Quit any *other* `codex` serving remote
   control from a terminal; two cannot share the Mac.
4. For a thread on an SSH host: `ssh <alias>` from the Mac. Then
   **Settings → Connections**, confirm the host is listed and enabled.
5. The thread's rollout is on whichever machine ran it — the Mac's
   `~/.codex/sessions` for a local project, the SSH host's for a remote one. Even
   when the phone cannot open it, `codex resume` on that machine lists it (see the
   next section).
6. If none of that brings it back, the connection predates June 8 2026 and both
   apps need updating and the phone pairing again — OpenAI's words.

Neither Arbos nor its deploy scripts touch any of this. The voice server runs
`codex exec` as a child of its own and reads `codex login status`; it does not log
in, log out, change `~/.codex/config.toml` or `~/.ssh/config`, or signal a `codex`
it did not start. The Arbos desktop updater SIGTERMs the *Arbos kernels* the window
started — nothing else.

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
