# Jev live check — moncks-MacBook-Pro-5

Checked 2026-09-18T15:29Z on Jacob’s Mac. No secrets printed.

## Verdict

**400.** Official slug `~typesafe/jev-latest` against OpenRouter `chat/completions` returns 400: Jev is a decisions model, not a chat model. The running kernel still calls chat completions. #664 (fail in the open, no fallback) is not in this 2049 binary.

## Host / app

- hostname: `moncks-MacBook-Pro-5.local`
- app: `/Applications/Arbos.app` `CFBundleVersion` **2049** (Mac feed current), `0.2.0`
- kernel: `arbos-kernel serve /Users/const/.arbos` pid 58726 git `ccf871bdb123` (commit 2026-09-18T14:48:10Z)
- #664 merged 2026-09-18T15:16:26Z — after this git. 2049 still falls through.

## Slug remap

- Official slug: `~typesafe/jev-latest` (tilde required).
- Saved `~/.config/arbos/config.toml`: no `jev` / `jev_model` keys. Default is the official slug. No tilde edit needed.
- provider `openrouter`, chat model `anthropic/claude-opus-5`.
- Running kernel binary contains `~typesafe/jev-latest` and remaps bare `typesafe/jev-latest` → `~typesafe/jev-latest`.
- Live proof the tilde matters: bare `typesafe/jev-latest` → **400** `not a valid model ID`.

## Key (present / missing only)

- `api_key` in `~/.config/arbos/config.toml`: **present** (auth/key **200**)
- `OPENROUTER_API_KEY` env: **present** (auth/key **401** User not found — dead; kernel uses config `api_key`)
- `secrets.toml` at `~/.config/arbos/secrets.toml`: **missing**
- Doppler project `arbos` has an `OPENROUTER_API_KEY` name. Not used for this probe.

## kernel.out.log (stderr)

- Path: `/Users/const/.arbos/.arbos/runtime/kernel.out.log` (7 lines: tgrep index + `serve … 127.0.0.1:62333`)
- `turn root: jev fell through (...)`: **none**
- Current kernel has had **no turns** since start (last root turn 2026-09-18T13:50:27Z, kernel start 15:21:28Z). JSON `kernel.log` also has zero jev events.

## Root transcript

- File: `/Users/const/.arbos/.arbos/agents/root/transcript.jsonl`
- `jev-*` tool call ids: **0**
- Regular tools: 48 (`bash` 12, `write` 8, `spawn` 5, `agents` 4, `browser` 4, `search` 3, `status` 3, `fetch` 3, `screenshot` 2, `read` 2, `transcript` 1, `await` 1) — LLM ids (`call_*` / `toolu_*`)
- Assistant prose turns: 31
- That is the **act=llm** path. 2049 still falls through, so a 400 would look like this.

## Live OpenRouter (config `api_key`, not the dead env key)

| slug | endpoint | http | note |
| --- | --- | --- | --- |
| `~typesafe/jev-latest` | `/api/v1/chat/completions` | **400** | “decisions model … Use the /api/alpha/decisions endpoint instead.” |
| `typesafe/jev-latest` | `/api/v1/chat/completions` | **400** | “not a valid model ID” |
| `~typesafe/jev-latest` | `/api/alpha/decisions` | **400** | schema (probe body was not the decisions contract) |

No timeout. Not missing-key on the config key. Not junk (OpenRouter rejected before a Jev body).

## Not done

- Did not start leftover Jev A–G.
- Did not publish `v0.2.0`.
- Did not change product code (saved slug already remaps / defaults to `~typesafe/jev-latest`).
