# What Max Blade’s CNVS teaches Arbos

Jacob asked us to look at [this tweet](https://x.com/_maxblade/status/2099967019462398463) and steal what makes it easy to use.

This note is facts first, then lessons. It does not start leftover Jev slices A–G. It does not publish `v0.2.0`.

---

## Who this is

**Max Blade** (`@_MaxBlade`). He builds **CNVS**, a paid Mac app at [cnvs.dev](https://cnvs.dev).

CNVS is a canvas. You run several coding agents on one screen. Claude Code, Codex, Cursor, Gemini, Kimi. You talk to them by voice or by typing. The app is closed source. There is no public repo.

---

## What the tweet actually shows

Posted 15 September 2026. About 14,500 views. 224 likes. 23 replies. 3 quotes.

His words, in full:

> if you forget what an agent is doing
>
> just press f key.
>
> I used the apple foundation model that runs on your mac for a free and super fast live summary of what the agents are working on.
>
> this feature is live in CNVS.

The only link is a **6.6 second video**. No GitHub. No Jev. No TypeSafe. No OpenRouter.

X blocked a full reply thread from this seat. Counts exist. Reply text does not.

---

## Jev, in one line

**Jev** is TypeSafe System One on OpenRouter. Official slug `~typesafe/jev-latest`. A decisions model that picks the next step (`tool` / `llm` / `done`). It is not a chat model.

---

## Honest note about “he used Jev well”

Jacob said Max did a good job of using Jev.

This tweet does not use Jev.

It uses **Apple Foundation Models**. That is Apple’s on-device language model. It writes a short title. It stays on the Mac. It costs nothing per request.

I searched the tweet, the video, [cnvs.dev](https://cnvs.dev), the [1.4.0 Shorthand notes](https://cnvs.dev/release/1-4-0-shorthand/), and the open web for Max Blade + Jev / TypeSafe. No public CNVS integration showed up.

Steal the **face** of the product. Do not invent a Jev wire that is not there.

---

# 1. Facts about their product

## What you see before F

Three agent windows on a forest wallpaper.

- **Rocky** — Kimi Code. Title: Database Security Audit. A welcome screen. MCP connected. Idle prompt.
- **Skye** — Claude Code. Title: ios app. A live shell. Grep and git. Status: Cogitating. Token count in the corner.
- **Rubble** — Codex. Title: Landing Page Changes. A live grep / git status. Status: Working. Esc to interrupt.

A bottom bar sits on every view:

- “Type or speak…”
- The workspace name (`maxmax`)
- Numbered slots
- A send target (“Sent to Rocky”)

You type, or you speak. You can aim the line at one agent.

## What you do

You press **F**.

You do not type a question. You do not ask the agents. You do not wait on a chat model.

## What you wait for

Almost nothing.

The 6.6 s clip is the zoom, not a spinner. Terminals fold into cards. No “thinking”. No “summarizing”. No model name.

The [Shorthand notes](https://cnvs.dev/release/1-4-0-shorthand/) say the titles come from the **request you made**, not from the current tool verb.

## How choices are shown

After F, each agent is a card:

| Card | Brand | Title | One line | Status |
| --- | --- | --- | --- | --- |
| Rocky | KIMI CODE | Database Security Audit | deep dive the database and make sure everything’s secure | WORKING · just now |
| Skye | CLAUDE CODE | Ios app | Shell cwd was reset to …/create/maxmax | WORKING · just now |
| Rubble | CODEX | Landing Page Changes | how are we looking on the current landing page changes | WORKING · just now |

Same facts sit in two places:

1. Press F — fleet briefing. Every card flips to its headline.
2. Stage sidebar — the title stays on the row. You do not have to press F again.

The sidebar groups **needs-you**, then agents, then windows. Rows only move when someone needs you.

## How failure looks

The clip does not show a fail.

The notes do. If Apple Intelligence is off, or the Mac is too old, CNVS keeps the **old labels**. It does not invent a chat reply. It does not pretend the briefing ran.

Settings ▸ Summarizer says whether the on-device model is ready.

One card in the demo is weak. Skye’s line is a raw shell event (“cwd was reset”), not the job he asked for. That is the miss: last tool line instead of the goal.

## Is the model’s pick visible?

No.

You never see “Apple Foundation Model chose…”. You see the **goal** and **WORKING**. The model is a means. The card is the product.

---

# 2. Lessons for Arbos

Arbos today, so the map is honest:

- Unified Jev controller is on `main` ([#647](https://github.com/unarbos/arbos/pull/647)).
- Decisions door is in Mac **2060+** ([#667](https://github.com/unarbos/arbos/pull/667)). File-list turns already pick `ls`.
- Fail stays failed. No chat-model fallback ([#664](https://github.com/unarbos/arbos/pull/664)).
- **Choosing the next step** is `hooks.kernel_step`. The desktop draws that line while Jev is asked.

Jev still does not speak. Live still speaks. The gateway still does not call Jev.

---

## Lesson A — Glance without asking

**CNVS:** forget what they are doing → press F.

**Arbos today:** Jacob asked Live “what are you working on”, heard “let me get that information”, then silence. A glance became a **chat turn**.

Do not send “what are you doing?” through Live or through the chat model.

Show the goal that is already on disk: spawn title, last user line, worker name, live step. One key or one strip. No new OpenRouter call.

Jev cannot write that sentence. Jev only picks. The words must already exist.

---

## Lesson B — Show the goal, not the tool verb

**CNVS:** “Database Security Audit”, not `grep`.

**Arbos today:** the live line is `Working` + `Choosing the next step`, then a tool name (`Listing…`). Root Project chat already hides most tools and shows `1 Working · <step>`. Worker chat still shows the machine.

**Choosing the next step** should stay a **flash** (a few hundred ms on a healthy Decisions hop). Then show the **job** he typed, or the real tool. Never leave the choosing line up while a chat model writes. That was the Mac 2036 shot. #664 already ends that lie.

If Jev picks `act=llm`, the next line is **Thinking**. That pick is valid. It is not a fail.

---

## Lesson C — Hide the router

**CNVS:** no “Foundation Model” badge on the card.

**Arbos today:** the live line already says “Choosing the next step”, not “Working jev”. Keep that.

Do not add a second brain. Do not print `act=tool`. Do not show Jev’s confidence to Jacob unless he asks.

A `jev-*` tool id in the transcript is for us. It is not a UI.

---

## Lesson D — Fail in the open

**CNVS:** no model → old labels. The briefing does not fake success.

**Arbos today:** #664 is the same law. Jev errors, times out, or returns junk → the turn **stops**. A failed notice names why. The chat model does **not** answer.

Do not undo #664 to feel more like a chat app. CNVS is friendlier **because** it refuses to invent a reply.

---

## Lesson E — Needs-you on top, stable map below

**CNVS:** the sidebar only reorders when someone needs you.

**Arbos today:** the Project panel already lists workers. Root chat already has `1 Working · <step>` and `Done <name>`. Asks already park.

Steal the sort, not a new canvas. Put **needs you** (ask, approval, failed Jev notice) above **working** above **done**. Do not reshuffle working rows every step. That flicker is why people look away.

---

## Lesson F — Type or speak, aim at one agent

**CNVS:** one bar. “Type or speak…”. “Sent to Rocky”. A short query can be a command. A full sentence is “Do it” — send the words, do not hijack them into a spawn.

**Arbos today:** composer + Live. `clear` is already a UI command, not a chat line. Steer already lands inside the running turn.

Steal the **aim**. A line meant for one worker should say so. A sentence should not become a new child by accident.

Do not merge composer and ⌘K as a program. That is a CNVS problem, not ours.

---

## Lesson G — Honest harness status

**CNVS:** idle / working / needs you, from the harness. Not from a timer on the screen.

**Arbos today:** `agent.activity` frames already carry `working` / `tool` / `idle`. The call-mode work sound follows those frames. A 12 s gap is **unknown**, not “still working”.

Keep that. A silent call and a dead call must not sound the same. A Jev fail must not look like Thinking.

---

# Keep / try / skip

## Keep

1. **Fail stays failed (#664).** CNVS does the same with old labels. A fake chat answer is worse than a stop.
2. **Decisions door (#667) on Mac 2060+.** Jev is asked as `state` + `questions`. Not as chat. That is already the right wire.
3. **Choosing the next step as a flash.** Then the real work. `hooks.kernel_step` stays the name of that wait. Do not rebrand it as a second model.
4. **Jev does not speak.** The glance is existing text. Live stays the mouth.

## Try

1. **One-key fleet briefing.** F, or one Project-page control. Every worker becomes a card: name, goal, working / needs you / failed. Words from the ask and the spawn title. No Jev prose. No Live turn.
2. **Permanent one-liner on each worker row.** Same goal as the briefing. So he does not have to press a key to remember.
3. **Needs-you first.** Failed Jev notice and parked asks sit above working rows. Working rows do not dance.
4. **Aim the composer.** “Sent to \<worker\>” when a line is for one child. A full sentence stays a message.

## Skip

1. **Treat this tweet as a Jev demo.** It is Apple’s on-device writer. Jev cannot write those titles.
2. **Put Jev in the gateway, or ask Jev to speak the briefing.** That breaks the controller we already shipped.
3. **Fall back to the chat model when Jev fails.** That is the old lie. #664 closed it.
4. **Rebuild Arbos as an infinite canvas with themes.** Pretty. Not the friendliness he asked for.
5. **Leftover Jev slices A–G as a new program.** The controller is on `main`. Widen the face, not the roadmap.
6. **Publish `v0.2.0`.** Mac update channel only.
