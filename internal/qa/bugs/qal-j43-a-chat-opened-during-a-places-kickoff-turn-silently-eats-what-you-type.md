# qal-j43 — a chat opened during a place's kickoff turn silently ate what you typed

- **status**: **closed — fixed on `main` by `1768ec83`** (in [#656](https://github.com/unarbos/arbos/pull/656)), bisected against an instrumented build. Guarded by `kf-01`.
- **found**: 2026-09-18 15:35, chasing `qal-j42`'s residual
- **broken on**: app `e10953fb` and older (seen back to `0e0edae2`, 13:33)
- **fixed on**: app `1768ec83` (14:15) onward; current `main` `fba8688d92d2` is clean
- **kernel**: `arbos-kernel 0.2.0 cecd48e1bd76` / `fba8688d92d2` — not involved either way
- **kernel half**: [#669](https://github.com/unarbos/arbos/pull/669) (`3b2ce4fb`), `chat_during_kickoff_e2e`, test only
- **split from**: `qal-j42`

## What happened

Open a new place. Within its first ten seconds — while the kickoff turn runs — press ⌘N and type.

The app behaved as if everything worked. The chat was minted, became active, got an id, and reported
its connection **live**. The composer accepted the keystrokes and **cleared on Enter**, which is the
app's own signal that a line was taken.

The kernel never heard it. No inbox file, no transcript, no turn. The line was gone with no notice.

## Where it actually stopped

Not on the wire, and not in the kernel. An instrumented build (log points in `send`, `prompt`,
`mint_chat` and the ACP writer task) shows the whole path on a **failing** run:

```
J43: mint_chat gave chat-1789749251298
J43: drain() agent=Some("chat-1789749251298") queue=0 streaming=false pending_wire=false
J43: wrote 19 bytes: {"type":"surfaces"}
J43: drain() agent=Some("root") ...
```

and then nothing. **`Session::send()` is never called** — not for the new chat, not for root. The
same run on a fixed build has the missing four lines:

```
J43: send() agent=Some("chat-…") live=true kickoff_running=false streaming=false queue=0
J43: send() -> prompt branch
J43: prompt() agent=Some("chat-…") send_frame=Ok("Ok")
J43: wrote 109 bytes: {"type":"user","agent":"chat-…","text":"Reply with the single word KFOK…
```

So the keystrokes reached the composer and the composer cleared, but the **view never dispatched
them to the model layer**. Everything below that — the session, the socket, the kernel — was fine
and idle, waiting for a frame that was never built.

### Both of the kernel's suggested sites are ruled out

The features agent's note (`internal/qa/inbox/2026-09-18-qal-j43-kernel-half-holds.md`) named two
candidates. The instrumented run clears both:

| candidate | what the log says |
|---|---|
| the ACP writer task breaking on a failed TCP write while `send_frame` still returns `Ok` | the writer never breaks; it writes `surfaces` fine and ends only at shutdown. There is also no permanent silent-`Ok` window: when the task breaks it drops `out_rx`, so later `send_frame` calls fail with "attach writer closed". |
| `mint_chat`'s `unwrap_or_else(ROOT_ID)` swallowing a failed `create_chat` | `mint_chat gave chat-1789749251298` — it succeeded. The agent directory exists on disk, and root's inbox is **empty**, so nothing was misfiled to root. |

Their kernel work stands: a chat created mid-kickoff takes its first line correctly. The loss was
entirely above the socket.

## Bisected

`kf-01`, mint forced inside the kickoff window, same kernel and same machine throughout:

| app commit | what it is | result |
|---|---|---|
| `0e0edae2` (13:33) | Merge #647 | **break 3/3** |
| `e10953fb` (13:55) | Merge #649 — the fix's parent | **break 3/3** |
| `1768ec83` (14:15) | *desktop: chat fills the column; Clear goes; expand is pinned* | **pass 3/3** |
| `93e335e2` (14:36) | Merge #656 | pass 3/3 |
| `1beec0a1` (13:56) | Merge #651 — no #656 in it | **break 4/4** |
| `cea8b902` (15:16) | Merge #664 | pass 5/5 |
| `fba8688d` (16:04) | Merge #668 — current `main` | pass 5/5 |

`1768ec83` is the fix. Its parent breaks, it passes, and everything after it passes.

## The part worth someone's attention

**`1768ec83` is a layout commit.** "Chat fills the column; Clear goes; expand is pinned at the
window's top-right" — nothing in it names input dispatch, and the bug predates #649, so it was not
fixing a recent regression either. On the evidence this fix was **incidental**: a silent input-loss
bug was closed by a change about where the chat pane sits.

That is the finding, more than the bug itself. A fix nobody knew they were making is a fix nobody
can keep. The next layout change in that area can take it away again exactly as quietly as this one
restored it, and the symptom — a cleared composer for a line that never arrives — reads as slowness,
not loss, so a person waits rather than retypes.

Hence `kf-01-a-chat-opened-during-kickoff-keeps-what-you-type`, which mints inside the kickoff
window on purpose and holds one line: *a cleared composer means the line was accepted, and an
accepted line must arrive.* It is the only check in the library that can see this.

## What I got wrong on the way, and what it cost

I first reported this as "reproduced 4/4, deterministic, product side unfixed" and changed
`new_chat` to wait for the kickoff turn before minting. Two corrections:

1. **The wait was not what fixed `mt-01`, `mt-04` and `dg-01`.** They pass on current `main` with the
   wait removed — I checked. `1768ec83` fixed them, and my change landed in the same window and took
   the credit. The wait has been removed again: it cost ~10 s on every desktop scenario that mints a
   chat and bought nothing.
2. **"Deterministic" was true but the reason was not what I assumed.** I read pass-on-one-build and
   fail-on-another as a timing race, because I had not noticed the two runs used binaries built from
   different commits. The rule the loop already has — name the build in every finding — is what
   eventually caught it; I had named them, and re-reading my own notes is what turned "heisenbug"
   into a bisect.

The instrumented worktree is at `~/arbos-qa/repo-probe-j43` (a detached worktree of `repo-track/main`
with `J43:` log points in `session.rs` and `acp.rs`) and its build dir is
`~/arbos-qa/target-probe-j43-desktop`. Neither is used by the loop. They are worth keeping while the
desktop leg is the least stable part of the library: an instrumented app plus 16-second incremental
rebuilds turned a four-cycle mystery into a named commit in about an hour.
