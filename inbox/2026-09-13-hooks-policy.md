# P-01 hooks with policy — QA note (features agent, 2026-09-13)

Branch `cursor/hooks-policy-b027`, base `rust`.

## What it does

Hooks are user programs the kernel runs around tool calls. Two ways to declare them:

1. Executables in `.arbos/hooks/<event>/` (as before): match every tool.
2. `.arbos/hooks.toml` (new):

```toml
[[hook]]
event = "before-tool"      # before-tool | after-tool | after-turn
match = "bash|write|edit"  # tool-name glob(s), `|`-separated; default "*"
run = "hooks/guard.sh"     # program, relative to the place; or an absolute path
timeout_secs = 8           # default 8
```

Payload on stdin (JSON): `{event, agent, tool, args}` for before-tool; `{event, agent, tool, args, result: {body, error, paths}}` for after-tool (body capped at 8 KB); `{event, agent}` for after-turn. Env: `ARBOS_EVENT`, `ARBOS_TOOL`, `ARBOS_AGENT`, `ARBOS_PLACE`.

Exit codes (before-tool): **0 allow**, **2 block** (stderr becomes the tool error the model reads), **3 ask** (the user gets an allow/deny question; stderr is the question text; deny → tool error), any other non-zero → allow, with a `hook … failed` Notice in the transcript. Timeout → block.

Stdout JSON (exit 0) may carry: `tool` / `args` (rewrite the call), `decision` (`allow|block|ask`, same as the exit codes), `reason`, and `context` (text appended to the tool's result for the model — before-tool: after the result; after-tool: appended to the body).

## Attack ideas

1. Old-style non-zero deny: a `.arbos/hooks/before-tool/x` that exits 1 used to deny; it now allows with a Notice. Breaking change — flag it in the report; the PR body says so.
2. `match = "*"` on after-tool with a slow script: every call pays the hook. Timeout 8 s → after-tool timeouts are ignored (result stands) but logged.
3. Hook rewrites `bash` into `write`: the second allowlist check and the readonly footprint check still run.
4. `decision = "ask"` from stdout together with exit 0: ask wins over exit.
5. Exit 3 from a hook while the kernel has no approver (headless `run` without `answer`): approve resolves false when the channel is dropped → block. Check the CLI path (#15) surfaces the question.
6. `context` of 1 MB: appended raw; evict handles the body later. Note.
7. `run` pointing outside the place (`../../bin/evil`): allowed on purpose (the user wrote the config); note.
8. Two hooks match: run in file order; a block from the first stops the chain; rewrites chain.
9. after-turn hook exit codes are ignored; stdout ignored.
10. hooks.toml with a bad `event`: the whole file is rejected with a Notice naming the line; the old-style dirs still run.

## How to run

```
mkdir -p .arbos/hooks && cat > .arbos/hooks/no-rm.sh <<'EOF'
#!/bin/sh
cmd=$(jq -r .args.command); case "$cmd" in *"rm -rf"*) echo "rm -rf is not allowed here" >&2; exit 2;; *sudo*) echo "sudo: allow?" >&2; exit 3;; esac
echo '{"context":"guard ok"}'
EOF
chmod +x .arbos/hooks/no-rm.sh
printf '[[hook]]\nevent="before-tool"\nmatch="bash"\nrun="hooks/no-rm.sh"\n' > .arbos/hooks.toml
```

Prompt: "run `rm -rf /tmp/x` then `echo hi`". Expect: first call errors with "rm -rf is not allowed here", second runs and its result ends with "guard ok".
