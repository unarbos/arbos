---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# `arbos-kernel check` and the fixture runner

Branch `cursor/kernel-check-fixtures-b027` (on the integration head). The lint and the fixture walker from `docs/filesystem-state-design.md` "Testing by authored states".

- `arbos-kernel check <place> [--json] [--quiet]`: parses every agent's `agent.md` (name must match the folder; parent must exist), `plan.jsonl` (each line a node; parents present), `attempts.jsonl`, `transcript.jsonl`, `checkpoints.jsonl`, the place `focus`, `access.toml`, `secrets.toml`, and `kernel.json` (stale pid = warning). Exit 1 on errors. Reads only — `read_focus` is not used because it repairs the file.
- `crates/arbos-kernel/tests/fixtures/<name>/`: `.arbos/…` + optional `now` (RFC 3339 UTC) + optional `replies.jsonl` + `expect.sh`. `cargo test -p arbos-kernel --test fixtures` copies each to a scratch place with its own config home (an unreachable provider, so a fixture that forgot `replies.jsonl` fails fast), lints it, runs `serve --until-idle --horizon 1h [--now …] [--provider replay --replies …]` (90 s limit), then `expect.sh`; on failure it prints the kernel output and the transcripts. Two fixtures ship: `cron-fires-and-reports` (the design's example) and `replay-turn`.

Attack surface: a fixture with a `user`-line transcript but no `replies.jsonl` (fails in 5 s: `stream_idle_ms = 5000`, `max_attempts = 1`); a `now` far in the past with an `every` node (fires once, then the horizon check — does it go idle?); an authored `agent.md` with `cwd:` pointing at another machine's path (the lint does not check cwd; say if it should); `check` on a place while a kernel runs (read-only; fine); the lint's `agents/root` missing `agent.md` → warning, other agents → error; `check --json` shape for QA's own runner.
