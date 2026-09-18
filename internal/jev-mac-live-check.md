# Jev live hop — build 2060

Checked after 2060 was the running app. No secrets printed.

## Verdict

**Jev works.** Decisions door **200**. File-list turn ran `ls` (`jev-1`), not the fail notice.

## Running app / kernel

- App: `/Applications/Arbos.app` `CFBundleVersion` **2060**, `0.2.0`
- Bundled kernel: `arbos-kernel 0.2.0 fc09dec49aee protocol 1` (#667)
- Project serve: `arbos-kernel serve` the Arbos project folder, pid 69236, `git_sha` `fc09dec49aee`, attach `tcp://127.0.0.1:59865`
- `kernel.out.log`: serve line only. No `jev fell through`.

## Key (present / missing only)

- `api_key` in `~/.config/arbos/config.toml`: **present**
- `OPENROUTER_API_KEY` env: **present** (unused for this probe; kernel uses config `api_key`)
- `~/.config/arbos/secrets.toml`: **missing**

## Decisions door

`POST https://openrouter.ai/api/alpha/decisions` with `model` + `state` + typed `questions` (no `messages`).

| slug | http | pick |
| --- | --- | --- |
| `~typesafe/jev-latest` | **200** (0.9s) | `act=tool`, `tool=ls` (model `typesafe/jev-1.13-20260917`) |

## File-list turn

Prompt: `what files are in this folder?` via `arbos-kernel run --no-spawn` against the live 2060 kernel.

- First tool: `name=ls` `call_id=jev-1` body `.arbos/ comms/ telegram/`
- Fail notice (`Jev did not choose the next step…`): **none**
- After `ls`, Jev kept picking `ls` again (100+ `jev-*` calls). The `run` CLI hit its 90s cap. A stop frame ended the turn. That loop is after the hop already succeeded.

## Not done

- Did not start leftover Jev A–G.
- Did not publish `v0.2.0`.
