# Arbos

![Arbos](arbos.jpg)

<p align="center">
  An autonomous coding agent that runs in a loop, controlled through Discord.<br>
</p>

---

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/unarbos/arbos/main/run.sh | bash
```

The installer handles everything: cloning the repo, installing dependencies (`uv`, `claude`, `pm2`), creating a Python venv, and prompting for API keys. When it finishes, Arbos is running under `pm2`.

### Manual install

```sh
git clone https://github.com/unarbos/arbos.git && cd arbos
uv venv .venv && source .venv/bin/activate
uv pip install -e .
```

Create a `.env` file:

```
OPENROUTER_API_KEY=<your-key>
BOT_TOKEN=<discord-bot-token>
DISCORD_GUILD=<server-id>
```

Start with pm2:

```sh
pm2 start "source .venv/bin/activate && python arbos.py" --name arbos
```

### Requirements

- Python 3.10+
- [Discord bot token](https://discord.com/developers/applications)
- [OpenRouter API key](https://openrouter.ai)
- [Claude Code CLI](https://www.npmjs.com/package/@anthropic-ai/claude-code) (`npm i -g @anthropic-ai/claude-code`)
- [pm2](https://pm2.keymetrics.io/) for process management

---

## How it works

Arbos is a single Python process (`arbos.py`) managed by `pm2`. It runs two things concurrently:

1. **Discord bot** — listens for commands and chat messages
2. **Channel manager** — spawns and monitors autonomous loops

### The loop

Each Discord channel maps to an independent work unit. When you set a `/goal` and `/start` a channel, Arbos enters an autonomous loop:

```
loop forever:
    prompt = PROMPT.md + pin + scope_context + goal + state + chat_history
    result = claude_code(prompt)       # full CLI agent with bash, read, write, etc.
    post step summary to Discord
    sleep(delay)
end
```

Each step is a fresh `claude` CLI invocation with `--dangerously-skip-permissions`. The agent has no memory between steps — continuity comes entirely from the `state` file it writes to disk. The agent can read its goal, modify its state, run arbitrary commands, and create code/data inside its workspace directory.

### Context structure

```
context/
  channels.json            — metadata for all channels (goals, status, token counts)
  general/chat/            — #general channel chat logs
  <channel-name>/          — one directory per managed channel
    goal                   — the channel's objective (set via /goal)
    pin                    — always-on context from pinned Discord messages
    state                  — the agent's working memory (persists between steps)
    chat/                  — operator ↔ bot chat logs (*.jsonl, rolling)
    runs/                  — per-step artifacts
      <timestamp>/
        rollout.md         — full agent output
        logs.txt           — runtime logs
        output.txt         — raw stream-json from claude CLI
    .env                   — channel-scoped env vars (set via /env)
  files/                   — operator-uploaded files from Discord
```

### Prompt assembly

Every step or chat prompt is assembled from layers:

```
PROMPT.md                  — base system prompt (identity, conventions, rules)
+ Pin                      — pinned messages (always-on context for this channel)
+ Scope context            — sibling channels' goals/pins/state (if in a category)
+ Goal + step metadata     — the channel's objective + workspace paths
+ State                    — the agent's self-written working memory
+ Chat history             — recent operator ↔ bot messages
```

### Scope system

Channels inherit context visibility from Discord's category hierarchy:

- **Channel scope** — sees its own goal, pin, state, and chat
- **Category scope** — also sees sibling channels in the same Discord category (their goals, pins, state)
- **Global scope** — uncategorized channels see everything across all categories

This enables multi-agent coordination: channels in the same category can read each other's state and send messages between siblings using `python arbos.py send-to <channel> "message"`.

### LLM

Arbos uses [OpenRouter](https://openrouter.ai) to route inference to Claude Opus 4.6 via the [Claude Code CLI](https://www.npmjs.com/package/@anthropic-ai/claude-code). Each step and chat invocation is a fresh `claude -p` call pointed at OpenRouter's API.

### Security

- The `.env` file can be encrypted at rest using `python arbos.py encrypt`. It derives a Fernet key from the `BOT_TOKEN` using PBKDF2 and stores the ciphertext in `.env.enc`.
- All outgoing text is run through a redaction layer that strips known secrets (env vars with KEY/TOKEN/SECRET/PASSWORD in the name) and matches common credential patterns (GitHub PATs, API keys, etc.).
- The agent prompt explicitly forbids reading or outputting secrets.

---

## Discord commands

All commands are issued in a Discord channel:

| Command | Description |
|---|---|
| `/goal <text>` | Set the channel's objective |
| `/goal` | Show the current goal |
| `/append <text>` | Append to the current goal |
| `/start` | Start the autonomous loop |
| `/stop` | Stop the loop |
| `/pause` | Pause the loop (alias for stop) |
| `/restart` | Kill current step and restart |
| `/delay <minutes>` | Set delay between steps |
| `/status` | Show channel status, tokens, cost |
| `/env NAME VALUE` | Set a channel-scoped env var |
| `/env -d NAME` | Remove a channel-scoped env var |
| `/help` | Show help |

Plain text messages in a channel are handled as chat — the agent responds directly.

### Agent CLI (from within a step)

The agent can control its own loop:

```sh
python arbos.py ctl delay <seconds>     # set delay between steps
python arbos.py ctl restart             # restart the loop
python arbos.py ctl goal <new goal>     # update goal
python arbos.py ctl pause               # pause loop
python arbos.py done                    # mark goal complete, stop loop
python arbos.py send "message"          # send a message to the operator
python arbos.py sendfile path/to/file   # send a file to Discord
python arbos.py scope                   # show scope tree
python arbos.py siblings               # list sibling channels
python arbos.py send-to <ch> "msg"      # message a sibling channel
python arbos.py resource add <type> <name> --destroy "cmd"  # track infrastructure
```

---

## Usage

Create a Discord channel under any category, then:

```
/goal Build a web scraper that monitors HN for AI papers and posts summaries to Slack
/start
```

Arbos will loop autonomously — building, running, evaluating, and iterating. Each step summary is posted to the channel. Send messages at any time to redirect, ask questions, or give feedback.

---

MIT
