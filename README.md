# arbos

Terminal coding agent. Reads, writes, edits, and runs commands in your project.

## Install

```bash
go install github.com/unarbos/arbos/cmd/arbos@latest
```

Or one line without cloning:

```bash
curl -fsSL https://raw.githubusercontent.com/unarbos/arbos/main/scripts/install.sh | sh
```

Ensure `$(go env GOPATH)/bin` is on your `PATH`.

## Use

```bash
export OPENROUTER_API_KEY=sk-or-...   # https://openrouter.ai/keys
cd your-project
arbos
```

That's it. `arbos` opens an interactive session in the current directory.

### One-shot

```bash
arbos -q "list files and summarize the README"
```

### Options

| Flag | What it does |
|------|----------------|
| `-q` | One-shot query (non-interactive) |
| `-approve` | Confirm before write/edit/bash |
| `-session ID` | Resume a session |
| `-serve` | Headless JSON control seam |

### Other providers

Set `ARBOS_PROVIDER` to `anthropic` or `google` with the matching `*_API_KEY`. Override the model with `ARBOS_MODEL`.

Without an API key, arbos runs a local fake provider (tools work, replies are synthetic).
