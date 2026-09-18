# qal-j35 — a relaunch comes back on the main chat, not the sub-chat the person left open

- **status**: **still open** — [#675](https://github.com/unarbos/arbos/pull/675) (`96048de0`, on `main` 17:01) does not close it. `mt-24` breaks 4/4 on the merge. Cause found and named below.
- **found**: 2026-09-18 12:13, triaging cycle 8's desktop-step breaks
- **kernel**: `arbos-kernel 0.2.0 d2a807e48423 protocol 1`; app at `2301abd291c0`
- **control**: `mt-24-relaunch-restores-active-tab`
- **rollout**: `20260918T121304Z-mt-24-relaunch-restores-active-tab`

## What happens

Open two sub-chats, leave the second active, quit the window, relaunch it. The window comes back
with the **project's main chat** active, not the sub-chat that was open.

```
agent_before:      chat-1789733586900     (the sub-chat the person left active)
agent_after:       root                   (the project's main chat)
restored_after_s:  None                   (never matched, across 20 s of polling)
```

Present in every cycle from 2026-09-17 18:18 to now — five in a row, same outcome each time.

## What made this hard to see, and why the old red was not evidence

The scenario had been asserting on the **session id**:

```python
active_before = d.app.state().get("active_session")   # 3
... quit, relaunch, sleep 3 ...
active_after = d2.app.state().get("active_session")   # 2
expect(str(active_after) == str(active_before))
```

Two faults in that, and both had to be cleared before the red meant anything.

**A relaunch renumbers sessions.** Recorded from the window's own list:

| before quit | after relaunch |
|---|---|
| id 1 → `root` | id 0 → `root` |
| id 0 → `root` | id 1 → **`chat-…679`** |
| id 2 → `chat-…087` | id 2 → `root` |
| id 3 → **`chat-…679`** (active) | id 3 → `chat-…087` |

The chat that was id 3 comes back as id 1. So `active_after != active_before` was true whatever
the app did, and the assertion would have gone red on a **correct** restore too. It sat beside a
real fault and named the wrong thing — the `qal-j27` lesson asked of identity rather than of whose
inbox to read.

**And it slept past the question.** `time.sleep(3)` then one read asserts the window finishes
restoring within three seconds, which nothing promises (review rule 7). Replaced with a poll to a
20-second bound that records how long the restore took, so a genuine slowdown stays visible.

With both cleared the finding survives: compared by `agent_session`, which is the chat's own
identity and does survive the relaunch, the active chat is `root` where it was
`chat-1789733586900`, and 20 seconds of polling never changes it.

## Suspected location

The active session looks to be restored by **position** while the positions are rebuilt in another
order — the renumbering table above shows the list is not reconstructed in its previous order, so a
stored index points at a different chat. Storing the agent id instead of the index would fix both
the restore and the renumbering surprise.

## Why it matters

A person with a project open and several sub-chats leaves the one they were working in. Every
relaunch puts them back in the main chat, and on a busy project the sub-chat they want is not
obvious from the tab strip. Nothing is lost, so this is irritation rather than damage — but it is
irritation on every single launch.

## Cross-reference

Found in the same pass as `qal-j33` and `qal-j34`, both rig faults in cycle 8's break list. This
one is the product's, and it is worth noting that it was the *third* of three reds examined that
afternoon: two dissolved on inspection and one survived. The two that dissolved are why this one is
worth believing.

## QA re-check on #675 (`96048de0`) — still broken, and why

Asked to re-check once the fix landed. It does not pass. `mt-24` breaks **4/4** on an app built
from the merge commit `96048de0`, against `arbos-kernel 0.2.0 fba8688d92d2`:

```
mt-24-active-tab-not-restored: the chat active before quit was 'chat-1789751196365'
and after relaunch it is 'root'
```

The build genuinely contains the change (`remember_session` at `workspace.rs:1127/1157`, the
`left_here` guard at `:1649`), so this is not a stale binary.

### The write half works. The restore half overwrites it first.

With `remember()` and `remember_session()` instrumented, the **first** window does the right thing.
The mint-time call no-ops as expected — a brand-new chat has neither `file` nor `agent_session` yet
— and a later call catches it:

```
J35: remember_session(1, 3) file=None agent=None                     <- the new line in #675, no-op
J35: remember_session(1, 3) file=Some(…/sessions/1789751802835.json) agent=Some("chat-…835")
J35: remember(1, Session, …/sessions/1789751802835.json)             <- correct: the sub-chat in front
```

So at quit, `[last."<place>"]` names the sub-chat the person left open. That part is right.

Then the **relaunch window**, before the restore can use it:

```
J35: remember(1, Session, …/sessions/1789751802125.json)   <- the MAIN chat
J35: remember(1, Session, …/sessions/1789751802125.json)   <- again
```

`1789751802125.json` is the project's main chat. The new window focuses it during startup, that
focus calls `remember_session`, and the remembered entry is **overwritten with the main chat before
anything reads it**. The `left_here` guard then compares the active chat against an entry that now
names the main chat, agrees, and leaves the window on `root`.

Confirmed on disk: in a kept scratch, `state.toml` after the whole run reads

```toml
[last."…/place"]
kind = "session"
id = "…/place/.arbos/desktop/sessions/1789751323235.json"   # the main chat, not the sub-chat
```

### The shape of it

`#675` added `self.remember_session(ix, id)` to the chat-creation paths, including
`new_session_in` (`workspace.rs:1127`) — and the restore path calls `new_session_in` for the main
chat on every launch. So the fix's own new line is what erases the memory it depends on. The
restore writes before it reads.

What would need to change is the ordering, not the lookup: read `last_for(place)` and settle the
front chat **before** any startup focus is allowed to call `remember`, or make startup focus not
remember at all (only a person's choice should). The features agent's note says their branch passed
this shape; the difference is worth a look, since the merge does not.

Instrumented worktree and build, if useful: `~/arbos-qa/repo-probe-j43` (detached, `J35:` log points
in `workspace.rs`) with `~/arbos-qa/target-probe-j43-desktop`. Incremental rebuilds there are ~20 s.
