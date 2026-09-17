---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# Chat doors (Discord / Slack) — PR #153, branch `cursor/chat-doors-b027` (on `main`)

K-04's last item. `.arbos/doors.toml` (`[[door]] kind, token, channels, agent, every, reply, mention_only`). The kernel polls the channel; a person's message becomes root's prompt with `channel = discord:<id>` / `slack:<id>`, `device = <author>`; the turn's last words are posted back.

Not run against real Discord: this VM has no `OP_SERVICE_ACCOUNT_TOKEN`. Two ways to do the live check:
1. Cursor Dashboard → Cloud Agents → Secrets: add `OP_SERVICE_ACCOUNT_TOKEN` (or `ARBOS_DISCORD_TOKEN` with the bot token). Then `token = "op://Arbos/6uppsa55jnmrghyoijerikwdoy/credential"` (inventory row 26) or `token = "env:ARBOS_DISCORD_TOKEN"`.
2. On a machine with `op` logged in.

Scenarios:
- Bot invited to a channel with Read Message History + Send Messages; kernel with the door → type a question in Discord → root's transcript gets a `user` line with `channel = discord:<id>`; the answer appears in the channel within `every` + turn time; `kernel.log` has `door_open`, `door_reply`.
- The bot's own reply must not re-wake root (`author.bot`); a second human message after the reply must.
- Kernel restart: history before the restart is not replayed (first look only sets the cursor).
- `mention_only = true`: plain messages are ignored, `@Arbos …` wakes.
- `check` with a token env var missing → warning naming the door and kind, no value. Bad kind → error.
- Redaction: `secret use X` then ask in the channel to print `$X` → the posted reply carries `[REDACTED:X]`.
- Rate: `every = "1s"` is raised to 3 s; Discord allows ~50 req/s per bot so 3 s per channel is well inside.

E2e against a local stand-in: `crates/arbos-kernel/tests/chat_doors_e2e.rs`.
