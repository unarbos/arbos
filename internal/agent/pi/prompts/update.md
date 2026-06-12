---
description: Update arbos in place — rebuilds from source or pulls the latest release, hot-swaps at idle
argument-hint: [web]
---

Update the arbos that is running this session. The whole procedure is one command — run it and report its output:

```bash
arbos upgrade
```

It detects the situation itself: inside an arbos source checkout it rebuilds the checkout (the self-editing path); elsewhere it installs the latest release. Either way it replaces the serving binary's file (it reads `ARBOS_EXE` from your environment), and the running server re-execs the new binary at its next idle moment — after this turn ends. No restart command exists or is needed.

Rules:

- **Never** `pkill`/`kill` arbos or restart its launcher — your shell is a child of the server; killing it decapitates this turn. Replacing the file is the entire job.
- If the command fails (compile error, network), report the error and stop. The running server is untouched and keeps serving.

If the user passed `web` or `all` (arguments: "$ARGUMENTS"), or you edited files under `web/src/` this session, also rebuild the frontend and tell the user to refresh their browser:

```bash
(cd "$(git rev-parse --show-toplevel 2>/dev/null || pwd)/web" && npm run build)
```

When done, reply in one short paragraph: which mode ran (source or release), old → new version if shown, and that the swap lands when this turn finishes.
