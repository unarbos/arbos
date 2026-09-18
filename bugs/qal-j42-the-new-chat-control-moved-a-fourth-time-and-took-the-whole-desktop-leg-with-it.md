# qal-j42 — the new-chat control moved a fourth time and took the whole desktop leg with it

- **status**: `new_chat` fixed and verified; `mt-01` still failing for a reason I have not found
- **found**: 2026-09-18 14:48, reading cycle 9's desktop step
- **app**: `1beec0a1fd98` (the break), `d2a807e48423` (the last one that worked)
- **kernel**: `arbos-kernel 0.2.0 cecd48e1bd76 protocol 1`

## What happened

Cycle 9's desktop step produced **14 `driver-exception` breaks**, every one
`TimeoutError_: timed out waiting for element 'new-subchat'`, plus a broken `journey-linux`. All 14
took 12.0–12.1 s — a uniform duration is the signature of one shared failure, not fourteen.

The cause is the app, between cycle 8's build and cycle 9's:

```
8d6cb643 desktop: the panel's tabs sit on the window strip, and Project never covers the chat
```

157 lines of `panel.rs`. `new-subchat` is **still in the source** (`panel.rs:1207`) and renders in
neither place the helper looks. Measured on `1beec0a1fd98`:

| | |
|---|---|
| leaves at launch | 30, `new-subchat` **absent**, `toggle-panel` present |
| leaves after `toggle-panel` | 37, `new-subchat` **still absent** |
| clicking `panel-tab-0`, `panel-tab-strip`, `panel-tabs` | reveals nothing |
| `new-tab`, `panel-new-tab` | mint no session, **even after nine clicks each** |

The nine clicks matter: `qal-j33` established that the first click after launch does not land, so one
click and a wait cannot tell "wrong control" from "window not listening yet". Nine can.

## The fix, and why it should last

`app.key("cmd-n")` mints a chat on the first press, makes it active, and gives it its own kernel
agent:

```
before: sessions [0, 1]  active_session=1
after:  sessions [0, 1, 2]  active_session=2  minted=[2]
the minted chat is active: True      agent: 'chat-1789743412669'
```

⌘N is the control's **own documented shortcut** — `panel.rs:1210` renders it as
`Tooltip::with_keystroke("New sub-chat", "⌘N")`. So `new_chat` now presses the keystroke and only
clicks `new-subchat` when that leaf is actually present.

This is the **fourth** time this control has moved: out of the sidebar the 2026-09-13 layout removed,
into the right-hand panel (`qal-j24`), and now out of reach entirely. A button's path is the most
volatile thing about it; its keystroke is a contract the app states in its own tooltip. Reaching for
the keystroke is not a workaround, it is the more stable of the two affordances.

`desktop-rapid-session-switch` passes again on the fixed helper.

## What I have not solved

`mt-01` now fails later, consistently, on
`TimeoutError_: timed out waiting for root running` — the line is sent and no session ever reports
`streaming` or `turn_open`. In its rollout the chat agent has **0 events** and `root` has no user
lines, so the line reached neither transcript.

I could not reproduce that in isolation. On the same app and kernel, driving the same steps by hand:

| checked | result |
|---|---|
| `new_chat` mints and activates a chat | yes, agent `chat-…` |
| `composer-field` after ⌘N | present, **reachable**, visible |
| composer focused | already focused; focused after 1 click |
| typing + Enter | composer clears, line appears in the app's items |
| the line on the **kernel's** transcript | **present twice** on the chat agent |

So sending into a ⌘N chat works end to end when I do it directly, and does not when `Desktop.send`
does it inside `mt-01`. The difference is inside that path or in the prompt itself — `SLOW_WORKER`
carries backticks and a semicolon, which is the one thing my probe did not type.

That step is now taken, and it cleared both suspects. Driving `Desktop.send`'s exact sequence —
`wait_element(reachable=True)` → `focus_composer` → `type(text + "\n")` — twice, once with
`SLOW_WORKER` and once with plain words:

| arm | focus | composer after Enter | user lines on the chat's transcript | a session streaming |
|---|---|---|---|---|
| `SLOW_WORKER`, backticks and semicolon | 1 click | empty | **1** | yes |
| plain words | 1 click | empty | **1** | yes |

So the helper is fine and the payload is fine. I also tested `mt-01`'s own predicate verbatim
against the live state, in case it raised inside `wait_state` and read as a timeout: every project
carries a `sessions` key and the predicate **evaluates True**.

What is left is the launch path. `ds.Desktop(cx)` starts the app through
`hidden_store_binary(cx.scratch)` — inside ns-wrap, store hidden and home read-only — while my probe
launched the binary directly. `desktop-rapid-session-switch` passes through the harness, but it
never sends, so it does not exercise this. The app's own log from a failing run holds only two lines
and nothing about the typed line, one of which is worth someone's attention on its own:

```
session 2 (chat-…): filed parent Some("root"), the kernel's record says None; the record wins
```

Parked here rather than chased further: it is one scenario, the rest of the leg is restored, and
cycle 10's desktop step measures the real extent for nothing. If `mt-01` is the only one still
failing after that, the next probe is `Desktop(cx)` through ns-wrap versus a direct launch, which is
the only difference left.

## Where this belongs

The desktop leg is the loop's least stable ground, and every instance has had the same shape: an
element the helper names by path stops being where it was, and the scenarios that depend on it all
fail at once with the same exception, naming the element rather than the change that moved it. The
fix each time has been to reach for something less positional — the panel toggle (`qal-j24`),
`panel-agent-<id>` rather than a row index, `agent_session` rather than a tab number (`qal-j27`),
`composer.focused` rather than a click landing (`qal-j33`), and now a keystroke rather than a button.

## Measured: the fix restored the leg, and exactly two scenarios remain

Re-ran a representative five of cycle 9's casualties on the fixed helper
(`arbos-kernel 0.2.0 cecd48e1bd76`, app `1beec0a1fd98`):

| scenario | before | now |
|---|---|---|
| `desktop-fresh-place-no-notice` | driver-exception | **pass** |
| `desktop-kill-kernel-under-ui` | driver-exception | **pass** |
| `mt-17-plan-strip-empty-chat` | driver-exception | **pass** |
| `mt-24-relaunch-restores-active-tab` | driver-exception | breaks on **`qal-j35`** — the real product bug, correctly detected |
| `mt-04-queue-survives-window-restart` | driver-exception | still `timed out waiting for root running` |

So ⌘N restored the leg, and `mt-24` now reaches its actual finding instead of dying at the door.

## The remaining two, narrowed to one line

`mt-01` and `mt-04` are the **only two scenarios in the library** that wait on this:

```python
d.app.wait_state(lambda s: any(c.get("streaming") or c.get("turn_open")
                               for p in s["projects"] for c in p["sessions"]),
                 timeout=40, what="root running")
```

`desktop-kill-kernel-under-ui` sends twice and passes, so it is not "new_chat then send". It is that
wait. What has been eliminated:

- the helper — `Desktop.send`'s exact sequence delivers the line and a session does report streaming;
- the payload — the same with `SLOW_WORKER`'s backticks and semicolon, and with plain words;
- the predicate — run verbatim against the live state it **evaluates True**, and every project carries
  a `sessions` key, so it is not raising inside `wait_state` and reading as a timeout;
- the fields — `streaming` and `turn_open` are still per-chat in the driver's serialisation
  (`driver.rs:1344`, `:1349`), so they have not moved or been renamed.

What is left is the one difference between my probe and the harness: `ds.Desktop(cx)` launches the
app through `hidden_store_binary(cx.scratch)` — inside ns-wrap — and my probe launched the binary
directly. Model turns plainly work under the harness (`journey-linux` passed in cycle 8 at 147.8 s),
so a blanket "no model under ns-wrap" is already ruled out; what is not ruled out is a sub-chat's
turn specifically.

Next probe, one run: the same send through `ds.Desktop(cx)` versus a direct launch, watching both the
chat agent's transcript and the session's `streaming` flag. That is the last difference standing.

## Six eliminations, and I did not find it

The remaining two (`mt-01`, `mt-04`) are now a well-bounded open question rather than a lead. A
diagnostic scenario, `dg-01-a-sub-chats-turn-starts-under-the-harness`, does `mt-01`'s steps inside
the harness and records a per-second timeline instead of a verdict. It says:

```
agent chat-1789744436578   connection live
transcript lines after 40 s: 0      inbox files: 0
no session ever reported streaming or turn_open
```

So under the harness the typed line **never reaches the kernel at all** — no inbox file, no
transcript line — while the composer clears and the session sits `live`. Driven by hand it arrives
every time. Everything I could name as different has been tested and cleared:

| difference | tested by | result |
|---|---|---|
| `Desktop.send`'s sequence | driving it verbatim | line arrives |
| the payload's backticks and semicolon | `SLOW_WORKER` vs plain words | both arrive |
| `mt-01`'s predicate raising inside `wait_state` | running it against the live state | evaluates **True**; every project has `sessions` |
| `streaming`/`turn_open` moved or renamed | `driver.rs:1344`, `:1349` | still per-chat |
| the ns-wrap launch (`hidden_store_binary`) | probe launched through ns-wrap | line arrives |
| the agent's missing parent | probe chats are parentless too | 4–7 transcript lines each |
| `HOME` set to a scratch dir (`run.py:444`) | probe with a scratch HOME | line arrives |

The app's own log carries one line from a failing run that may or may not be related, and is worth
a look from whoever owns sessions either way:

```
session 2 (chat-…): filed parent Some("root"), the kernel's record says None; the record wins
```

**One difference I noticed and did not test**: the harness writes
`$XDG_CONFIG_HOME/arbos/config.toml` with `api_base`, `api_key_env`, `model` and `window_tokens`
(`run.py:440`), while my probe's XDG directory is empty. That should not govern whether a *user line
is recorded* — the kernel writes that before any model call — but it is the last difference I can
name, so it is the next thing to try.

`dg-01` is kept rather than deleted. It is cheap, it is tagged `diagnostic`, and it turns this from
"two scenarios time out" into a timeline anyone can read. If it goes green on a later build, that is
the answer arriving without anyone chasing it.
