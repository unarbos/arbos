---
cursor:
  subagentId: "bc-32d10b66-6bef-50c3-9ccf-4350ba54f23a"
---

# GPT Live in the iPhone app

**Short answer: yes, it already works. The phone dials the same gateway, and the gateway is GPT-Live now.** Three small phone-side gaps remained; they are on [PR #485](https://github.com/unarbos/arbos/pull/485).

## How the phone reaches GPT-Live (checked 2026-09-17 18:50Z)

Terms first. The *gateway* is our voice server (`voice-server/`). The *engine* is the speech model behind it. `--engine openai` means GPT-Live speaks; the Arbos kernel answers project questions through *client delegation* (GPT-Live hands the question to us, we ask the kernel, we hand the answer back for it to speak).

1. The phone reads its voice URL from `https://raw.githubusercontent.com/unarbos/arbos/qa-results/voice-endpoint.txt` (`ios/Arbos/Settings/EndpointDirectory.swift`). The gateway's supervisor keeps that file fresh. The token comes from the vault item the phone already holds. Nothing changed there.
2. I opened a session on that exact URL with that token. The gateway answered:
   `session.ready { engine: "openai", asr: "openai/gpt-live-1", tts: "openai/gpt-live-1", reply: "openrouter/google/gemini-2.5-flash", tools: [send_agent, agent_status, ask_arbos], kernel: true, answerer: "model", via: "gateway" }`
3. The phone's `answersItself` flag (`ios/Arbos/Voice/VoiceSession.swift`) was already true for this reply, because `reply` is not `none`. So the phone does **not** forward transcripts to the kernel on its own, and there is no double answer.
4. Barge-in, first word, token, and URL are all gateway-side properties. They were measured to survive the move to GPT-Live (see `internal/gpt-live-status.md`). The phone sends `interrupt` and `client.speaking` exactly as before.

So Jacob's acceptance exchange ("hey" → "hey"; "what's the status on the project" → "one sec let me check" → … → "the status is …") runs on the phone with no client change.

## What I built anyway (PR #485, branch `cursor/ios-gpt-live-f23a`, off `main`)

| Gap | What happened | Fix |
|---|---|---|
| Project line under the orb was blank | The gateway now sends `project: "machine/project"` (a string) with the roster's name and icon in `project_info`. The phone read only the old dict shape. | `SelfHostedVoiceSession` reads `project_info`, then a dict in `project`, then the string. |
| `openai` engine not named | `answersItself` was true only by way of the `reply` field. A gateway started with `--reply none` would have made the phone answer twice. | `answersItself` now also checks `engine == "openai"`. |
| No working sound | During the "one sec let me check" silence, the phone was silent. The desktop plays a sound from the gateway's `agent.activity` frames; the phone ignored the frame. | New `ios/Arbos/Audio/WorkSound.swift` and a handler in `CallViewModel`. |

The working sound, in one paragraph. The gateway sends `agent.activity { agent, state, tool?, detail? }` on every change. `state` is `working` (a turn is running), `tool` (inside a tool call), or `idle`. While any agent is not idle the phone plays a soft tick: 880 Hz, 45 ms, damped, every 2.5 s — the same tick the desktop's `ticks` mode uses (`desktop/src/voice_ws.rs`), so both surfaces sound like one product. A command starting on the main agent adds one tick at once. The tool's name and detail go on the note under the label. The sound waits while a reply plays or while the caller talks. It stops after 12 s without a frame, so a dropped link cannot tick forever. It plays through its own `AVAudioPlayer`, not the reply path, so it is never reported to the gateway as `client.speaking` and a barge-in never cuts it.

## What I could not do here

This worker has no Xcode, so the PR's first compile was CI's "iPhone (build, simulator)" job on macOS: it passed (`xcodebuild build`, 1 min 8 s, [run 35261713981](https://github.com/unarbos/arbos/actions/runs/35261713981)). I could not hear the tick myself. The change is small and additive: one new file, one new `VoiceEvent` case handled in the single exhaustive switch (`CallViewModel`); every other switch over `VoiceEvent` has a `default`. The mobile loop's rig is the right place to hear the tick on a real phone.

## For the mobile and desktop loops

- The tick's shape is copied from the desktop's `ticks` mode. If the desktop changes its sound, change `WorkSound` too, or the two surfaces drift.
- `agent.activity` is the only source of the sound on both surfaces. Do not add a timer.

---

## Status: the call's text in the project chat (iPhone loop, `bc-7c66cfa8-381e-5700-9d78-3129f338a4fa`, 2026-09-17 21:00Z)

Appended by the mobile loop; the sections above are the voice worker's and are untouched.

**Answer: no, #485 does not do this. A change was needed and is on
[#502](https://github.com/unarbos/arbos/pull/502), branch
`cursor/mobile-call-text-in-chat-a4fa` off latest `main`.**

### What was there before, measured

A GPT-Live call against `pod`, four consecutive runs. The kernel's transcript
total before and after each call: 1917 → 1921 → 1925 → 1929 → 1933. **Four
lines every time**, and those four are always the same three-plus-one:

```
1919 user          What is the status on the project?
1920 assistant     This project (poems, sorting algorithms, and the nine
                   J-series math-library fixes …) is fully done, tested
1921 turn_complete
```

What was actually said on that call was six turns:

```
"Okay."                              -> "Hey! How can I help?"
"What is the status on the project?" -> "One sec, let me check. Everything's
                                         done - all those math fixes and the
                                         little extras … committed on their
                                         own branches"
```

So two things were wrong. The small talk was **nowhere** — it is answered by
GPT-Live and never touches the kernel, so nothing recorded it. And the half
that did appear was **the kernel's wording, not the words Jacob heard**.

### What the change does

`ChatStore.spoke(_:byUser:)` puts a line in the chat and sends nothing —
display only, no kernel wake. The call feeds it both halves: each
`transcript.final`, and the spoken reply gathered from the assistant deltas
when the reply ends. Lines are marked spoken, so they draw with the existing
small mark.

A spoken question that gets delegated is recorded by the kernel as well, so
its replay **replaces** the local copy rather than sitting beside it, matched
on the text — the same shape already used for a typed line's pending card.

**Typed lines during a call are unchanged**: they still go through `send` and
still wake the kernel. That path was not touched.

### What is proven and what is not

Proven, by count, four runs: a call's small talk does not wake the kernel,
and only the delegated turn is recorded. That is the measurement that
establishes the gap, and it is unchanged by this PR — the new lines are
display-only, so the totals stay exactly as they were.

**Not yet shown on screen: the spoken lines appearing in the chat.** Three
attempts to photograph it failed on harness navigation rather than on the
feature — leaving the call goes through a context menu, and the injected clip
is consumed when the audio engine starts rather than when the call is entered
from a chat. The code builds and the path is short, but I am not claiming the
screenshot until I have it. It is the first item of the next cycle.

### One thing for whoever owns the shape

The kernel's answer and GPT-Live's spoken answer are **different text** for
the same question. Both are now in the chat: the kernel's because it is
replayed, the spoken one because it is what Jacob heard. That may be right —
one is the record, the other is the conversation — or it may read as the same
answer twice. I have not guessed; say which is wanted and I will make it so.

### Update 21:20Z — shown on screen, and one claim above corrected

**The screenshot the section above owed now exists**: `media/mobile/cycle-53/01-chat-shows-the-call.png`. The chat after a call reads:

```
day.                                                   [user, Spoken]
I didn't quite catch that — could you say more about what you mean by "day"?
Worked 10s
What is the status on the project?                     [user, Spoken]
One sec, let me check.
This project (poems, sorting algorithms, and the nine J-series math-library
fixes …) is fully done, tested, and committed …        [the kernel's text]
Worked 10s
This project is fully done, tested, committed on its own branches …
                                                       [the spoken answer]
```

Both halves of the call are there, marked Spoken, and each question appears
once — the local copy is replaced by the kernel's replay, so the dedup works.

**The correction.** I wrote above that small talk "is answered by GPT-Live and
never touches the kernel". That is not reliably true. In this run GPT-Live
delegated `"day."` as well, and the kernel recorded it (`1939 user day.`,
`1941 assistant I didn't quite catch that …`). Whether a trivial utterance is
delegated is GPT-Live's decision, not a fixed property of small talk. The
four-run measurement that founded the claim stands for those runs — "Okay."
happened to be answered locally — but it does not generalise.

Nothing about the change depends on it: `spoke()` sends nothing either way,
and the kernel lines are the gateway's delegation, which happens with or
without this PR.

**The shape question now has a picture.** The kernel's answer and GPT-Live's
spoken answer sit one after the other, saying the same thing in different
words. Visible in the still above. Still not guessed at.

### Update 21:40Z — the shape question is answered and built

**The decision:** the project chat shows the spoken conversation. Where Live
spoke the kernel's answer, the spoken row stays and the kernel's parallel
text row for that turn goes. Never both wordings. [#502](https://github.com/unarbos/arbos/pull/502)
is now exactly that rule and nothing wider.

**How it decides, and why not by comparing text.** The two wordings are
different on purpose — that is the whole problem — so matching on the words
would never fire. The rule is scoped to the *turn* instead. A row the app
put there itself is marked `spoken`; a row from the kernel is not. When a
spoken answer arrives, every unspoken agent row since the last question is
removed, and any that arrives later in the same turn is not added. A new
question, or a new kernel turn, clears the flag — so a turn Live never
spoke keeps the kernel's answer, which is the case that would otherwise
lose the reply altogether.

**What is deliberately kept.** `Worked 7s` and the tool lines stay. They are
the record of what the kernel did, not a second wording of what it said, and
without them a delegated turn would look like Live answered it alone.

**The evidence**, from a run at 21:34Z on the acceptance conversation:

| | |
|---|---|
| `media/mobile/cycle-54/00-before-the-rule-both-wordings.png` | the same question answered twice, one under the other |
| `media/mobile/cycle-54/01-one-wording-per-turn.png` | the same question, one answer: the spoken one |

The second still is the claim. Under `What is the status on the project?` it
reads `Main agent is idle, and all the workers are finished.` / `Worked 7s` /
`The main agent's just picked something up. It's taken a turn.` / `Okay,
confirmed, everything's done and committed on its own branches…` — all of it
spoken wording.

**The kernel wrote its own answer for those same turns** and it is not on the
screen: seq 1957 and 1961, both `This project (poems, sorting algorithms, and
the nine J-series math-library fixes — J143228 through J214750) is fully
done…`. That the app can render that text is not in question — it is on the
screen directly above, replayed from history, and the `Worked 7s` row proves
the app watched the same kernel turn. It dropped the wording, not the turn.

**One limit, stated plainly.** Spoken rows are display-only and are never
sent anywhere, so they do not survive the app being restarted. After a
restart the chat replays the kernel's transcript and shows the kernel's
wording for those turns — `media/mobile/cycle-54/02-history-replay-is-the-kernels-wording.png`.
That is the honest fallback rather than a bug: the kernel's transcript is the
only record that persists, and a spoken paraphrase the kernel never heard
cannot be in it. Worth knowing before someone reports the rule as broken.

**The route to this still is now a committed script**, `deploy/mobile/scenarios/call-text-in-chat.sh`,
because three earlier attempts were lost to navigation rather than to the
feature. The thing that defeated them: a call screen is wordless by design
and its composer and close button only exist once the screen is pulled down,
so a tap for the close button before the pull gesture finds nothing.

M-179 is closed.

**22:30Z — CI green on #502**, all five checks including the iPhone
simulator build, on the commit that carries the rule. Nothing further has
been pushed to that branch.
