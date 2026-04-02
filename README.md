# Arbos

 
 A Ralph-loop combined with a Discord bot.
 That's all you need to do just about anything.
 

# The Design

Arbos loops a `GOAL.md` through a coding agent, one step at a time.

```
                                     ┌────── [GOAL.md] ────────┐
                                     ▼                         │
                ┌──────────┐     ┌───────┐                     │
                │ Discord  │◄───►│ Agent │─────────────────────┘
                └──────────┘     └───────┘
```

Each step is a fresh `claude` CLI invocation. Output streams back to Discord in real time. Between steps the agent persists its progress in `STATE.md` so it can pick up where it left off.

## Requirements

- [Discord Bot token](https://discord.com/developers/applications)
- [OpenRouter API key](https://openrouter.ai/keys)
- Node.js, [`claude` CLI](https://docs.anthropic.com/en/docs/claude-cli), and [pm2](https://pm2.keymetrics.io/)

## Getting started

```sh
curl -fsSL https://raw.githubusercontent.com/unarbos/arbos/main/run.sh | bash
```

Or manually:

```sh
git clone https://github.com/unarbos/arbos.git && cd arbos
./run.sh <discord_token> <guild_id> <openrouter_key>
```

## Usage

All interaction happens through Discord slash commands.

| Command | Description |
|---------|-------------|
| `/thread <name> <goal>` | Create a RALPH loop thread with a goal |
| `/close` | Stop the loop and delete the thread |
| `/pause` | Pause the loop |
| `/resume` | Resume the loop |
| `/step` | Run a single step |
| `/delay <mins>` | Set minutes between steps |
| `/goal [content]` | View or update the goal |
| `/state` | View the agent's STATE.md |
| `/status` | Show Arbos status and active loops |
| `/pin [content]` | View or append operator pins |
| `/cwd [path]` | View or set the working directory |
| `/env set/remove/list` | Manage encrypted environment variables |

Example `/thread` usage:

```
/thread name:trading goal:Build an adaptive trading system that discovers strategies via evolutionary search. Use walk-forward validation, horizon ensembles, and consensus gating.
```

Then let it iterate.

---

MIT
