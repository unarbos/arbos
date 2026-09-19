# qal-j43 — a chat opened during a place's kickoff turn silently ate what you typed

- **status**: **REOPENED 2026-09-18 20:10 — regressed on `main` by `2ea8d565`.** Fixed by `1768ec83` (#656) at 14:15, broken again at 19:11, six hours later. `kf-01` caught it on its first real cycle.
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

## Reopened: `2ea8d565` brought it back

Cycle 11's desktop step broke `kf-01`, `mt-04` and `dg-01` — the same three this bug always took.
`kf-01` recorded exactly its own contract failing:

```
kickoff_running_when_minted: true
composer_cleared_so_line_accepted: true
transcript_exists: true
line_landed: false
```

Bisected the same way as the first time, on the same kernel, `kf-01` only:

| app commit | what it is | breaks |
|---|---|---|
| `fba8688d` | earlier today | 0 of 5 |
| `5925a156` | 18:43 | 0 of 2 |
| `01b32cd5` | *the chrome Jacob asked for — panel, composer, clear, model card* (18:58) | **0 of 6** |
| `2ea8d565` | *a cleared chat is the centered empty chat again* (19:11) | **5 of 6** |
| `1b4ef7a9` | current `main` | 3 of 3 |

So `2ea8d565` is the one.

### It takes all three, confirmed rather than assumed

Cycle 11 broke `kf-01`, `mt-04` and `dg-01` together, which is the signature this bug always had.
Rather than assume one cause, I ran the other two against `01b32cd5` — the commit that is clean for
`kf-01`:

| scenario | `01b32cd5` | `1b4ef7a9` (cycle 11) |
|---|---|---|
| `mt-04-queue-survives-window-restart` | pass (18.1 s) | break |
| `dg-01-a-sub-chats-turn-starts-under-the-harness` | pass (4.9 s) | break |

Same regression, all three. `dg-01` is doing what it was written for again: it was kept as a
diagnostic that would go green when this answered itself, and it goes red when it comes back. Note it is **not** the commit that touched the composer — `01b32cd5`
reworked panel, composer, clear and the model card and is clean across six runs. The one that
broke it changes what an **empty chat** renders as, which is exactly the state a chat minted during
kickoff is in.

### This is the point of keeping the guard

`qal-j43` was fixed at 14:15 by `1768ec83`, a layout commit that never mentioned it, and broken
again at 19:11 by `2ea8d565`, another layout commit that never mentioned it. Six hours between a
fix nobody knew they were making and a regression nobody knew they were causing.

When I closed this I wrote that the incidental fix was the finding, "because a fix nobody knew they
were making is a fix nobody can keep", and kept `kf-01` on that argument rather than deleting a
check that only passed. It earned that on its **first run in a real cycle**.

One correction to my own confidence: I called the original 4/4 "deterministic". At `2ea8d565` the
break rate is 5 of 6, not 6 of 6 — one conclusive pass slipped through. So the underlying fault is
a race that these layout changes widen or narrow rather than a switch they flip. `kf-01` should be
read as a rate, not a verdict, and a single green run of it does not mean much.

Handed to features in `internal/qa/inbox/2026-09-18-qal-j43-regressed-by-2ea8d565.md`.

`deploy/kf01-rate.py` reads the rate out of the rollouts rather than off verdict lines, and
separates the third outcome the verdict hides: a run where the kickoff turn had already finished
at mint time proves nothing either way, and `kf-01` reports that as a pass because nothing broke.
Read it **within one build** — this bug moves with the app, so an aggregate across builds is
meaningless.

## Score on `8af868422fcb` (2026-09-19 09:15)

The rate, read within one build as it has to be — cycle 15's desktop step built the app fresh at
`8af868422fcb`, and every run below is against that binary:

```
cycle 15's own run   LOST   (window hit, composer cleared, line never arrived)
four runs by hand    LOST, LOST, LOST, LOST
```

**5 of 5 conclusive, no missed windows.** `mt-01`, `mt-04` and `dg-01` broke alongside it in the
same step, which is this bug's full signature.

So `2ea8d565`'s regression has now been scored on three separate app builds — `1b4ef7a9` (3 of 3),
`55c82877`/`01b32cd5` era (5 of 6), and `8af86842` (5 of 5). It is not drifting back on its own.

The retry I added is earning its place quietly: none of these five needed it, but the one that
would have been a hollow "no window" pass is the reason the five are all conclusive.
