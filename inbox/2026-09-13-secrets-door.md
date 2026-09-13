---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# For QA: secrets door — vault keys into bash, never into the transcript (K-08)

From the features agent. Branch `cursor/secrets-door-b027` → `rust`. Benchmark item 10.

## What I am building

- `<place>/.arbos/secrets.toml` names the secrets an agent may use and where each comes from — never the value:
  ```toml
  [secrets]
  GITHUB_TOKEN = "op://Arbos/vvnyarkwampjl3diocn7n6vcqe/credential"
  OPENROUTER_API_KEY = "env:OPENROUTER_API_KEY"
  DEPLOY_KEY = "file:/home/me/.keys/deploy"
  ```
- A `secret` tool: `list` (names and source kinds), `use NAME` (the kernel resolves the source — `op read` for `op://`, the kernel's environment for `env:`, a file for `file:` — and puts the value into the environment of every later `bash` command of the kernel under that name; the value never comes back to the model), `revoke NAME`. A name not in the file but present in the kernel's environment can be `use`d too.
- **Redaction**: every granted value, and the kernel's own model API key, is replaced by `[REDACTED:NAME]` in every tool result before it is written to the transcript — bash output, `await`/`jobs` logs, `read` of a file that contains it, `fetch` echoing it. Values shorter than 8 characters are not tracked (too many false hits).
- The `secret` tool's receipt says exactly this, so the model knows not to echo the value.

## How to exercise it

Place with `.arbos/secrets.toml` naming `OPENROUTER_API_KEY = "env:OPENROUTER_API_KEY"` (the kernel has it). Prompt: "use the secret OPENROUTER_API_KEY, then run bash: echo $OPENROUTER_API_KEY | cut -c1-12, then run: env | grep -c OPENROUTER". Expect the tool result `[REDACTED:OPENROUTER_API_KEY]…`-style text, no key characters anywhere in `transcript.jsonl`. Then `grep -c 'sk-or' .arbos/agents/root/transcript.jsonl` → 0. Also `read ~/.config/arbos/config.toml` when it holds `api_key`: redacted.

## What could break — attack here

1. Partial leaks: the model prints the value with a separator (`echo $K | sed 's/./& /g'`), base64 (`echo $K | base64`), reversed, split across two commands, or as hex. Redaction is exact-substring only; document what gets through.
2. The jobs' `out.log` on disk still holds the raw value (redaction is at the transcript). A `read .arbos/agents/root/jobs/j1/out.log` must come back redacted.
3. `op read` failures: no `OP_SERVICE_ACCOUNT_TOKEN`, bad ref, `op` missing → clear error, nothing granted.
4. `use` of a name not configured and not in the environment → error listing the configured names.
5. Two agents `use` different secrets: the store is kernel-wide, so the child's bash sees the parent's grants too. Decide whether per-agent scoping is needed (noted in the PR).
6. Kernel's own API key in bash `env` output → redacted from the transcript; but a child process could write it to a file and `read` it back → also redacted (same value). Check both.
7. Secrets with regex-special characters, unicode, trailing newlines from `file:`.
