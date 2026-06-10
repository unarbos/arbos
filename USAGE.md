# Using arbos

A guided tour of everything you can do with `arbos`, built up from the
simplest one-liner to the headless control protocol. Each level assumes the
ones before it.

`arbos` is a single binary with no subcommands — you drive it with a task,
a handful of flags, and (once it's running) the tools, slash commands, and
config files described below.

---

## Level 0 — Install and point it at a model

```bash
go install github.com/unarbos/arbos/cmd/arbos@latest
# or, from a clone:
make install        # go install ./cmd/arbos

export OPENROUTER_API_KEY=sk-or-...   # https://openrouter.ai/keys
```

With no key set, arbos runs a deterministic **fake** provider (and warns on
stderr) — handy for trying the mechanics without spending tokens.

Other providers (pick one):

```bash
export ARBOS_PROVIDER=anthropic        # openai | anthropic | google
export ARBOS_ANTHROPIC_API_KEY=sk-ant-...
export ARBOS_MODEL=anthropic/claude-opus-4.8
```

---

## Level 1 — Ask one question (one-shot)

The fastest path: a single turn, then exit. Anything after the flags is the
task, so quoting is optional.

```bash
arbos -q "explain what cmd/arbos/main.go does"
arbos -p "list the files in internal/ and summarize each package"
echo "what does the engine package do?" | arbos      # task from stdin
```

`-q`, `-p`, and `-prompt` are aliases. Piped input (no TTY) always runs a
single turn.

---

## Level 2 — Give it a task to do, once

Same one-shot shape, but now it can touch the repo. `-once` forces a single
turn even from an interactive terminal.

```bash
arbos -once "add a doc comment to every exported function in internal/core"
arbos -once fix the failing test in internal/engine
```

Under the hood it uses the coding tools: `read`, `ls`, `grep`, `find`,
`write`, `edit`, `bash`, `fetch`.

---

## Level 3 — Work interactively

Run it bare (or with a starting task) from a terminal and the session stays
open for follow-ups at the `›` prompt.

```bash
cd your-project
arbos                       # shows a brief, then waits for your first task
arbos refactor the auth module    # starts on this task, keeps the session open
```

At the prompt:
- type the next task and hit enter
- a blank line just re-prompts (a stray enter never ends the session)
- `exit`, `quit`, `q`, or Ctrl-D ends it

A bare interactive start greets you with a **brief** (what changed since you
left, what's waiting on you) and, on the way out, a **handoff** of what's
still open.

---

## Level 4 — Stay in control of mutations

Gate every `write` / `edit` / `bash` behind a `y/N` confirmation:

```bash
arbos -approve "tidy up the imports across the repo"
```

Each mutating call pauses with `approve <tool>? [y/N]` until you answer.

---

## Level 5 — Read, search, and edit (the coding tools)

Once a session is running, the model has these tools. You rarely call them
by name — you describe the goal — but knowing them shapes good prompts:

| Tool | What it does | Notes |
|------|--------------|-------|
| `ls` | list a directory | read-only |
| `read` | read a file (images come back as content blocks) | read-only |
| `find` | locate files | needs `fd` on PATH |
| `grep` | search file contents | needs `rg` (ripgrep) on PATH |
| `write` | create/overwrite a file | mutating |
| `edit` | exact + fuzzy match edits, line-numbered diffs | mutating |
| `bash` | run a shell command | mutating |
| `fetch` | HTTP GET/POST (truncated at 256KB) | read-only |

Install the two external helpers so `find`/`grep` work:

```bash
# example: Debian/Ubuntu
sudo apt install ripgrep fd-find
```

---

## Level 6 — Long-running and background work

`bash` can launch background jobs that survive across turns and even across
restarts (they're journaled under `.arbos/jobs/`). Two more tools manage them:

| Tool | What it does |
|------|--------------|
| `await` | block until a background job finishes |
| `jobs` | list background jobs and their state |

Prompt-side, this looks like:

```bash
arbos "start the dev server in the background, then run the test suite and
report failures once both are done"
```

---

## Level 7 — Reach the web

```bash
arbos -q "fetch https://example.com/api/status and summarize the JSON"
```

`fetch` does GET/POST and truncates large bodies, so point it at specific
endpoints rather than whole pages.

---

## Level 8 — Slash commands and skills

**Slash commands** are reusable prompt templates. Drop a markdown file in
`./.arbos/prompts/` (project) or `~/.config/arbos/prompts/` (user), then call
it with `/name` — arbos expands it before the turn:

```bash
# ./.arbos/prompts/review.md  ->  "Review {{args}} for bugs and style."
arbos "/review internal/engine/relay.go"
```

**Skills** are `SKILL.md` bundles (agentskills.io format) the agent loads on
demand. Put them in `./.arbos/skills/` or `~/.config/arbos/skills/`.

---

## Level 9 — Project context

arbos auto-loads `AGENTS.md` and `CLAUDE.md` from the working tree, so house
rules, architecture notes, and conventions ride along with every turn. Keep
those files current and you stop re-explaining your project each session.

---

## Level 10 — Plans (durable goals)

The `plan` tool maintains a durable goal forest in the session store
(`op:add`, `op:update`, `op:show`). On long-running hosts (an interactive
session or `-serve`), a scheduler fires time-armed plan nodes: deferred shell
commands run as jobs, and `wake` nodes spawn fresh executor sessions.

```bash
arbos "plan a 3-step migration of the sqlite layer, then start step one"
arbos "remind me to re-run the e2e suite in 30 minutes"   # time-armed node
```

Anything those background runs need to tell you lands in the **outbox** and
is delivered to your terminal at quiet moments (never mid-stream).

---

## Level 11 — Long-term memory

Memory (`internal/mind`) is automatic, not a tool you call. At the start of a
turn, relevant past facts are recalled and injected; at the end, a background
model folds what happened into durable atoms in SQLite. It needs a real LLM
key and a persistent `-db`. A cheaper model can do the curating:

```bash
export ARBOS_DISTILL_MODEL=anthropic/claude-haiku   # compaction + memory
```

---

## Level 12 — Delegation and fan-out

The `delegate` tool spawns a sub-agent — optionally scoped to a different
directory — and streams its tool activity live into the parent view.
Read-only delegations run in **parallel**, so you can fan a task out across
many targets.

```bash
arbos "delegate a read-only audit of each package under internal/ in parallel,
then summarize the findings"
```

---

## Level 13 — MCP (external tools)

arbos is an MCP client: point it at MCP servers and their tools show up as
`{server}__{tool}` (e.g. `fake__ping`). Configure via `./.arbos/mcp.json`
(checked first), `~/.config/arbos/mcp.json`, or inline:

```bash
export ARBOS_MCP_CONFIG='{"servers":{"fs":{"command":"npx","args":["..."]}}}'
arbos "use the fs MCP server to list the project root"
```

arbos can also expose its own tools over MCP (server side) — see
`internal/mcp`.

---

## Level 14 — Persist, resume, and fork sessions

Every session is event-sourced into SQLite (`~/.config/arbos/sessions.db` by
default; override with `-db`). Resume one by id in one-shot mode:

```bash
arbos -db live.db -session sess-1700000000-abc123 -q "continue where we left off"
```

Forking a session (branch from a point in its history) is available over the
control seam — see the next level.

---

## Level 15 — Drive it headlessly (the control seam)

For programmatic use, run the JSON-lines control protocol over stdio:

```bash
arbos -serve -db live.db
```

You write one JSON object per line to stdin and read events from stdout.

Client → server frames:

| `type` | Fields | Purpose |
|--------|--------|---------|
| `open` | `session_id?` | open/resume a session (omit id for fresh) |
| `intent` | `intent` | send a `core.Intent` (prompt, interrupt, approval, …) |
| `set_model` | `model` | switch model |
| `compact` | — | compact the context |
| `switch_session` | `session_id` | bind a different session |
| `fork` | `session_id`, `new_session_id`, `through_seq` | branch a session |

Server → client: `opened`, `switched`, `forked`, `event`, `error`.

Minimal exchange:

```json
{"type":"open"}
{"type":"intent","intent":{"kind":"prompt","text":"Reply with exactly: OK"}}
```

Then read `event` frames until a `turn_complete`. If a client disconnects
mid-turn, the server drains the in-flight turn (default 120s, tunable with
`ARBOS_SERVE_DRAIN_TIMEOUT`).

---

## Flag reference

| Flag | Default | Meaning |
|------|---------|---------|
| `-q` / `-p` / `-prompt` | `""` | one-shot task (aliases) |
| `-once` | `false` | single turn, then exit |
| `-approve` | `false` | confirm every `write`/`edit`/`bash` |
| `-session` | `""` | resume a session id (one-shot) |
| `-db` | `~/.config/arbos/sessions.db` | session store path |
| `-serve` | `false` | headless JSON-lines control seam over stdio |
| *(positional)* | — | trailing words are folded into the task |

## Environment reference

| Variable | Purpose |
|----------|---------|
| `OPENROUTER_API_KEY` | OpenRouter onboarding (auto-routes OpenAI adapter) |
| `ARBOS_PROVIDER` | `openai` \| `anthropic` \| `google` (default `openai`) |
| `ARBOS_OPENAI_API_KEY` / `ARBOS_OPENAI_BASE_URL` | OpenAI creds/endpoint |
| `ARBOS_ANTHROPIC_API_KEY` / `ARBOS_ANTHROPIC_BASE_URL` | Anthropic creds/endpoint |
| `ARBOS_GOOGLE_API_KEY` / `ARBOS_GOOGLE_BASE_URL` | Google creds/endpoint |
| `ARBOS_MODEL` | model id (`fake` with no key) |
| `ARBOS_DISTILL_MODEL` | cheaper model for compaction + memory |
| `ARBOS_MCP_CONFIG` | inline MCP server config (JSON) |
| `ARBOS_SERVE_DRAIN_TIMEOUT` | seconds to finish an in-flight turn after disconnect |

## Files and directories

| Path | Purpose |
|------|---------|
| `~/.config/arbos/.env`, `./.env` | env loaded at startup (no override of set vars) |
| `~/.config/arbos/sessions.db` | default session store |
| `~/.config/arbos/arbos.log` | diagnostics in quiet interactive mode |
| `./.arbos/mcp.json`, `~/.config/arbos/mcp.json` | MCP servers |
| `./.arbos/skills/`, `~/.config/arbos/skills/` | skills (`SKILL.md`) |
| `./.arbos/prompts/`, `~/.config/arbos/prompts/` | slash-command templates |
| `.arbos/jobs/` | background job journals |
| `AGENTS.md`, `CLAUDE.md` | auto-loaded project context |
