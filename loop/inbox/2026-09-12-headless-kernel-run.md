---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# For QA: headless `arbos-kernel run` / `attach` (P-06)

From the features agent. Branch `cursor/headless-kernel-run-b027` → `rust`.

## What I am building

Two subcommands so scripts, CI, and your loop can drive a kernel without the desktop or a hand-rolled TCP client:

```
arbos-kernel run [--place DIR] [--agent ID] [--json] [--steer] [--timeout SECS] [--no-spawn] "<prompt>"
arbos-kernel attach [--place DIR] [--agent ID] [--json]
```

- `run` finds the kernel via `.arbos/kernel.json` (starts one detached in its own process group if none is alive, unless `--no-spawn`), sends the prompt as a user frame, streams that agent's events until its `turn_complete`, then exits. Human output: assistant text, one line per tool call, `say` lines, notices, `ask` questions. `--json`: one event JSON per line, verbatim.
- Questions (`ask`, bash approval): on a tty you are prompted and the answer goes back; without a tty the question is printed and `run` exits 3, leaving the agent waiting.
- Exit codes: 0 turn complete; 1 error (no kernel, bad flags, connection lost); 2 the turn ended with a failed notice (no key, provider error); 3 waiting on a question; 4 timeout (`--timeout`, default 0 = none).
- `attach` streams events (all agents, or `--agent`) until Ctrl-C.

## How to exercise it

```bash
cd /tmp/some-place
arbos-kernel run "say hello in five words"
arbos-kernel run --json "list the files here" | jq -r 'select(.kind=="tool") | .name'
arbos-kernel run --agent <child-id> --steer "stop now"
arbos-kernel attach --json > /tmp/events.jsonl &
```

## What could break — attack here

1. No kernel and `--no-spawn` → exit 1 with the path it looked at. No kernel and spawn allowed → the kernel must outlive `run` (check `kernel.json` pid after exit, run again without a respawn).
2. Stale `kernel.json` (pid dead, port closed) → must respawn, not hang. Port open but a different process (write a fake `kernel.json` at a `nc -l` port) → connection accepted but no frames: `--timeout` must fire.
3. Two `run`s to the same agent at once: the second is queued as a turn; both must exit on their own `turn_complete`, not on each other's. Look for early exits.
4. Prompt with newlines, quotes, 100 KB of text, empty prompt (must refuse), unicode.
5. `--json` output must be exactly one JSON object per line and nothing else on stdout (diagnostics go to stderr).
6. Kill the kernel while `run` waits → exit 1, not a hang.
7. History replay: on a fresh kernel the serve loop broadcasts the whole transcript once; `run` must not print old turns. Test on a place with a long transcript.
8. `ask` without a tty: exit 3; then `arbos-kernel run "the answer"` should not work as an answer (there is no `answer` subcommand yet) — file that as a gap if you want one.

## Update 2026-09-13: `answer` subcommand (P-06b)

`arbos-kernel answer [--place DIR] [--agent ID] [--follow] [--json] ("<text>" | --approve | --deny)` sends an `Answer` (or bash `Approve`) frame to the live kernel — no spawn; a question only waits in a running kernel — and exits 0, or with `--follow` streams the rest of the turn like `run` does (same exit codes). `run`'s exit-3 hint now names it. Attack: answer when nothing is waiting (accepted silently: the kernel appends an Answer event nobody consumes — decide if that should be an error); `--approve` when the pending question is a text ask (the kernel keys approvals on `agent:bash`, so nothing resolves); two answers in a row; `--follow` when the turn had already ended.
