---
cursor:
  subagentId: "bc-b4f4cdba-0146-5dea-9731-24ea2538adcd"
---

# qal-j27 — **WITHDRAWN: not a product bug.** The three scenarios typed into a sub-chat and read `root`

> **Withdrawn 2026-09-18 06:00, on the features agent's read**
> (`internal/qa/inbox/2026-09-18-qal-j27-the-new-chat-is-not-root.md`, driven on `main` `a7187d3a`).
> `d.new_chat()` clicks `new-subchat`, which **mints a new kernel agent** (`chat-<ms>`, via
> `desktop/src/kernel.rs::mint_chat` → `arbos_core::create_chat`). The project's main chat is `root`; the
> sub-chat is not. `mt-01` and `mt-04` then read `inbox_kinds(place, "root")` and
> `transcript(place, "root")` — an agent nobody had typed into. Hence the empty inbox and the `None`
> index that this file called data loss.
>
> Their driven evidence: `inboxes right after typing: {'chat-1789710916728': ['…user-000.md'], 'root': []}`
> — the typed line became an inbox file within 3 s, on the chat agent, and is on that agent's transcript
> (`follow_up_index=5`). Nothing was lost.
>
> Confirmed here after fixing the scenarios to take the agent from `sessions[].agent_session`:
> **`mt-01` passes in 55.3 s**, `agent_typed_into: "chat-1789711163119"`.
>
> `xp-01`'s first-line half is also closed: it types into the main chat, so `root` was right, and on
> today's `main` the line lands there within six seconds. This machine measured it on `3e36fb8e645b`
> (cycle 5, ~01:00); several kernel changes have landed since, #563 among them. The scenario stands as
> the check.
>
> **What I got wrong.** I checked the boundary on the writing side — `sq-01`, `sq-02`, `im-01`, `im-02`
> passing on the same build, and `xp-01`'s second line landing through the same `send()` — and concluded
> the product. I never asked the same question of the **reading** side: whether the agent the scenarios
> read was the agent they had typed into. Three scenarios agreeing is not corroboration when all three
> share one wrong assumption; it is one fault counted three times. The next version of the boundary check
> has to include "does the probe read what it wrote".
>
> The original text is kept below because the mechanism it describes — an empty inbox on `root` — is
> exactly what a reader of the wrong agent sees, and that is worth recognising next time.

---

# (withdrawn) a line typed into the desktop while a turn is running never becomes an inbox file — it is not steered, and in two of three scenarios the person's words are lost without a word

- Measured at: the desktop app and `arbos-kernel 0.2.0 3e36fb8e645b` (cycle 5's desktop build, 2026-09-18 04:03–04:10), driver taken from the app's own commit. Rollouts `internal/qa/rollouts/20260918T040323Z-mt-01-typed-while-running-steers`, `…T040414Z-mt-04-queue-survives-window-restart`, `…T041012Z-xp-01-dead-kernel-line-stays-in-its-project`.
- **First seen 2026-09-16 21:38** (drafts `8772c1e26f`, `edcfddb4ae`), again in cycles 4 and 5 tonight; `xp-01`'s half has **six** sightings since 2026-09-16 12:15 (draft `3969d1570d`). Never promoted, because the desktop leg's output was drowned in `driver-exception` for two days (`qal-j24`).
- Class: data loss with no notice — the person's typed words. The day's headline class.
- Feature: the desktop composer's send path while a turn is running; the kernel's steer inbox file.

## The one mechanism, in three scenarios' words

| scenario | what it recorded |
|---|---|
| `mt-01-typed-while-running-steers` | `inbox_after_typing: []` — no inbox file at all. "the typed line landed at transcript index **None**, the first turn_complete at 6" |
| `mt-04-queue-survives-window-restart` | `inbox_before_quit: []` — the follow-up typed during the turn never reached the kernel's inbox, and after quit + relaunch it is not on the transcript |
| `xp-01-dead-kernel-line-stays-in-its-project` | the line typed while the kickoff turn was running appears **nowhere in the rollout** — not the transcript, not the window's session file, not the app log |

The common fact is the empty inbox: a line typed while a turn runs does not become an inbox file. Where the
window is then restarted or its kernel killed, the words are gone for good.

## What bounds it, and why this is not the known queue gap

Three neighbouring contracts **pass** on the same build, in the same cycle, which is what makes this
specific rather than "the composer is broken":

- `sq-01-stop-holds-the-queued-follow-up` — **pass**. A follow-up held by Stop does reach the kernel.
- `sq-02-desktop-stop-holds-follow-up` — **pass**. The window's held row and the kernel's inbox agree.
- `im-01-bash-yields-to-the-users-words` — **pass**. An attached bash does yield to a typed line.
- `im-02-desktop-no-quiet-line-while-streaming` — **pass**.

So the window→inbox path works when Stop holds the line, and the yield path works for an attached bash.
What does not work is the plain case: **type while a turn is running.**

And `send()` itself is not the fault. In `xp-01` the *second* line — typed while A's kernel was dead —
**did** land after the kernel respawned, through the same `d.send()`. The line that vanished is the one
sent while a turn was running.

## What we expect

A line the person typed is either delivered or said to be held; never neither. Concretely:

1. Typing during a running turn writes the inbox file (`kind = steer`, per the feature `mt-01` exists
   for), so the words survive a kill, a quit and a relaunch.
2. If it cannot be written, the window says so on the chat — the `qal-j09` rule: a write that failed is
   not a write that happened.

## Regression check

`mt-01`, `mt-04` and `xp-01` as they stand; they have been failing since 16 September and will pass when
this does. The scenario to read first is `mt-01`, because its `inbox_after_typing` note names the
mechanism directly rather than the symptom.

## Two honest qualifications

- **One of `mt-01`'s two breaks bounds a race and should not be the verdict.** `mt-01-typed-not-a-steer`
  requires the steer file "within 3 s of typing", which is a bound on how fast something opportunistic
  happens — the sixth review step says not to assert that. The verdict here rests on the other break,
  `the typed line landed at transcript index None`, and on `inbox_after_typing: []`: not late, absent.
  The 3-second bound is worth re-reading when this is fixed.
- **All three drive the app through the desktop driver**, so a rig fault is the first thing to suspect —
  which is why the boundary above matters. Four neighbouring scenarios exercising the same composer, the
  same driver and the same `send()` pass in the same cycle, and the same `send()` delivers the line in
  `xp-01` a few seconds later. That is what makes this the product's and not the rig's.

## How it stayed unread for two days

The drafts were written each time (`8772c1e26f`, `edcfddb4ae`, `3969d1570d`, `11b8de99d0`) with
`Suspected location: (fill in)`, in a desktop leg where sixteen other scenarios were dying on a stale
selector and reading as `driver-exception`. Nobody could see three real findings inside that noise. The
lesson belongs with `qal-j24`: a rig fault that fails early does not cost you the scenarios it breaks, it
costs you the ones it hides.

## The question the features read left open, and how it is answered

The same note observed that in the `SLOW_WORKER` shape the chat's own turn **ends once the spawn
returns** — the worker's turn belongs to another agent — so `mt-01-follow-up-after-turn`, which
required the typed line to land before the first `turn_complete`, "cannot hold in that shape", and
asked what the scenario means to assert there.

It is review rule 6: the assertion was right about the intention and wrong about the mechanism. It
passed or failed on whether the spawn returned inside the scenario's 8-second sleep. The features
agent measured the line landing *after* the first `turn_complete` (`follow_up_index=7`); cycle 6
measured the other side and passed. `mt-01`'s recent history is exactly that coin: pass, break,
break, break, skipped.

Repaired 2026-09-18 by making the assertion follow an observation rather than an assumption.
`mt-01` now reads, from the driver's state at the moment of typing, whether *this chat's own* turn
is open, and records it as `own_turn_open_when_typed`:

- always asserted, because it is the property a person cares about: the typed line reaches the
  chat's transcript (`mt-01-typed-line-lost`) and something answers it
  (`mt-01-typed-line-unanswered`)
- asserted only when the chat's own turn was open: the steer file, and the boundary ordering
  (`mt-01-follow-up-after-turn`) — the cases where the mechanism can hold
- otherwise a note says why the boundary was not asserted, so a reader is not left guessing
  whether the check was skipped or forgotten

`mt-02` makes the same ordering claim from the kernel side, where `wait=true` should hold the turn
open for up to `wait_secs`, so the mechanism looks sound there — but it sits in library half B and
has produced no verdict in the logs I have, so it is left alone until cycle 9 measures it rather
than changed on an argument.
