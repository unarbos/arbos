# Jev, fully in the agent and the app

Jacob asked for a full design. This page is that design.

What already shipped: [Jev in the harness](jev-harness.md) ([#546](https://github.com/unarbos/arbos/pull/546)). Read that first. This page does not replace it. It says how to take the same split and put Jev on every mechanical choice Arbos still burns an LLM call for.

This page does not ship code. It does not open a PR. It does not publish `v0.2.0`.

---

## 1. The point

Arbos should feel instant when Jacob wakes it.

The slow part of an agent is not the tool. The slow part is asking a chat model “what next?” after every grep.

**Jev** is a cheap, fast model that answers a typed question. It does not write. It does not talk. It picks.

The **LLM** is the model in `config.toml` (`model`). It writes, plans, and talks.

Keep that split. Widen it. Do not add a second brain on the screen.

---

## 2. Words used here

Each term is defined the first time it matters. This list is the short map.

- **Kernel**: `arbos-kernel serve <folder>`. One process. One project folder. The agent loop lives here.
- **Turn**: one wake of the kernel. A user line, a worker report, or a subscription opens it. The kernel works, then ends.
- **LLM**: the chat model named in `config.toml`. It generates text and tool calls.
- **Jev**: TypeSafe’s System One model. OpenRouter slug `typesafe/jev-latest` (family alias `~typesafe/jev-latest`). About $0.042 per million input tokens. Output is free. Window: 32,000 tokens. It returns typed decisions, not prose.
- **System One**: TypeSafe’s name for models that decide, not chat. Kahneman’s “System 1”: fast judgment. The LLM is the slow, writing half.
- **State**: the short packet Jev reads. In Arbos this is the **situation card**.
- **Situation card**: goal, last user line, last few tool glances, files already touched. Never the full transcript. Never a vault key. Never a whole file.
- **Question primitive**: one typed ask. Three kinds exist (defined next).
- **Noul**: a yes/no question. Answer is a probability from 0 to 1 that “yes” is true.
- **Choice**: pick one option from a list you supply. Answer is the winner, a probability per option, and a **confidence** (how peaked that list is).
- **Score**: a place on an ordered scale you write (2 to 10 levels). Answer is a fractional score, probabilities, and confidence.
- **Confidence**: a number from 0 to 1 on Choice and Score. High means the options were not a close fight. It is not a promise the winner is right.
- **Fan-out**: many questions in one Jev call, all against the same state, answered in parallel.
- **Fall-through**: if Jev errors, times out, or returns junk, the LLM takes the step. The turn still finishes.
- **Mechanical move**: a tool step that does not need new language. Read, grep, glob, list, run a test, `git status`, pick a search hit, click a known control.
- **Menu**: a list of exact next moves the kernel already knows how to run. Each row has a tool name and filled arguments. Jev picks a row. It does not invent a row.
- **Router**: code that asks Jev, then runs the chosen row or hands the step to the LLM.
- **Live**: the voice call path (GPT Live today). The gateway talks to the voice model and to a kernel. Jev is not on this path.
- **Project store**: the shared folder every agent in a Project reads: `docs/`, `notes.md`, `internal/`, `media/`.
- **Worker**: a child agent the root spawns. It has its own kernel turn loop and its own transcript.

---

## 3. What Jev is, and is not

Jev is not a smaller chat model.

You send it a **state** and a list of **questions**. It sends back one answer per question. There is no prose to parse.

Official call, through OpenRouter ([Jev Latest](https://openrouter.ai/~typesafe/jev-latest)):

```
POST https://openrouter.ai/api/alpha/decisions
```

Body shape:

- `model`: `~typesafe/jev-latest`
- `state`: a string, object, or array of text
- `questions`: a map of Noul, Choice, and Score

TypeSafe’s own door is `POST /v1/systemone` with the same shape ([System One](https://docs.typesafe.ai/concepts/system-one.md)). Arbos does not need that door. Jacob already has an OpenRouter key. Use it.

Jev reads text only. No images. No audio. No video.

Jev cannot:

- write a reply
- write a file
- invent a search query
- invent a click target
- invent a spawn brief
- speak on a call

The rule from TypeSafe, restated for us: **Jev picks a card from the deck. It does not name a new card.** ([Flavio Copes, 2026-09-17](https://flaviocopes.com/jev/))

If a step needs a new sentence, a new query, or a new patch, that step is the LLM’s.

---

## 4. What #546 already shipped

The kernel harness asks Jev “what next?” on each mechanical step.

One JSON object:

```json
{
  "act": "tool" | "llm" | "done",
  "tool": "grep",
  "args": {},
  "why": "short"
}
```

- `tool` — run that tool. Do not call the LLM this step.
- `llm` — one normal `model_step`.
- `done` — end the turn. If nobody has spoken yet, one short LLM `say`.

Also shipped:

- Default on when the provider is OpenRouter and a key exists.
- `jev = false` is the old one-model loop.
- Empty `jev_model` turns Jev off.
- Situation card under 32k. Last three tool glances. Secrets redacted. File bodies omitted.
- First-byte timeout 15 seconds. Junk or error falls through to the LLM.
- `say`, `ask`, `plan`, and free-form `write` / `edit` stay on the LLM.
- The live line says “Choosing the next step”, not “Working jev”.
- Voice and Live stay on the LLM.
- Jev is not a fallback model. `fallback_models` is unchanged.
- Tests pin the three acts, junk fall-through, the 32k card, and `jev = false`.

Code: `crates/arbos-engine/src/jev.rs`, hooked from `turn.rs`. Config in `arbos-core` host.

#546 talks to Jev as if it were a tiny chat model that emits JSON. That works as a first cut. It throws away the part that makes Jev cheap and honest: **typed questions, probabilities, and many answers in one call**.

The rest of this page moves the harness onto those primitives. The three acts stay. The off switch stays. Fall-through stays.

---

## 5. How other people use Jev

These are public uses as of 2026-09-18. Each one is a pattern we can steal, not a product to copy.

### 5.1 Router in an agent loop

[LangChain, “Building a Harness with Jev” (2026-09-17)](https://www.langchain.com/blog/building-a-harness-with-jev)

- `TypeSafeClassifier`: state in, typed answers out.
- `ModelRouterMiddleware`: Jev picks a cheap model or a strong model for the run.
- `AutoModeMiddleware`: Jev scores a tool call for risk before it runs. Example: block or review `bash`.

LangChain also names field uses: Kyle Jeong (Browserbase) on browser agents, Jarrod Watts on a live trading agent, Ryan Vogel on email triage.

### 5.2 Next browser action

[Retriever AI / rtrvr (2026-09-16)](https://rtrvr.ai/blog/jev-browser-agent-benchmark)

- A planner model writes the goal.
- Code reads the page and builds a **menu** of exact actions (click this id, type this string).
- A large page first gets a **Score** pass per section. Only the useful section reaches Choice.
- Jev picks one menu row, or **delegate** (hand back to the planner).
- LinkedIn and Amazon runs finished 31% and 43% sooner. They also cost more, because scoring a huge page is its own bill.
- Choice calls were 165–367 ms.
- Jev never invented an element id.

### 5.3 Drop dead tool results

[`hermes-jev-compact` (Hermes Agent plugin, 2026-09-18)](https://pypi.org/project/hermes-jev-compact/)

- Built-in compressors drop old tool results by age.
- This plugin asks two Nouls per stale call: does the call still matter? does the full result still matter?
- Keep, truncate to a head, or drop.
- Any Jev failure falls back to the old prune.
- State is a shrunk transcript, not the raw dump.
- A pass that does not shrink the transcript by about 25% is thrown away.

State shaping comes from `tamara/fast-jev-compaction`.

### 5.4 Cheap labels, routes, and checks

[TypeSafe launch (2026-09-15)](https://typesafe.ai/blog/introducing-system-one-models-and-jev), [The Register (2026-09-16)](https://www.theregister.com/ai-and-ml/2026/09/16/typesafe-ai-debuts-model-for-machines-that-plays-doom/5296711), [Copes (2026-09-18)](https://flaviocopes.com/jev/)

Uses people actually ran:

- Label every row (papers, listings, resumes, inbox).
- Intent routing: which handler, which model.
- Verify an LLM claim against a source (Noul per claim).
- Re-rank search hits (Noul per query–passage pair).
- Guard a shell command: read-only, reversible, or irreversible.
- Pick a skill from a list the harness already has.
- Real-time UI: score a draft on each pause.
- Games: Doom on structured state; Wikiracing among hundreds of real links.
- Map-reduce over a corpus.

TypeSafe’s own numbers: 70–500 ms end to end, most calls near 100 ms, from the US West Coast. Headline “193.6× faster, 444.6× cheaper” is their workflow eval ceiling, not a promise.

### 5.5 What this is not

OpenRouter’s [`openrouter:web_search`](https://openrouter.ai/docs/guides/features/server-tools/web-search) is a **server tool**. It is not Jev. Jev does not search. Jev can choose *whether* to search and *which* hit to keep after the kernel’s `search` tool returns.

---

## 6. Uses we do not have yet

#546 covers one thing: pick `tool` / `llm` / `done` for the next kernel step.

We do not yet have:

1. **Decisions API** — Noul / Choice / Score with probabilities. Today we parse chat JSON.
2. **Fan-out** — many questions in one call. Today we ask one act.
3. **Confidence gate** — low confidence falls through to the LLM. Today any parsed JSON is trusted.
4. **Web search pick** — whether to search; which numbered hit to `fetch`; which hit is enough to cite.
5. **Browser pick** — next click / type / navigate from a snapshot menu; Score large pages first.
6. **Context compression** — keep or drop stale tool calls by meaning, not only by age.
7. **Cheap JSON steps** the LLM still does: “is this done?”, “did the test pass?”, “which grep hit?”, “which file from this glob?”, “no change?”.
8. **Skill pick** — load one existing skill, or none.
9. **Worker route** — spawn, steer an existing child, or do the quick read here. Jev picks. The LLM writes the brief.
10. **Silent coordinator close** — on a worker `[done]`, should the root speak, or only fold into `notes.md`?
11. **Claim check** — before a research `say`, Noul each citation against the fetched text.
12. **Risk class** — read-only / reversible / irreversible, then the existing hard-safety path. Not a new ask card.
13. **Model pick inside one turn** — cheap LLM vs the configured LLM for *this* writing step, from models already in config. Optional. Not a second personality.
14. **Store triage** — docs vs `internal/` vs skip; which store paths belong on the next card.

We will not take: email triage, trading, Doom, feed filters, resume screening, or any product that is not Arbos.

---

## 7. The design rule

One sentence:

**The kernel builds a menu. Jev picks a row. The LLM writes when the menu has no row.**

Four laws, unchanged from #546:

1. Jev is a router and a fast mechanical worker. It is not a second brain in the UI.
2. Voice and Live stay on the configured voice model / LLM. Jev does not speak.
3. `jev = false` is the off switch. The old loop must still work.
4. Junk, timeout, or a missing key: fall through to the LLM. The turn still finishes.

Three more, for the full integration:

5. No vault key, no `op://` reference, no secret tool body, ever, in a situation card.
6. Do not train Jev. Do not host Jev. Call it on OpenRouter with the key already on the host.
7. Jev never invents arguments that are language (a query, a patch, a spawn brief, a spoken line). If those are empty, fall through.

---

## 8. How one step works after this design

Same outer loop as #546. Richer ask.

1. Build the situation card. Same budget. Same redactions.
2. Build this step’s **menu** in code (section 9).
3. Ask Jev **one** Decisions call. Fan-out the questions for this step.
4. Read probabilities and confidence.
5. If the call fails, times out, or confidence is below the gate: **LLM**.
6. If the winner is a menu row: run that tool. No LLM this step. Loop.
7. If the winner is `llm`: one `model_step`. Loop.
8. If the winner is `done`: end. One short `say` if the window is blank.
9. Barge-in, cancel, and spend caps stay on the kernel. Jev cannot override them.

The chat JSON from #546 stays as a **fallback parser** if the Decisions door is down. Then the LLM. Two cushions, not zero.

---

## 9. Kernel harness

This is the core. Desktop, phone, and workers all see the result of this loop. They do not each grow a Jev client.

### 9.1 Two doors, one key

| Door | When | What we send |
| --- | --- | --- |
| Decisions API (`/api/alpha/decisions`) | Default | `state` + typed `questions` |
| Chat JSON (today’s `complete_stream`) | Decisions fails | The #546 system prompt and card |
| LLM `model_step` | Both fail, or Jev says `llm`, or confidence is low | Today’s turn |

Same OpenRouter key. Same `jev_model` (default `typesafe/jev-latest`). No TypeSafe account. No second env var.

If OpenRouter wants the family alias `~typesafe/jev-latest` on the Decisions door, keep that in one constant. `jev_model` in config still wins.

### 9.2 The situation card, still small

Keep #546’s card:

- `goal`
- `last_user`
- `first_step`, `spoke`
- `repro` when we have a recorded run
- `files_touched` (paths only)
- `tools` (names the agent may use)
- `last_tools` (three glances)

Add only what a later picker needs, and only as short lists:

- `menu`: the rows for this step (id, tool, one-line why)
- `hits`: numbered search results already in hand (title, url, 120-char snippet)
- `controls`: numbered browser actions already in hand (role, name, id)
- `children`: name + state of live workers (no transcripts)
- `skills`: name + one-line description of skills that already matched
- `stale_tools`: id + name + size of tool results that are compact candidates

Hard caps stay. If the card would exceed the 24k token budget, drop in this order: stale_tools bodies, control lists beyond the scored set, hit snippets, then glance size (already shrinking).

Never add:

- vault items or field values
- `secret` tool results
- file contents, diffs, patches
- whole `notes.md` or whole transcripts
- Live audio or call traces

The `secret` glance stays `(secret; not sent)`.

### 9.3 Fan-out questions every mechanical step

Ask these together. Code uses the ones that apply.

| Id | Type | Ask |
| --- | --- | --- |
| `act` | Choice | `tool` / `llm` / `done` / `other` |
| `tool_row` | Choice | menu row ids, plus `none` |
| `need_llm` | Noul | Does this step need new language? |
| `done_enough` | Noul | Does the evidence already satisfy the user ask? |
| `no_change` | Noul | After a passing `repro:true` run, is the tree already right? |
| `compact_now` | Noul | Should we drop stale tool results before the next LLM call? |
| `need_search` | Noul | Do we need the web for this step? |
| `need_browser` | Noul | Do we need a page action for this step? |
| `spawn_now` | Noul | Should a child start (root only)? |
| `risk` | Score | `read-only` / `reversible` / `irreversible` for the winning row |

`other` on `act` is the exit. No `other` forces a bad pick. That is TypeSafe’s own rule.

A first open-ended ask with no tools this turn still forces `llm` in **code**, not as a hope. #546 already says this. Keep it.

### 9.4 Confidence gate

Defaults, visible in one file next to the questions:

- `act` confidence &lt; 0.5 → LLM
- `tool_row` confidence &lt; 0.5 → LLM
- `done_enough` noul &lt; 0.7 → do not end the turn
- `no_change` noul &lt; 0.8 → do not mark `no_change`
- `risk` at or above `irreversible` → existing hard-safety path, not a new dialog

These numbers are starting gates. Pin them with labeled fixtures before we trust them. A threshold is a claim.

### 9.5 What Jev may run without the LLM

Same family as #546, plus picks from a menu the kernel built:

- `read`, `grep`, `find`, `ls`
- `bash` for tests, `git status`, `git diff`, `git log` — not for commit messages or new scripts
- `jobs`, `await`
- `fetch` of a URL already on the menu
- `search` only when the query is already on the menu (section 10)
- `browser` only for a menu row (section 11)
- `changes`, `status`, `agents`, `transcript` (bounded tail)
- `spawn` only as “yes, spawn” — the brief is the LLM’s

Still LLM-only: `say`, `ask`, `plan`, `write`, `edit`, `apply_patch` (unless a one-line mechanical fix with a path already on the card), `remember`, `pr` body text, anything the user will read.

### 9.6 Time and spend

- First byte: 15 seconds (already shipped). Then fall through.
- Decisions calls are short. Cap wait at the same 15 seconds.
- Jev usage folds into the turn’s existing cost. Do not add a second meter the user has to watch.
- Spend caps and barge-in stay on the kernel.

---

## 10. Web search

Jacob’s seed: Jev can search the web fast.

Jev does not call the web. The kernel already has `search` and `fetch` (`arbos-engine` `tools/web.rs`). Backends today: custom `search_url`, Exa, Brave, Tavily, OpenRouter’s web plugin, DuckDuckGo HTML.

### 10.1 The split

| Who | Does |
| --- | --- |
| Code | Builds candidate queries from the user line and the goal. Runs `search` when Jev picks a query row. |
| Jev | `need_search`. Then Choice among candidate queries. Then Choice among numbered hits (`fetch` this one / enough / none). |
| LLM | Writes a query Jev could not pick. Writes the cited reply. |

### 10.2 Candidate queries, without inventing text

Jev cannot write `query`. So the menu must already hold queries.

Build them in code, in this order:

1. The user’s own words if they look like a search (`search for X`, a quoted phrase, a named paper or error).
2. The goal line, clipped.
3. The last failed `search` query, if we are retrying, marked as a retry so Jev can pick `none`.

If the list is empty and `need_search` is high: **LLM** writes one query, then we search. That is fall-through, not a new brain.

### 10.3 After hits land

Put `[n] title — url — snippet` on the next card (cap 8, snippet 120 chars — the tool already caps).

Fan-out:

- one Noul per hit: “Does this source help answer the goal?”
- one Choice: `fetch n` / `enough` / `none`

`enough` means the snippets already support a cited answer. The **LLM** writes that answer. Jev does not.

Citation rule stays the protocol rule: a URL the model did not see in a tool result is not a source. Jev cannot invent one.

### 10.4 What we will not do

- Do not give Jev the OpenRouter `web_search` server tool as if it were a chat model.
- Do not send page HTML into the card. `fetch` already returns text. Glance it.
- Do not search from the phone or the desktop. The kernel searches. Clients show the same `search` tool line they show today.

---

## 11. Browser

Jacob’s seed: Jev can do browser use fast.

Same split as rtrvr. We already have `browser action:navigate|click|type|screenshot|snapshot|close`.

### 11.1 The split

| Who | Does |
| --- | --- |
| LLM | Sets the page goal. Writes text to type when the string is new. Repairs a dead plan. |
| Kernel | Takes a snapshot. Builds a menu of real controls. Runs the chosen action. |
| Jev | Scores sections on a large snapshot. Picks one menu row, or `delegate`. |

### 11.2 Menu rows

After `snapshot`, each interactive control becomes one row:

- `click #<id>`
- `type #<id>` only when the string is already known (user said the words, or a prior LLM step wrote them)
- `navigate <url>` only when the url is already on the card
- `screenshot`
- `close`
- `delegate`

Jev never invents `#id`. If the snapshot is stale, take another snapshot first (code), then ask.

### 11.3 Large pages

A snapshot with hundreds of controls is too big for one Choice (max 255 options; we should stay far under that).

rtrvr’s move, which we copy:

1. Split the snapshot by subtree (a form, a result card, a nav). Do not split a product name from its button.
2. One parallel Score pass: “How useful is this section for the current goal?”
3. Keep the top sections. Rebuild a short menu.
4. One Choice.

Skip the Score pass when the menu already fits (for example ≤ 40 rows). rtrvr paid most of its Jev bill on Score. Do not score a page we will hand to the LLM in full anyway.

### 11.4 After a click

Read the new snapshot. Ask again.

A high-confidence click can still do nothing on the page. rtrvr saw this on LinkedIn Send. Always check the new state. Do not treat confidence as “the site accepted it.”

### 11.5 Images

Jev cannot see a screenshot. The **LLM** reads pixels. Jev may pick `screenshot` as a row so the LLM can look next. That is the whole image path.

---

## 12. Compress the window

Jacob’s seed: Jev can shrink the context window fast. Drop tool calls that no longer help.

We already fold and compact in `arbos-engine/src/compact.rs`:

1. **Fold** — old tool bodies become one cite line. No model.
2. **Compact** — oldest turns become a checkpoint. Today a (possibly cheaper) **LLM** writes the summary.

Jev sits **between** those two. It does not replace the summariser. It chooses what the summariser never has to see.

### 12.1 The Hermes move, in our files

For each stale tool call/result pair (not the protected tail, not the current step):

- Noul: does this call still matter to the goal?
- Noul: does the **full** result still matter?

Then code:

- keep both
- keep the call, truncate the result to a short head + cite (we already have cites)
- drop the result body (the transcript on disk stays; the **projection** shrinks)

This is a projection change. `transcript.jsonl` remains the full record. Grep still finds the body. Same compact contract as today.

### 12.2 Fall back

If Jev fails, times out, or the pass does not free a useful amount (start at 512 tokens — our `MIN_FREED` — and require a real shrink): run today’s fold.

Never compact by asking Jev to write the checkpoint. Writing is the LLM (or the existing cheaper summariser model). Jev only points at dead weight.

### 12.3 `#546`’s `compact` / `fold` flags

Those yes/no fields are parsed today. They must become real gates into this path, or be deleted in the Decisions move so we do not carry a dead flag. Prefer the Nouls in §9.3. One way to say “compact now.”

---

## 13. Cheap JSON steps

These are steps where we already have the candidates in code. Today an LLM call exists only to pick.

| Step | Menu already in hand | Jev asks |
| --- | --- | --- |
| Which grep hit to open | The grep list | Choice of paths, or `none` |
| Which glob file | The find list | Choice of paths |
| Did the test run pass | The last `bash` glance | Noul |
| Is the user ask covered | Goal + last tools + files touched | `done_enough` |
| `no change` | A passing repro + empty diff | `no_change` |
| Which skill | Skills that already matched by description | Choice, plus `none` |
| Which worker to steer | Live children | Choice, plus `spawn`, plus `here` |
| Speak or stay quiet (root, event turn) | The done notice + open user ask | Noul `user_should_hear` |

Each of these is one fan-out on the card we already send. Do not add a second Jev round trip unless the first answer must fetch more state (TypeSafe’s rule: a second call only when the first unlocks new options).

---

## 14. Desktop

The desktop is a view onto the kernel. It does not call Jev.

### 14.1 What the window shows

Project chat (root) already hides tool cards. A Jev-picked grep is one more hidden explore line.

Classic agent chat (worker) already shows tool cards. A Jev-picked grep looks like any other Grep card.

Do **not** add:

- a Jev pane
- a Jev avatar
- a Jev transcript
- “Jev decided …” in prose
- a second shimmer that means “the router is thinking” as if it were a person

Do show:

- the same live step we show today (“Choosing the next step”, then “Reading foo.rs”)
- `jev` on the model name of **that step only**, if we show a model name, so a stall is honest
- Jev tokens in the existing turn cost, as a small part of the same number, not a new row the user has to learn

### 14.2 Settings

One control, already implied by #546:

- **Use Jev** — maps to `jev` in `config.toml`. Default on for OpenRouter with a key.
- Off = `jev = false` = today’s one-model loop.

Do not put a Jev model picker on the home screen. Power users can set `jev_model`. Empty string is off.

### 14.3 Cursor feel

Parity still compares Arbos chat to Cursor chat. Jev must not create a new kind of card the parity suite has to invent. If a Jev step cannot draw as an existing tool line, it is the wrong step.

---

## 15. iPhone

The phone is the same view, smaller.

- Text chat: same rules as the desktop. No Jev chrome.
- Project icons, composer, attachments: unchanged.
- The call screen (orb, pull-down composer) is Live. See §16.
- A tool the kernel runs while Jacob is on a call may update the chat the way it does today. That update is a kernel event, not a Jev event.

The phone never holds an OpenRouter key for Jev. The kernel on the project’s machine does.

---

## 16. Voice and Live

This is a hard wall.

**Jev does not hear. Jev does not speak. Jev does not sit in the gateway.**

Live (GPT Live today, or the open-source speech model later) stays the model of the call. It barges in. It small-talks. It decides when to delegate to the kernel.

When Live delegates, the **kernel** turn may use Jev the ordinary way. Jacob hears the LLM / narrator, never Jev.

Do not use Jev to classify “is this small talk?” on the call path. That is Live’s job. Putting Jev there would make a second brain in the one place Jacob asked to feel like one phone call.

The work-sound while tools run ([call-mode-work-sound](call-mode-work-sound.md)) is driven by **kernel running state**, not by a Jev timer.

---

## 17. Project store

The store is files. Jev does not write them.

The root still owns `docs/project-context.md` and `notes.md`. Workers still write the paths in their brief.

Jev may only **point**:

| Moment | Jev | Then code / LLM |
| --- | --- | --- |
| Worker `[done]` | Noul: does the user need a line? | Root `say` or silent notes fold |
| New deliverable path | Choice: `docs/` / `internal/` / `media/` / `other` | Only as a check against the brief; the brief still wins |
| Next card | Score store paths we already listed (names, not bodies) | Include the top few paths as names on the card |
| Mirror / loss | nothing | Existing mirror script. Jev does not decide restore. |

A spawn brief still hands **addresses** (`arbos://…`), not inlined store text. Jev must not paste store bodies into a card to “help.”

---

## 18. Workers

Every worker kernel uses the same harness. There is not a special “worker Jev.”

### 18.1 Root: spawn or not

Root’s extra questions (§9.3 `spawn_now`, §13 worker Choice):

- `here` — one quick read-only probe on the root (allowed)
- `steer <name>` — existing child
- `spawn` — new child
- `wait` — the event turn can end; a `[done]` will wake us

The **LLM** writes `name`, `task`, `do`, `rules`, `output`. Jev only said “spawn.”

If Jev says `spawn` and the LLM has not written a brief yet, that is an `llm` step, not a blind `spawn` with an empty task.

### 18.2 Mesh

A remote child is a kernel on another machine. Jev runs **there**, with that machine’s OpenRouter key, or falls through.

Do not send vault keys across the hub in a situation card. A spawn brief already carries addresses, not secrets ([mesh design](arbos-mesh-design.md) part 3). Keep it.

If the peer is unreachable, fail loudly. Do not ask Jev to “guess from cache.” There is no cache.

### 18.3 Inline helpers

Explore / computer-use helpers stay tools. Jev may pick `explore` from a menu. It may not drive the helper’s inner loop unless that helper is itself a kernel with Jev on.

---

## 19. Config and the off switch

All optional. Same file as today: `config.toml`.

| Key | Default | Meaning |
| --- | --- | --- |
| `jev` | `true` when provider is OpenRouter and a key exists | Master switch |
| `jev_model` | `typesafe/jev-latest` | Slug. Empty string = off |
| `jev_confidence` | `0.5` | Below this, fall through on Choice |
| `jev_compact` | `true` when `jev` is on | Keep/drop stale tools |
| `jev_search` | `true` when `jev` is on | Query/hit pick |
| `jev_browser` | `true` when `jev` is on | Snapshot menu pick |

`jev = false` turns **all** of the above off. That is the only off switch a person needs. The extra keys are for a bisect, not for a settings page.

Replay, tests, and a missing key never ask Jev. #546 already pins this.

---

## 20. Safety

Jev is not a new permission mode. Default stays **full auto**.

What Jev may do for safety:

- Score a bash row as irreversible.
- Hand that row to the **existing** guard (root/home/system tree refuse; protected-branch refuse).

What Jev may not do:

- Invent an ask card in `auto` mode.
- Override barge-in, cancel, or spend caps.
- See vault values.
- Soften a hard refusal.

LangChain’s Auto Mode is a classifier in front of a tool. We already have a classifier in code for the worst commands. Jev is a second cheap look for the ambiguous ones. Code still wins.

---

## 21. What we will not do

- Train Jev, fine-tune Jev, or host Jev on ArbosLife.
- Put a Jev card, pane, or voice in the desktop or the phone.
- Let Jev speak on Live or dictate on the composer.
- Send vault keys, whole files, or diffs in a situation card.
- Replace `fallback_models` with Jev.
- Block a turn on a human choice about the split.
- Publish `v0.2.0` as part of this work.
- Use Jev to write `notes.md`, PR bodies, or research docs.
- Ask Jev to count, do date math, or compare hex colors (TypeSafe jaggedness). Do that in code.
- Chain Choices over characters to fake text generation.

---

## 22. Build order

Each slice is one PR-shaped change. Each slice keeps `jev = false` green. Do not implement them in this document’s turn.

| Slice | What lands | Done when |
| --- | --- | --- |
| A | Decisions client + fan-out parse. Chat JSON remains the cushion. | Tests: noul/choice/score parse; door fail → chat JSON → LLM |
| B | Confidence gate on `act` / `tool_row` | Low confidence fixtures fall through |
| C | Compact: keep/drop stale tools into the existing fold | A dead test log leaves the projection; a needed failure stays; Jev down → old fold |
| D | Search: candidate queries + hit pick | A research turn fetches a hit Jev picked; empty query list → LLM |
| E | Browser: snapshot menu + optional Score | A click row runs; stale id falls through; screenshot still goes to the LLM |
| F | Root event: `user_should_hear` + spawn/steer/here | A worker done with no user ask stays out of chat |
| G | Skill pick + citation Nouls | Optional; only after A–E are boring |

Do not start G until C, D, and E have real timings on an OpenRouter key. rtrvr’s lesson: Score can erase the dollar win. Measure.

---

## 23. How this makes the agent and the app feel finished

The acceptance benchmark is this Project’s own kickoff: a stream of goals, a master file, research, design docs, parallel workers, steer, vault use without leaks, a live status page, risky asks held once.

Jev does not do those jobs. It makes the **wait between them** short.

- Root: “is this a spawn or a `ls`?” in ~100 ms, then the LLM writes the brief only when needed.
- Worker: grep → read → test without a 3-second chat model between each.
- Research: search and fetch loop on the menu; the LLM writes the linked doc once.
- Long run: dead tool results leave the window so the LLM still sees the failure that matters.
- Desktop and phone: the same Cursor-like chat, faster under the fold. No new character in the room.
- Call: Jacob talks to Arbos. The kernel may use Jev behind the narrator. The orb does not change.

That is “perfectly designed” here: one voice, one window, one off switch, and a router that stays in the kernel.

---

## 24. Done when

- A turn with an OpenRouter key uses the Decisions door for mechanical picks, search hits, browser rows, and stale-tool drops.
- The configured LLM still writes, plans, talks, and speaks.
- `jev = false` is the #546 / pre-Jev loop.
- Junk, timeout, low confidence, or a missing menu argument falls through. The turn finishes.
- Situation cards still refuse secrets and whole files.
- Desktop and iPhone show no Jev brain.
- Live and voice never call Jev.
- Tests pin: primitives parse; fall-through; card ≤ 32k; compact fallback; search with no candidate query; browser `delegate`; `jev = false`.
- No `v0.2.0` publish from this work.

---

## 25. Sources

Primary:

- [TypeSafe: Jev Latest on OpenRouter](https://openrouter.ai/~typesafe/jev-latest) — slug, price, 32k window, Decisions API example
- [TypeSafe models on OpenRouter](https://openrouter.ai/typesafe)
- [OpenRouter latest-resolution aliases](https://openrouter.ai/docs/guides/routing/routers/latest-resolution) — `~author/family-latest`
- [TypeSafe introduction](https://docs.typesafe.ai/introduction.md)
- [System One](https://docs.typesafe.ai/concepts/system-one.md)
- [Confidence](https://docs.typesafe.ai/confidence.md)
- [Patterns](https://docs.typesafe.ai/patterns.md) — fan-out, confidence gating, composite scoring, intent routing
- [Introducing System One and Jev](https://typesafe.ai/blog/introducing-system-one-models-and-jev) (2026-09-15)
- [LangChain: Building a Harness with Jev](https://www.langchain.com/blog/building-a-harness-with-jev) (2026-09-17)
- [LangChain / RuntimeWire write-up](https://runtimewire.com/article/langchain-adds-jev-decision-model-agent-workflows)
- [rtrvr: How Jev Chooses the Next Browser Action](https://rtrvr.ai/blog/jev-browser-agent-benchmark) (2026-09-16)
- [hermes-jev-compact](https://pypi.org/project/hermes-jev-compact/)
- [Copes: A deep dive into Jev](https://flaviocopes.com/jev/) (2026-09-17)
- [The Register on Jev](https://www.theregister.com/ai-and-ml/2026/09/16/typesafe-ai-debuts-model-for-machines-that-plays-doom/5296711)
- [OpenRouter web search server tool](https://openrouter.ai/docs/guides/features/server-tools/web-search) — not Jev; listed so we do not confuse the two

Arbos:

- [Jev in the harness](jev-harness.md) — what #546 shipped
- [PR #546](https://github.com/unarbos/arbos/pull/546)
- [Project context](project-context.md)
- [Project chat vs agent chat](project-chat-vs-agent-chat.md)
- [GPT Live context](gpt-live-context.md)
- [Mesh design](arbos-mesh-design.md)
