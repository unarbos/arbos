# Jev for speed: three architectures

Jacob asked what else Jev could do in the app to make it faster. He named voice: can Jev return context for the voice model faster, and what other context can we give it.

This page answers that. It compares three architectures. Then it recommends one.

This is not more mechanical tool picks. That job already has a design: [Jev in the harness](jev-harness.md) and [full integration](jev-full-integration.md) slices A–G. Do not start those slices here. Do not implement. Do not open a PR. Do not publish `v0.2.0`.

A second flavour of the situation-card router does not count as an architecture on this page.

---

## 1. Words used here

- **Jev**: TypeSafe’s System One model. It answers typed questions. It does not write prose. It does not speak. OpenRouter slug `typesafe/jev-latest`. Window: 32,000 tokens. About $0.042 per million input tokens. Output is free.
- **Live**: the voice model on the call (GPT Live today). It hears Jacob. It speaks. It may **delegate** a question to the kernel.
- **Gateway**: `voice-server/`. The phone and the desktop connect to it. It connects to Live and to one kernel.
- **Kernel**: `arbos-kernel serve <folder>`. One project. The agent loop lives here. Jev already runs here for tool routing ([#546](https://github.com/unarbos/arbos/pull/546)).
- **Inject**: text the gateway puts into the Live session. Three kinds: `instructions` (rules), `session.input` (startup history), and mid-call appends (`thinking` = quiet context; `commentary` = words to speak; `instructions.append` = new rules).
- **Pack**: choose which existing lines enter an inject, so the inject fits a budget. Packing is selection. It is not writing.
- **Slice**: one candidate line or short block the packer might keep. Each slice already exists on disk or on the wire. It has an id, a source, a clipped body, and a size in tokens.
- **Budget**: the hard size Live will accept. Startup history: 128 messages / 8,192 tokens (we send at most 40 lines, 600 characters a line, 12,000 characters total). One mid-call append: 500 tokens. Live later compact: original instructions plus up to 8,192 tokens of recent history.
- **Situation-card router**: today’s Jev job. The kernel builds a menu of tools. Jev picks the next mechanical move. That is [jev-full-integration](jev-full-integration.md). Not this page.
- **Fall-through**: if Jev errors, times out, or returns junk, we keep the current inject rules. The call still works.
- **`jev = false`**: the off switch. The old loop and the old inject stay.

---

## 2. The question, in two parts

**Part A — voice.** Today we stuff Live at the start of a call, then drip more text every few seconds. That is slow to *build*, easy to *overflow*, and easy to *miss the fact Jacob just asked for*. Can Jev pick the right slices faster than age-based stuffing?

**Part B — the rest of the app.** The same wait shows up when the desktop strip, the phone orb, or a new tab needs “what is going on.” That is not a tool pick. It is the same packing problem on a different surface.

The earlier full-integration page put a wall around voice: Jev does not sit in the gateway, does not speak, and does not judge small talk ([§16](jev-full-integration.md)). This page keeps “Jev does not speak” and “Jev does not judge small talk.” It asks whether Jev may *pack* what Live reads. Those are different jobs.

---

## 3. Why the current inject is slow

Read [what Live can see](gpt-live-context.md) first. Three stores exist. None is a copy of another.

| Store | Where | What Live gets from it today |
| --- | --- | --- |
| Kernel transcript | `<folder>/.arbos/agents/root/transcript.jsonl` | Older user/Arbos lines if room remains after the screen snapshot. Tool *names* only. No diffs. No file bodies. |
| On-screen chat | The client’s snapshot at `session.start` | Last 40 visible rows: user, Arbos, workers, tool labels, notices, asks. |
| Live session | OpenAI, for this call only | Audio both ways, our injects, delegation ids. Gone when the call ends (`store` is off). |

What we already inject at start:

- **Project identity**: name, `machine/project`, folder, `arbos://` address, how we reached the kernel.
- **On-screen chat** and leftover kernel lines.
- **Workers and activity**: name, state, step, whether root is running.

What we drip during the call (at most one quiet append every 3 seconds):

- A line Jacob typed.
- An Arbos text reply Live did not speak.
- A worker start, finish, report, or tool name.

Measured cost today:

- Session start is about **1.6 seconds**, up from 0.5–0.8 seconds, because we seed chat. The seed waits at most **1.5 seconds** and is dropped if the kernel is slow. The caller’s first word is not delayed by that wait.
- A mid-call append is capped at **500 tokens**. An acknowledgement does not prove Live used it.
- Past about 90% of a 128k window, Live keeps the original instructions and up to 8,192 tokens of recent history. Facts that matter must live in the kernel, not only in the call.

The silence Jacob heard (“what are you working on” → “let me get that information” → nothing) is this shape: Live did not have the fact, delegated, and the spoken path waited on a kernel turn. Stuffing more history at start does not fix a fact that changed *after* start. Dripping every tool name every 3 seconds also does not fix it: the useful sentence is often the worker’s last words or the notes `tldr`, which we do not inject at all.

So the speed problem is not “call Jev instead of grep.” It is “Live holds the wrong 8,192 tokens, then waits on a full turn for the right sentence.”

---

## 4. What other context we can give

Jev cannot invent a sentence. Every slice must already exist. Here is the deck we do **not** send today, or send only as a name.

| Slice | Where it already lives | Why a call wants it | Secret risk | Size if we glance it |
| --- | --- | --- | --- | --- |
| `notes.md` **tldr** | Project store, root’s page | “What are we working on?” | Low if the tldr is the public list | One short list |
| Dated decisions | `docs/project-context.md` headings | “Did we decide X?” | Low | Titles only, not the essay |
| Open plan items | `plan.md` | “What’s left?” | Low | Labels, not bodies |
| Last worker `say` | Child transcript tail | “What did run-tests report?” | Medium: may quote a command | One sentence, already capped in the narrator |
| Last tool **error** | Tool record `error` field | “Why did that fail?” | Medium | One line |
| Git branch + dirty flag | `git status` already on the kernel | “Are we on the fix branch?” | Low | Branch name + yes/no. Never the diff |
| Open ask | Parked `ask` | Live must not decide it; it may *say* one is waiting | Low | Question text we already speak |
| Spend vs cap | Turn usage | “Are we burning money?” | Low | Two numbers |
| Last PR url | `prs.jsonl` / tool output | “Where is the PR?” | Low | One url the tool already produced |
| Files touched this turn | Tool `paths` | “What did we edit?” | Low | Paths only |
| Store doc names | `docs/*.md` names | “Where is the design?” | Low | Names, not bodies |
| Subscriptions | `subscriptions/*.toml` names | “What is watching CI?” | Low | Names |
| Build / kernel version | `/healthz`, hello line | “Are we on the new build?” | Low | One line |
| Artifact paths | `media/<topic>/` | “Show me the screenshot” — Live still must not dump pixels | Low | Paths |
| Last spoken Q and A | Live already has these | Re-inject after Live compact, or they vanish | Low | The lines we already spoke |

Still forbidden on any card we send Jev:

- Vault values, `op://` references, `secret` tool bodies
- File contents, diffs, patches, whole repos
- Live audio
- Whole `notes.md` or whole transcripts

The **tldr**, the last worker sentence, the last error line, and the branch name are the four slices that would have answered Jacob’s “what are you working on” without a kernel essay.

---

## 5. Three laws that every architecture must keep

1. **Jev does not speak.** Live and the narrator stay the mouth. A Jev answer is ids, scores, and keep/drop. Code copies existing text.
2. **Jev does not judge small talk.** Live still decides greetings vs a kernel ask. Putting Jev on that fork makes a second brain on the call. Rejected in §9.
3. **`jev = false` and fall-through keep today’s inject.** Age-based last 40 lines. No new key. No training. No hosting Jev.

Where Jev *runs* is the fight. The three architectures disagree.

---

## 6. Architecture 1 — Inject packer

**Shape.** Jev sits on the inject path. Each time we would stuff Live, we ask Jev which slices to keep. Live receives a shorter, better inject.

This is the thing Jacob named: return context for the voice model faster by packing it.

### 6.1 Where it lives

Two placements, same shape:

- **Gateway calls Jev.** Fastest clock. The voice server needs the OpenRouter key, or a new hop to someone who has it.
- **Kernel packs, gateway injects.** Same key Jev already uses. One extra hop on every inject.

Either way, Jev is *on the call’s critical path* for context. Not for speech. For the brief Live reads before it can answer.

### 6.2 What we send Jev

A **voice card**, not a situation card. Same redactions. Different rows.

```
goal: last spoken user line (clipped)
place: machine/project, folder
workers: name, state, step
slices:
  1. notes_tldr  [notes]  "…"  80 tokens
  2. worker_say:run-tests  [child]  "…"  40 tokens
  3. screen:12  [chat]  "…"  30 tokens
  …
budget: start=8192 | append=500
```

Each slice body is already clipped. Jev does not see a file.

### 6.3 What Jev returns (and what Live actually gets)

Jev returns typed answers, not a paragraph:

| Question | Type | Meaning |
| --- | --- | --- |
| `keep_<id>` | Noul | Does Live need this slice to answer the last ask, or the standing “where are we / what is running” questions? |
| `channel_<id>` | Choice | `instructions` / `input` / `thinking` / `drop` |
| `enough` | Noul | Can we stop adding slices? |

Code then:

1. Drops low-noul slices.
2. Fills the budget, highest noul first, identity slices always first (place must stay).
3. Injects the winning **existing** text through today’s inject kinds.

**Live never sees a Jev string.** It sees the same inject types it sees now. The bytes are a subset.

### 6.4 What gets faster

- Start: we stop spending the 8,192-token budget on old chat lines that cannot answer “where are we.” Room appears for the tldr and the live workers.
- Mid-call: a 500-token append holds the one worker report that matters, not three tool names.
- After Live compact: we can re-pin the identity + tldr into `instructions.append` so they survive.

### 6.5 What gets slower or worse

- Every inject waits on Jev (about 70–500 ms, plus network from the gateway’s region). Session start is already 1.6 seconds. A Jev call inside the 1.5-second seed window will often lose: the seed drops, Live starts thin, then we append late. That is slower than today for the first “where are we.”
- Scoring many slices is the bill rtrvr hit on large pages. A 3-second drip plus a Score pass can cost more than it saves.
- If the gateway holds the key, we have split Jev across two processes. `jev = false` must turn both off. Easy to get wrong.
- This is the wall in the full-integration page: Jev on the gateway, even as a packer, is a second mind next to Live. Jacob will not hear it. He will feel a pause before Live knows anything.

### 6.6 Fall-through

Jev down → today’s age-based seed and 3-second drip. Call still starts.

---

## 7. Architecture 2 — Delegation index

**Shape.** Do not stuff Live. Give Live a short **index**: names, states, and addresses. When Jacob asks a fact, Live delegates a *small* read (“tail worker run-tests”, “read notes tldr”), not a full thinking turn.

This is the opposite of packing more context into Live.

### 7.1 Where it lives

The kernel already knows the roster, the store paths, and the last `say`. Jev, on the kernel, ranks which **pointers** belong on the index. The gateway injects the index as `instructions` (stable) and refreshes it with `instructions.append` when the ranked set changes.

Jev is not on every inject. Jev is on roster change: worker start, worker done, notes edit, plan touch.

### 7.2 What we send Jev

A list of **addresses**, each with a one-line label we already have:

```
place: arbos://arboslife/demo
candidates:
  notes_tldr        notes.md#tldr
  worker:run-tests  agent run-tests · running · cargo test
  worker:fix-ci     agent fix-ci · idle · last say clipped
  plan:3            plan item "hold Live then speak kernel"
  git:branch        cursor/foo
  ask:open          (none)
```

No file bodies. No vault. The label is the glance.

### 7.3 What Jev returns (and what Live actually gets)

| Question | Type | Meaning |
| --- | --- | --- |
| `pin` | Choice | Up to N candidate ids, plus `none` |
| `delegate_first` | Choice | Which address should Live ask first if it does not know the answer? |

Code writes a short index Live can read:

```
INDEX
- place: /home/const/arbos-hub/projects/demo
- working: run-tests (cargo test)
- page: notes tldr (ask kernel if you need the lines)
- branch: cursor/foo
If a fact is not in this index, delegate. Do not invent.
```

Those lines are templates plus the labels Jev picked. Jev did not write them.

**What Live gets:** a map, not a dump. Delegation becomes “read this address,” which the gateway can satisfy with the kernel’s existing `read` / `tail` / `list` frames (the narrator already does this for `more_detail`). That is a file hop. It is not a model turn.

### 7.4 What gets faster

- Live’s window stays small. Compact hurts less.
- “What did run-tests say?” can be a `tail`, not “let me get that information” plus a root turn.
- The 3-second tool-name firehose can stop. The index changes when a worker changes, not when a grep lands.

### 7.5 What gets slower or worse

- A question the index does not name still waits on a full kernel turn. “What are we working on?” needs the tldr **text**, not only the pointer `notes.md#tldr`. If we refuse to put that text on the index, we re-create Jacob’s silence: Live knows *where* to look and still has nothing to say for a second.
- Live must follow “delegate if missing.” It already has that rule and still invents or stalls. An index does not make Live obedient.
- Addresses across machines (`arbos://mac/…`) fail loudly when the Mac is off the hub. The index must not point at an unreachable peer. Today we refuse that call. Keep that.

### 7.6 Fall-through

Jev down → inject today’s identity + workers block, no extra pointers. Delegation stays as it is.

---

## 8. Architecture 3 — Standing brief

**Shape.** The kernel keeps a **hot brief**: a small, already-packed packet of existing lines. Jev ranks which sections stay in it, **before** anyone calls. Session start and “what are you working on” read a file that is already there.

Jev is not on the call path. Jev is on the kernel’s state path.

### 8.1 Where it lives

One object on the project, next to the other wakeable files. A name is enough for this page: the **voice brief**. It is not a second chat. It is not a Jev card. It is a projection, like the compact projection: the transcript stays whole; the brief is what a mouth is allowed to know.

The gateway already asks the kernel for place and status at `session.start`. It would also take the brief. The desktop strip and the phone can read the same object. One packer, three faces.

### 8.2 When Jev runs

On kernel events, not on `session.start`:

- root turn starts or ends
- a worker starts, changes step, or reports
- `notes.md` or `plan.md` is written
- an ask parks
- a tool ends in error

One Jev call per event batch (coalesce for a short window, the way we already coalesce worker frames). Not every grep.

### 8.3 What we send Jev

The **current brief sections** plus the **new candidate slices** from §4. Same glance rules.

```
brief_now:
  identity: (always pinned, not scored)
  tldr: "…"
  working: "run-tests · cargo test"
  last: "All thirty tests pass."
candidates:
  … new worker say, new error line, new tldr …
```

Identity is pinned in code. Jev cannot drop the folder. That is the #492 lesson.

### 8.4 What Jev returns (and what Live actually gets)

| Question | Type | Meaning |
| --- | --- | --- |
| `keep_section` | Noul per section id | Stay in the brief? |
| `admit_candidate` | Noul per new slice | Replace a section, or skip? |
| `user_should_hear` | Noul | Should the *narrator* speak this change, or only refresh the brief? |

Code rebuilds the brief from winning **existing** lines. Hard cap so one inject always fits (start with 2,000 tokens; stay under the 500-token append if we send only a diff).

The gateway:

- At start: inject the brief as the workers/activity block we already send, plus the tldr. No Jev wait. The 1.5-second seed is a **read**, which we already do.
- Mid-call: inject when the brief **hash** changes, not every 3 seconds.
- On “what are you working on”: Live already has the tldr and the working line. It can speak without a delegation. If it still delegates, the kernel returns the same brief in spoken form. Either path is a read.

**What Live gets:** one standing packet of lines we already had. Not Jev’s voice. Not a new store.

`user_should_hear` is for the narrator, not for Live’s small talk. High noul + a worker `done` → the narrator speaks the last words, as it does today. Low noul → refresh the brief only. That is the silent-coordinator idea, aimed at the mouth, still not a second brain.

### 8.5 What gets faster

- Session start: no extra model. The seed is a file we already wanted to send.
- “What are you working on”: Live answers from the brief. No “let me get that information” wait.
- Mid-call: fewer, denser appends. Live compact loses less, because the brief is small and we can re-inject the whole thing after compact.
- Desktop and phone: the strip and the orb can show the same `working` line without each growing a packer.

### 8.6 What gets slower or worse

- The brief can be **stale** between events. A tool that runs for minutes needs the existing activity heartbeat (every 5 seconds, already on the wire) so “still running cargo test” stays true. Jev does not need to re-rank a heartbeat.
- The first event after `jev = false` was on must rebuild from the age-based rules. One cold start.
- If we let the brief grow “just one more slice,” we recreate Architecture 1’s overflow. The cap is the design. A slice that does not fit is a pointer (Architecture 2), not a second page.

### 8.7 Fall-through

Jev down → keep the last good brief, or if none exists, today’s identity + last 40 lines. Never block `session.start` on Jev. If the brief read fails, that is **unknown**, not empty: inject identity only, and say we do not have the page. Do not pretend the project is idle. (Empty and unknown are different; we have paid for that lesson.)

---

## 9. Shapes we will not treat as architectures

These came up while exhausting the space. They are rejected, with a reason.

| Shape | Why it is not a candidate |
| --- | --- |
| Situation-card router, more tools | Already designed. Not a new shape. |
| Jev classifies small talk vs work | Second brain on the call. Live already does this. |
| Jev writes the spoken highlight | Jev cannot write. The narrator already clips two sentences. |
| Jev in the phone or desktop | Those clients must not hold the OpenRouter key. |
| Jev hears audio | Jev is text only. |
| LLM writes a voice summary at start | That is the slow thing we are trying not to do. 1.6 seconds is already the seed. An LLM summary would add seconds and invent wording. |
| Gateway-hosted Jev as a product | We will not train or host Jev. A key on the gateway is still “Jev on the call path.” |

---

## 10. Side by side

|  | **1. Inject packer** | **2. Delegation index** | **3. Standing brief** |
| --- | --- | --- | --- |
| Job | Filter what we already inject | Point Live at the right read | Keep a hot packet off the call path |
| When Jev runs | Every inject (start + 3 s drip) | Roster / notes / plan change | Same events as 2, plus turn end and tool error |
| Where Jev runs | Gateway or kernel-on-the-path | Kernel | Kernel |
| What we send Jev | Slices with clipped bodies | Addresses + one-line labels | Current brief + new slices |
| What Jev returns | keep / channel / enough | pin set + first delegate | keep section / admit / user_should_hear |
| What Live receives | Packed existing text on today’s inject kinds | A short INDEX in instructions | The brief, as today’s workers/activity block |
| Wait on `session.start` | Jev (risk: miss the 1.5 s seed) | Small | None (read a file) |
| “What are you working on” | Fast *if* the tldr made the last pack | Fast only if the index holds the sentence; else a read hop | Fast: the sentence is already in Live |
| Mid-call traffic | Still frequent, just slimmer | Rare (index change) | Rare (brief hash change) |
| Live compact | Re-pack, another Jev call | Re-send the small index | Re-send the whole brief; it fits |
| Second brain on the call? | Yes, on the inject path | No | No |
| Helps desktop / phone? | No, unless they listen to injects | Only if they show the index | Yes. Same object |
| Failure | Today’s dump | Today’s identity + workers | Last brief, or identity + unknown |
| Main risk | Slow start; Score bill; key on gateway | Silence when the pointer has no sentence | Stale brief; brief bloat |

The three disagree about **where the truth Live needs should sit.**

1. In Live, freshly packed.  
2. In the kernel, behind a pointer.  
3. In a small projection the kernel already maintains, which Live merely holds a copy of.

---

## 11. Recommendation

**Build Architecture 3. Use Architecture 2 as the overflow rule inside the brief. Do not put Architecture 1 on the call path.**

One idea at a time:

**Keep identity in code.** Folder, machine, `arbos://`, via. Jev cannot drop these. That bug is closed.

**Keep a standing brief on the kernel.** Sections: tldr, who is working, last result or last error, open ask, branch name. Each section is a glance we already have. Cap the whole brief so one inject always fits.

**Ask Jev only when those sections might change.** Noul keep/admit. Fall through to “keep the last brief.” Never ask Jev at `session.start`.

**If a slice is true but does not fit, store it as a pointer** (Architecture 2). Live may delegate a `tail` or a `read` of that address. The narrator already has those frames. Do not grow the brief.

**Do not call Jev from the gateway.** The gateway reads the brief the way it already reads place. One key, one off switch, one process that already talks to Jev.

**Do not start slices A–G.** This page does not need the Decisions rewrite to be true: even today’s JSON door can answer keep/drop on ids. When A lands, these Nouls become a fan-out. Until then, the architecture is still 3.

**What Jacob should feel:** he starts a call; Live already knows the folder, the tldr, and who is running. He asks what we are working on; he hears the tldr, not a stall. A worker finishes; the brief changes; the narrator may speak the last words; Live’s copy updates. The orb does not grow a Jev face.

---

## 12. Done when (later; not this turn)

This turn only writes the comparison.

A later implementation is done when:

- `session.start` injects the brief without a Jev call.
- “What are you working on” is answerable from the brief alone on a project that has a tldr and a live worker.
- A Jev timeout leaves the last brief in place and the call up.
- `jev = false` is today’s inject.
- No vault keys, no file bodies, in the brief or in the voice card.
- Live still speaks. Jev still does not.
- `v0.2.0` is not published from that work.

---

## 13. Related

- [Jev in the harness](jev-harness.md) — what #546 shipped
- [Jev, fully in the agent and the app](jev-full-integration.md) — tool router; voice wall in §16
- [What GPT Live can see](gpt-live-context.md) — three stores, inject caps, the 1.6 s seed
- [Desktop call mode](desktop-call-mode-design.md) — narrator, highlights, `more_detail`
- [Work sound](call-mode-work-sound.md) — sound follows kernel activity, not a Jev timer
