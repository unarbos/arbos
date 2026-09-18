# Jev live hop — build 2060

No secrets printed.

## Verdict

**Jev works.** Decisions door **200**. File-list turn ran `ls` (`jev-1`), not the fail notice.

## Running app / kernel

- App `CFBundleVersion` **2060**, `0.2.0`
- Kernel `arbos-kernel 0.2.0 fc09dec49aee protocol 1` (#667)
- Project serve `git_sha` `fc09dec49aee`
- No `jev fell through` in `kernel.out.log`

## Key (present / missing only)

- `api_key` in config.toml: **present**
- `OPENROUTER_API_KEY` env: **present**
- `secrets.toml`: **missing**

## Decisions door

`POST https://openrouter.ai/api/alpha/decisions` with `model` + `state` + typed `questions` (no `messages`).

| slug | http | pick |
| --- | --- | --- |
| `~typesafe/jev-latest` | **200** | `act=tool`, `tool=ls` |

## File-list turn

Prompt: `what files are in this folder?`

- First tool: `name=ls` `call_id=jev-1`
- Fail notice: **none**
