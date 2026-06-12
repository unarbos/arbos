---
description: Update arbos in place — rebuilds from source or pulls the latest release, hot-swaps at idle
argument-hint: [web]
---

Update the arbos that is running this session. The whole procedure is one command — run it and report its output:

```bash
"${ARBOS_EXE:-arbos}" upgrade
```

(`ARBOS_EXE` is set in your environment by the serving process and points at its own binary; using it works even when `arbos` is not on PATH. Plain `arbos upgrade` is only a fallback for shells outside a server.)

It detects the situation itself: inside an arbos source checkout it rebuilds the checkout (the self-editing path); elsewhere it installs the latest release. Either way it replaces the serving binary's file, and the running server re-execs the new binary at its next idle moment — after this turn ends. No restart command exists or is needed.

Rules:

- **Never** `pkill`/`kill` arbos or restart its launcher — your shell is a child of the server; killing it decapitates this turn. Replacing the file is the entire job.
- If the command fails (compile error, network), report the error and stop. The running server is untouched and keeps serving.

How the swap works, so you describe and verify it correctly:

- The server polls its own executable path (engine.WatchRestart). When the file changes and no turn is in flight, it `exec`s the new binary in place. There are **no sentinel files and no signals** — replacing the file is the whole trigger.
- `exec` keeps the same PID **and the same process start time**. `ps`/`lstart`/`etime` therefore say nothing about whether the swap happened — do not use them as evidence, and do not conclude "still old code" from an old start time.
- The swap cannot land *during* one of your turns (your turn keeps the engine busy). It lands in the gap after your turn ends; the very next message is normally served by the new build.
- To actually verify (e.g. the user asks "are you running the new code?"): compare the inode of the executable image mapped into the server process against the binary on disk. They match only after the re-exec, because upgrade installs via rename and every build gets a fresh inode:

```bash
PID=$(pgrep -x "$(basename "$ARBOS_EXE")" | head -1)
lsof -p "$PID" | awk '$4=="txt" && $9 ~ /arbos/'   # mapped image inode
stat -f 'inode=%i' "$(readlink -f "$ARBOS_EXE")"    # on-disk inode
```

If the user passed `web` or `all` (arguments: "$ARGUMENTS"), or you edited files under `web/src/` this session, also rebuild the frontend and tell the user to refresh their browser:

```bash
(cd "$(git rev-parse --show-toplevel 2>/dev/null || pwd)/web" && npm run build)
```

When done, reply in one short paragraph: which mode ran (source or release), old → new version if shown, and that the swap lands when this turn finishes.
