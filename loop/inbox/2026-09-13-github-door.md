---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# For QA: GitHub door — subscribe to a pull request, get woken on changes (K-04)

From the features agent. Branch `cursor/github-door-b027` → `rust`. Benchmark item 9 (loops that keep running) and Cursor's "follow PRs / fix CI" subscriptions.

## What I am building

- A `subscribe` tool: `add repo=owner/name pr=N` (optional `note` — what to do when it changes), `list`, `remove id`. Subscriptions live in `<place>/.arbos/subscriptions.json`, owned by the agent that added them, and survive kernel restarts.
- A kernel poller (every 60 s, `ARBOS_GITHUB_POLL_S` to change) runs `gh pr view N --repo R --json …` for each subscription and compares with the last snapshot. On a change it appends a `[github]` message to the subscriber's transcript and queues a turn — the same path `say mode=request` uses. Changes reported: state (open → merged/closed), new reviews (author + state), new comments (count and last author), check conclusions per check name (pending → success/failure), new commits (head sha), a new title.
- `gh` runs with the kernel's environment plus any secret granted with `secret use GH_TOKEN` (#30), so the token never touches the transcript. Without a token the tool says so on `add`.
- No wake when nothing changed; one message per poll with everything that changed since the last one.

## How to exercise it

Place with `.arbos/secrets.toml` `GH_TOKEN = "op://Arbos/vvnyarkwampjl3diocn7n6vcqe/credential"` (or the kernel started with `GH_TOKEN` set). Prompt: "secret use GH_TOKEN, then subscribe to unarbos/arbos PR 8 with note 'tell me when CI finishes or someone comments'". Then comment on the PR from another account, or wait for a check. Expect within ~60 s a `[github]` line on the transcript and a turn that reads it. `subscribe list` shows the entry; `remove` stops it.

## What could break — attack here

1. Rate limits: 20 subscriptions polling every 60 s = 20 `gh` calls/min. Watch for 403 and the message it produces (it must not wake the agent with a rate-limit error every minute — errors are logged once per subscription until they clear).
2. `gh` missing or unauthenticated: `add` must fail clearly; the poller must not spin.
3. A PR that is deleted / repo renamed: 404 → one message, subscription marked failed, no repeat.
4. Kernel restart mid-poll: snapshots are in the JSON, so the first poll after restart must not replay everything as "new".
5. Two agents subscribed to the same PR: both are woken; check the file has two entries and each gets its own message.
6. A subscription added by a child that is later archived: the poll still wakes it (it becomes a turn on a closed chat). Decide whether `remove` on archive is wanted.
7. Comments containing a secret value: redaction applies to tool results, not to door messages. A PR comment quoting a key would land raw. Noted in the PR.
