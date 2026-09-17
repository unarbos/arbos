# Side panels: terminals, browsers, documents and files

Design, 2026-09-17. For Jacob. **Decided and being built**: the shape is on [#445](https://github.com/unarbos/arbos/pull/445); the four panel kinds follow it.

Terms are defined the first time they appear. Everything here is checked against the code in `unarbos/arbos`, and each thing that does not exist yet is listed at the end as a kernel job.

**What is settled, 2026-09-17.** He read this, chose it, and added the tabs: *"I want to go for this. Note we should be able to tab between these items where there are more than one. Keep the driver chat on the left, the side panel has its own tabs like you suggest, allow command shift {} between them + command t for new ones. Let's just copy exactly how Cursor is doing this with the side panel."* So: chat on the left, one drawer on the right with its own tab row, `⌘⇧{` and `⌘⇧}` between its tabs, `⌘T` for a new one, default closed, everything open listed in it. The co-editing fork at the end is still open; nothing else in §4 is.

---

## 1. What you asked for

Three things you said, in your words:

> "terminals and browsers and documents and files should be opened as side panels which you can organize on a grid on the screen."

> "open the terminal, open the browser, open this file and let me edit — but easy navigation back to the main chat."

> The side panel is default closed. "Open the terminal", "open the browser", "open the project" each open that thing in the side panel. Anything being worked with lives there and is listed there while it is open. With the panel closed you do not have to look at any of it; opening it shows what is there.

> "Keep the driver chat on the left, the side panel has its own tabs like you suggest, allow command shift {} between them + command t for new ones. Let's just copy exactly how Cursor is doing this with the side panel."

And your question, **does the panel show a list of open things that you click through to fill the panel?**, is answered in §4: no menu to pass through — the tab row *is* the list.

*Provenance note, for us and not for you: these three came through the coordinator. They are not in the feedback ledger yet — its rows 2026-09-17-11 through -19 are still marked unread, and no captured report mentions a panel. The poller owner should file them so the words are on the record.*

---

## What it looks like

Stills from the running app, in `media/desktop/side-panel/`. The two that show the argument: [07](../media/desktop/side-panel/07-job-tab-live-output.png) is a real job's live output in the drawer while the chat only says *Working*, and [06](../media/desktop/side-panel/06-agent-opened-both-drawer-stays-shut.png) is the promise kept — the agent opened a terminal *and* a job, and the drawer stayed shut.

---

## 2. Why this is worth building

**The chat is what the agent says it is doing. The panel is what the kernel says is happening.** Those are two different things, and every serious bug of the last day was the gap between them.

Four from yesterday and today:

| What happened | What the model or the window claimed | What a panel would have shown |
| --- | --- | --- |
| Three sorting workers finished in 25 s; the coordinator then ran `sleep 75` to "wait" for them, so nothing came back for over a minute (your report -6) | The headline read *Waiting on three sorting workers* for minutes | Four rows: `root · bash sleep 75 · 40s`, and three `done 06:43:18`. The list would have been right while the sentence was wrong |
| A phantom **Delegate 1 · Working** row on your screen for two days (F-137) | A worker with a name, a parent and a status | Nothing. The kernel has no such delegate, and a row the kernel does not know is drawn as `gone`, never as `Working` |
| A job wrote 164 GB into a deleted file for 4.7 hours after the model killed what it thought was the job (#377) | The job was killed | One row still counting up, hours after the agent said it was done |
| A kernel served 14 September code for two and a half days while its own binary was current | The roster said current | The panel header naming the running process's own commit, not the file's |

Read together they are one fault: a person had no second, non-model channel to look at. The panel is that channel. It reads the kernel's record — job journals, the roster, the browser page, git — and never the model's account of itself.

**With three workers running, the list is the answer to "what is my agent actually doing."** You have asked that twice today and been answered wrongly twice. That alone pays for the first panel.

---

## 3. The frame

In Cursor a panel is a tool the person drives. Here the agent does the work and you watch, steer and take over. So:

**A panel is a window onto what the agent is doing, and only secondarily a tool you use.**

Two consequences that shape everything below:

1. Every panel has an owner on its face — the agent, or you. A terminal panel shows the job the agent is running, with its command and its live output. When you take it over, it must say plainly that it is now yours.
2. The way to make that label impossible to get wrong is not careful wording. It is to never let one object have two owners. See §5.1.

---

## 4. The shape: one drawer, default closed

**Decided, and built on [#445](https://github.com/unarbos/arbos/pull/445).** One drawer on the right — the surface that already shows `.arbos/`. Default closed. Remembered width, per project. It holds every open thing as a **tab in its own tab row**, with the content filling the rest. Not a free grid.

**Why not the grid you asked for.** Five reasons, and I would rather argue this now than build it twice:

- The chat is this app's centre of gravity. A grid makes the conversation one cell among several.
- A turn runs for minutes to hours. Panels are glanced at, not inhabited. Arranging a grid is work you did not ask for, done for a layout you will look at for eight seconds.
- Cursor's Projects release removed chrome rather than adding it. We are copying that shape deliberately.
- A grid multiplies "where does my typing go" by the number of cells. We have already shipped that bug twice (§6.1).
- The one real need behind the grid — *let me really work in this thing* — is answered by zoom, one key, no arranging.

Where your grid instinct does land: **Show everything**, a grid of cards inside the drawer, for when many things are open. A chooser, not a workspace.

### Your question: a list you click through, or not?

**Not a menu you pass through.** A list-then-fill costs a click every time and loses your place when you come back. So the list *is* the switcher, and it is always visible — which is what you settled on as the tab row:

- **A tab row across the top of the drawer**, drawn like the window's own strip: `+` on the left, equal pills after it, the drawer's close on the right. The tab in front is filled; the rest are flat and light on hover. Each tab carries its state as a word (§6.6).
- **`⌘T`** opens a new tab, **`⌘⇧{`** and **`⌘⇧}`** step along the row, wrapping at either end as the window's project tabs already do with the same chords.
- **The first tab is always Project** — the `.arbos/` view — and it does not close. It is the floor the drawer can always fall back to, and what "open the project" opens.
- Opening the drawer lands on the tab that was in front when you left it, never on a menu.
- **Show everything**, a grid of cards for a row too long to scan, is the one deliberate list view. Not built yet.

**One pair of chords, two rows of tabs.** They follow the focus: the panel's tabs while the panel has it, the window's projects otherwise. The state is not invisible — the front tab of the *focused* row is filled **and lit**, the other row's front tab is filled and **muted**, both read from the same focus the chords read. So the lit row is the row that answers, before you press anything. Focus moves only on a click, `⌘B` or `⌘1`; no frame, no agent action and no tab opening moves it. This matters because a chord that does two things depending on hidden state is how "my message went to the wrong project" happens again.

### Getting back: three ways, always the same three

- **Escape** — closes the drawer (and leaves zoom first, once zoom exists).
- **⌘1** — the chat, whatever is open, and it takes the focus off the panel, so the next `⌘T` is a project tab again.
- **A visible control** at the right of the tab row, always drawn.

A keyboard route alone is not enough: you opened the Project page and told us you could not close it. That failure is why the visible control is a requirement, not a nicety. Escape needed one line of its own to work at all — an action reaches a handler only through the focused element's own ancestors, and the drawer's focus is not under the window's context, so without binding `escape` there too it did nothing.

### Zoom

**⌘\\** makes the panel in front take the window. The same key gives the chat back, and so does Escape. This is the answer to "let me really work in this" without a grid. The call strip and the composer are never covered (§6.4).

### The rest of the frame

- **The `.arbos/` view stays**, as the tab row's permanent first tab: agents and their workers nested, the processes they started, the resources, the project page. **⌘2** still reaches it, and "open the project" opens it here. This is a change to the layout decision of 2026-09-13, which had that panel always present — it becomes the drawer's first tab, and the drawer is closed by default. Flagging it because it is your standing decision to revise.
- **⌘B** keeps toggling the drawer.
- **Per project tab.** Drawer state — open or closed, width, rows, which row is in front — belongs to the project, and switches with the tab (§6.5).
- **The chat is never squeezed below its reading measure.** On a window too narrow for both, the drawer overlays the chat rather than crushing it.

### The panel never opens itself

Your "if it is closed I do not need to look at these things" is a promise the app keeps literally.

- A thing the agent opens while the drawer is closed appears as a card in the chat — *Terminal · j3 · `cargo test`* — and as a row in the rail when you next open it. The card is the reopen handle.
- Nothing may move focus or open the drawer except you. Not a frame arriving, not a job starting, not a worker finishing.
- The drawer's handle carries a quiet count of **live** things only. A number, not a badge, and never a colour that asks to be cleared.

**The exception, recommended against.** An approval, or a conflict on a file you have unsaved edits in, both stay in the chat. Two reasons: your eyes are already there, and an interface that can steal the screen can steal a keystroke — which is the exact class of bug we have shipped twice. The conflict card in the chat names the file and offers the choice; it does not open a panel over your typing.

### Opening is something you ask for

"Open the terminal" is a sentence you say or type; the agent opens it; and the chat keeps a card — *Terminal · j3* — which is the handle that brings it to the front. That makes the conversation the index of everything you have open, which is native to this app rather than borrowed from an editor.

The rule is **the drawer opens when you ask, however you ask; it never opens because the agent did something** — and today the window can only keep half of it. A `Board` frame from the kernel says a terminal was opened; nothing in it says whether you asked for it in prose or the agent needed it for itself. So the shipped behaviour is the honest one: the window names the route at every call (`OpenedBy::User` for its own clicks, `OpenedBy::Agent` for a frame) and never guesses, which costs one click on a spoken "open the terminal". Kernel handover 2 closes that gap; until it lands, `⌘T` and the Terminal card are the routes that front a tab directly.

**The empty tab.** `⌘T` lands on four cards — Project, Terminal, Browser, File. Project and File act now; Terminal and Browser are the kernel's to open, say so, and put the request in the composer for you to send. No card does nothing.

---

## 5. The four panels

### 5.1 Terminal

Two different objects, and never one row for both. This is the whole answer to "whose is it".

**A job — the agent's.**
- *Shows:* the command, the agent that ran it, live output, elapsed time, and the exit code when it ends.
- *Data today:* `agents/<id>/jobs/<jN>/out.log`, streamed as `Frame::Job { agent, id, delta, running, exit }`. The desktop already models this as `SurfaceKind::Process`.
- *Who caused it:* the agent, named on the face.
- *You can:* read, copy, scroll, **Stop it** — routed to the kernel's own kill, which ends the whole process group (#377, after the 164 GB incident) — and **Open a shell here**.
- It is a follower, not a shell. A job has no terminal behind it, so there is nothing to type into. Saying otherwise would be the lie.

**A shell — yours.**
- *Shows:* your own `$SHELL`, interactive and login, in a directory.
- *Data today:* the kernel's `PtyHub` mints `t1`, `t2`, … and announces them with `Frame::Board { panel: "terminal", terminal_ids, cwd }`; `desktop/src/view/terminal.rs` already attaches and types.
- *Who caused it:* you. It is yours from birth.

**Whose shell, today.** The kernel's `terminal` tool opens one pty page and both sides can write to it, so a page the agent opened is labelled `agent's` rather than `yours`: typing there types into the agent's shell. That is the honest label for what exists, not the design's goal — a page of your own needs handover 2. A label that is wrong from birth is worse than no label, and this one shipped wrong for an hour until a still of a live terminal showed it.

**So "take over" mints a new object rather than changing an owner.** *Open a shell here* starts your shell in the job's directory, beside the job's row. No row ever changes hands, so the ownership label cannot go stale — the same reasoning as an exhaustive match with no wildcard arm: make the wrong state unrepresentable instead of labelling it correctly by hand.

### 5.2 Browser

- *Shows:* the page the agent is on — URL, title, the current picture, and its last action (*clicked "Sign in"*, *typed into the search box*).
- *Data today:* `BrowserHub` runs one Chromium per kernel over CDP, one page per agent (`b1`), with `click`, `type`, `press`, `back`, `screenshot` and a text snapshot already implemented; `Frame::Browser` carries the URL and a preview to the window.
- *Who caused it:* the agent. Every action is attributable, and the row reads `agent` until you take it.
- *You can:* watch, and **take it over** — the panel forwards your clicks and keystrokes as CDP input to the same page. The header then reads *yours*, and the agent's `browser` tool on that page is refused with a sentence saying you are driving. One driver at a time, named on the face; the refusal is loud, never a queue that silently reorders your clicks.
- *Needs from the kernel:* a screencast, so it is live rather than one frame per action, and an input frame from the client. See §7.

### 5.3 Documents

- *Shows:* a document, at reading size, in the same column measure the transcript uses.
- *Data today:* `store_view.rs` already reads `.arbos/` — `notes.md`, `docs/project-context.md`, the store's files — and `project_page.rs` already renders them. `article.rs` already carries a real markdown editor for `.arbos/desktop/articles/`.
- *Who caused it:* whoever wrote it last, named with a time. A document the agent wrote this turn says so.
- *You can:* read; and for a file, **edit it**.

**Editing is the hard one, because it is co-editing with a working agent.** The rules, in order:

1. On open, remember the file's bytes by hash.
2. A save is a compare-and-swap: it lands only if the file still matches that hash.
3. If the agent wrote it while you were typing, the save is **refused** — never a silent overwrite — and you are offered both versions: keep mine, take theirs, or see the difference.
4. If the agent writes while you have unsaved edits, the panel says so on its face and keeps your buffer. It never reloads under you.
5. A save that did not happen is never reported as one, and nothing is destroyed before its replacement is in hand.

Rules 3 to 5 are tonight's principles applied literally: destroy nothing before the replacement is in hand, and confirm a write before anything is built on it. The store already has the precedent — compare-and-swap writes by address, from the mesh work.

### 5.4 Files

Answers one question: **what changed, and who changed it.**

- *Shows:* two groups. **This turn** — the paths the agent touched, with `+`/`−`. **Working tree** — everything not committed, and how far ahead the branch is.
- *Data today:* `model/changes.rs` already computes the working tree from git, with junk filtered out, exactly as Cursor's `Changes +6 −1` pill does. Per-turn attribution needs the kernel (§7): the rewind checkpoint already holds the tree from before each turn, and each `edit` call already names its paths — neither is exposed to a client yet.
- *Who caused it:* each row is marked *agent · turn N* or *yours*. A row marked both is the conflict in §5.3.
- *You can:* open a row (it fills the documents panel), and reveal it on disk. **No revert button** — see §8.

---

## 6. The hard parts

### 6.1 Where typed text goes

We have shipped this bug twice: a filtered list sent a message to an off-screen project, and a rig typed into a search box. Five rules:

1. **One focused surface, ever.** Its title bar is the only lit one. The composer and a panel are never both hot.
2. **Every place that takes text names its destination in place** — the composer names the project and chat it sends to; a shell reads `zsh · ~/proj`; an editor reads the path and *unsaved*; the browser reads the page.
3. **Focus moves only because you moved it.** A click or a key. Never a frame, never an agent action, never a panel opening.
4. **A destination is read from the thing that will receive the text**, never from a list's selection. That is precisely how a message reached an off-screen project.
5. **A test that types must first assert which surface is focused.** A rig that types into whatever has focus is a rig that will one day type into the wrong thing and pass — the same family as the rig that clicked elements nobody could see.

### 6.2 Stale data, and a kernel that is gone

This is the state you meet after something has already gone wrong, which is where every one of last night's worst findings lived. Three states and never a default, per the standing rule that a read answers present, absent or unknown.

**Built and photographed** ([still 10](../media/desktop/side-panel/10-kernel-gone-link-lost.png)): a place's kernel was killed with a running job and a terminal open in the drawer.

- **The link goes down.** Every row that was claiming to be live stops claiming it: the job's tab reads `link lost` instead of `running`, and so does the terminal's. The journal's last lines stay on screen, frozen — what we knew is still worth reading; what we no longer know is not asserted. The body's footer takes the same word from the same function the tab does, because when those two disagreed on screen one of them was lying.
- **What ended still reports how.** An exit code on disk is a fact that outlives the socket, so `done`, `failed` and `stopped` keep their words with the link down.
- **A state is not a property of a row.** It is the row *and* the link, which is why the word is decided in one place that takes both. A row that could read `running` while nothing can hear from it is the 164 GB shape in miniature.

**And the case that ends it: a replacement kernel.** When a kernel dies the desktop starts a new one for that place, and the new kernel has no record of the old pty — while inheriting the old job from disk. Watched live, both tabs went from `link lost` back to `running` and `agent's` the moment the replacement answered: a job ticking that nothing runs, a terminal with no shell behind it.

**Closed by asking** ([#468](https://github.com/unarbos/arbos/pull/468) gave the kernel a `surfaces` frame; [#476](https://github.com/unarbos/arbos/pull/476) wires it, [still 11](../media/desktop/side-panel/11-reconciled-after-a-replacement-kernel.png)). On every attach — not only a reconnect, since after a relaunch the kernel answering is certainly not the one that opened these rows — the window asks what the kernel holds, and reconciles:

- **listed and running**: nothing to say.
- **listed and not running**: take the kernel's own words, and for a job its exit. The job above now reads `stopped`, with *killed: the kernel was stopped and ended its jobs with it after 26s* under its still-readable log.
- **not listed**: the kernel does not hold it. The row stays, because its output is worth reading, and says `gone` — the absence is the fact. That is the terminal above.

`gone` outranks every other word, because an answered question has to outlast a link going down and coming back; that flip is exactly how the lie got in. And the kernel's words are used **only** for an end: a live job's *running for 41s (pid 4812)* would go stale between asks, and a stale clock is a smaller lie of the same kind.

The attach snapshot carrying the same list is now an optimisation over the same function rather than a second path, so handover 1 as written is done.

**Not built yet, and stated as intended behaviour:**

- **Reconnected.** Rows are re-resolved against the kernel's record. Anything it does not know becomes `gone` — never drawn as working. That is the F-137 rule, and it is why the window may not draw a row from its own memory alone.
- **A job whose folder is gone.** Built: the tab reads `no journal`, and the body says its output cannot be read here rather than "(no output yet)" — which is only true while there is somewhere for output to appear. Checked by opening the file, not by remembering a flag. The Stop beside it still needs handover 4.
- **A document changed under you.** A `changed on disk` badge. Never an auto-reload.
- **A browser page nobody has touched for ten minutes.** `stale · 10m`, and the picture is dimmed, because a screenshot from an hour ago looks exactly like a live page.

### 6.3 What survives a relaunch

**Addresses survive; content never does.** On reopening a project we restore the *identity* of what was open — job `j3`, page `b1`, `src/main.rs`, and which row was in front — then resolve each against the kernel. What it still knows comes back live. What it does not know is drawn as `gone`, greyed, dismissible.

We do not cache a job's old output or a page's old screenshot. A stored picture of a page you closed yesterday is a lie waiting for someone to believe it, and that is the whole shape of F-137: the window remembered something the kernel had never had.

Today `Vec<Surface>` lives in memory on `Project` and nothing persists it, so this is new work. It belongs in `.arbos/desktop/` beside boards, so it travels with the folder rather than being orphaned by a rename.

### 6.4 During a call

The call is a first-class mode and panels must work while one runs.

- **Panels are silent.** The narrator's rule is already "it is on your screen" for tool output; a panel is that screen. Opening one speaks nothing, and nothing a panel does interrupts a voice.
- **"Open the terminal" spoken on a call opens the drawer**, the same as typed. One rule for both channels: you asked, so it opens.
- **The work sound keeps playing.** A panel does not duck the bed; only voices do.
- **The call strip is never covered.** It lives under the composer, outside the drawer, and zoom does not reach it. Mute and End are always one click away.
- **Highlights stay in the chat.** A spoken line is written into the transcript as today. Nothing is duplicated into a panel.

### 6.5 Several projects in tabs

- The drawer belongs to the project. Switching tabs switches the drawer, including open or closed and which row was in front. Surfaces already hang off `Project`, so this is the natural shape.
- **A background project's live things keep running**, and are counted on their own tab's badge — never on the front one.
- **Nothing in a background tab draws, speaks or takes a keystroke.** A panel that is not on screen cannot be a destination for text. This is the off-screen-project bug stated as a layout rule.

### 6.6 The graveyard problem

The rail must not accumulate for ever. Two groups and five rules:

**Live** — running now. **Recent** — finished.

1. A thing that finishes keeps its place while the turn that made it is the current one, then falls into Recent.
2. Recent holds at most five per kind, newest first. Everything older is reachable through **Show everything**, which reads the kernel's record — jobs, the archive — rather than the window's memory.
3. **Anything you touched is pinned** and never falls away by itself: a shell you typed in, a file you edited, a page you drove. Your things outlive the agent's.
4. Closing a row closes the *view*, not the process. A running job's row cannot be dismissed while it runs — hiding the thing the panel exists for is the bug, not the feature. Stop it, then dismiss it.
5. Every row carries its state as a word, not a colour: `running`, `finished · exit 0`, `stopped`, `stale · 12m`, `gone`, `yours`.

---

## 7. Scope and order

### First, and done: the shape — [#445](https://github.com/unarbos/arbos/pull/445)

The drawer, default closed and per project, with its tab row, the chords following focus, the three ways back, the empty tab's four cards, and what survives a relaunch. No panel kind is a *feature* yet; the bodies that already render — a job's journal, a terminal, a document, a page — simply render in the drawer now instead of taking the chat's column.

Two behaviour changes fall out of it, both fixes: a file or page the agent opens no longer takes the column (your own complaint about a board arriving over the chat while you typed), and the agent's `focus` command fronts a tab inside the drawer without opening it.

More of this existed than expected. The kernel already runs a pty hub with your own `$SHELL`, and a Chromium page per agent over CDP with `click`, `type` and `press`; the window already had `SurfaceKind::{Terminal, Browser, Process, Panel}`, a git-changes model and a markdown editor. So the four kinds are mostly wiring plus the kernel's missing frames, not new surface.

### Then, in this order

2. **Terminal, properly**: a job's live output with a Stop, and *Open a shell here*. Needs kernel handover 2, 3 and 4.
3. **Files.** `changes.rs` exists; per-turn attribution is the new part (handover 5).
4. **Documents**, read-only now; editing with compare-and-swap saving next (handover 6).
5. **Browser**, the current picture and URL, then screencast and takeover (handover 7).

Also waiting, and deliberately not in the first PR: drag-to-resize (no resize handle exists anywhere in the app yet), reorder and drag between rows, the zoom of §4, and **Show everything**.

### Handover: what the kernel does not have yet

Each of these is a client-visible gap, not an internal refactor.

1. ~~**Open surfaces in the attach snapshot**~~ — **done**, and as a frame rather than a snapshot field: `surfaces` / `surface_list` ([#468](https://github.com/unarbos/arbos/pull/468)), wired in [#476](https://github.com/unarbos/arbos/pull/476). A window now rebuilds its rows from the kernel's record instead of its memory, which is the F-137 fix at the protocol level. A **live** job's row comes back with it ([stills 12 and 13](../media/desktop/side-panel/12-live-job-back-after-a-relaunch.png)): nothing on disk can restore that one, since a running job has no `exit` file, so the kernel's answer seeds it — listed, not fronted, primed with what the job has already written and appended to by the kernel's `job` frames from there. Those frames carry what is appended *after* a client attaches and never a replay, which is why the row is primed rather than left to start at whatever second the window came back. Shells are deliberately not seeded: a pty's scrollback is not replayed, so the row would draw an empty screen for a live shell — that wants a kernel-side replay before it wants a row.
2. ~~**Job metadata on the frame**~~ — **done** in the same pair: command line, cwd, owning agent, started-at, `journal: present | gone`, `pid`, `status`.
3. **`by: user | agent` on `Frame::Board`, and a client frame that asks for a shell.** Together these are what make "open the terminal" open it: the window would know the route, and `PtyHub::spawn_shell` would be reachable from a click. Without them the Terminal card can only put the words in the composer.
3. **Job metadata on the frame** — the command line, cwd, owning agent, started-at, and `journal: present | gone`. Today a `Job` frame carries deltas but not enough to title a row honestly.
4. **Stop a job from a client**, routed through the kernel's own group kill, with the reason written by a single writer. There is already a known race between two writers on the killed reason; do not add a third.
5. **Per-turn changed paths** — expose the rewind checkpoint's diff as a frame (`turn N`, paths, `+`/`−`), plus the paths each edit tool call touched. The data exists for rewind and is not readable by a client.
6. **A path a person is editing, and `write_if_unchanged`** — the two halves of Jacob's co-editing ruling. A client claims a path while its editor holds unsaved edits, and the kernel **refuses the agent's write** to a claimed path, telling the agent in its own turn so it can say what it wanted. Plus `write_if_unchanged(path, expected_hash)` and a `changed` frame per watched path for the save itself. Without the refusal the window can only decline to be overwritten, which defends his buffer but not his file.
7. **Browser screencast and input** — `Page.startScreencast` frames out, an input frame in, and a `driver: agent | user` field with a loud refusal for whoever is not driving.
8. **A person's shell in a job's directory**, so *Open a shell here* starts your shell where the job ran.

---

## 8. What I would not build

- **A free grid or tiling workspace.** §4.
- **Presets and saved layouts.** With nothing to arrange there is nothing to save.
- **A second tab bar over the chat.** The drawer's tabs live in the drawer; the chat column keeps none of its own.
- **A code editor.** The article editor is enough for "open this file and let me edit". You asked to edit a file, not to replace Cursor. No language server, no multi-file editing, no find-and-replace across the project.
- **A revert or discard button in the files panel.** Rewind owns undoing, and six data-loss bugs in one night came from second paths to destroying work.
- **One terminal object with two owners.** §5.1.
- **Auto-open, auto-focus, and a badge that nags.** §4.
- **Cached content across a relaunch.** §6.3.
- **A panel that speaks.** §6.4.

---

## Running any of this yourself

Build the kernel from the branch under test before you judge a panel feature: `cargo build -p arbos-kernel`, then let the app find it (`ARBOS_KERNEL_BIN`, or `target/debug/`). A kernel binary older than the branch answers nothing to a frame it has never heard of, and the window then draws exactly what a broken feature draws. That cost an hour here, and it was the third time in one day that someone lost time to a binary that was not what its path implied.

The Xvfb check is `desktop/driver/examples/side_panel.py`. For anything a harness cannot reach — a real pty, a real job, a kernel that dies — drive a real turn with no model: `arbos-kernel serve <place> --provider replay --replies <file>`, where the file is one JSON object per line with `content` and `calls`.

---

## Against Cursor

"Copy exactly how Cursor is doing this" was measured rather than remembered. The parity loop timed Cursor's own side panel at a 1440-wide window, pixel by pixel: `internal/cursor-side-panel-measured.md`, with stills in `media/cursor-reference/side-panel/`. What it found, and what this took:

**Taken.** A surface tab opens at **592** and a drag clamps between **377 and 766**, with the chat's floor at **418** — Cursor's divider stops there and never squeezes the chat under it. A new tab opens **beside the one in front**, not at the end. The row is **40** with a **26** pill, and a panel tab **hugs its label** rather than sitting in an equal cell the way our project tabs do. The empty tab is a **2×2 of tiles**, icon over label, low in the panel — which is exactly the empty state in your screenshot, so that state is Cursor's rather than ours; ours is built fresh to match. And the **⤢ expand** control joins the row's right side, which answers two of the three icons at the top right of your still. The third is Cursor's Ports popover, and we have nothing to put in it.

**Confirmed.** Cursor cycles its panel tabs with **the same chord as its project strip, and which set moves follows focus** — the rule here, arrived at independently.

**Not taken, with reasons.** Cursor throws the whole tab set away when you close its last tab, while keeping it when you toggle the panel shut: two answers for "the panel went away". Ours keeps it either way. And Cursor shows **nothing** about which tab set its chord will move — no focus ring, no difference in the active pill. That is the one place their own measurer judged us better, and it is why the focused row here is lit and the other muted.

**Different by your instruction.** Cursor has no generic new-tab chord: it opens panel tabs by kind (⌘G file, ⌘J terminal, ⌘⇧B browser, ⌘E changes) and never uses ⌘T. You asked for ⌘T by name, so ⌘T it is, landing on the same four choices Cursor's `+` menu offers. Their kind chords are worth adding beside it later, and their `+` menu's search field — *"Open any file, URL, …"* — is the better half of that menu and worth taking when the File and Browser tabs are real.

**Still open with them:** whether a second Browser tab can be opened from the `+` menu, drag between the two tab sets, and Retina metrics, which none of this was measured at.

---

## Decided: co-editing, and who wins

**Jacob's ruling, 2026-09-17: option 1 — his version wins, and the agent is refused.** No fork remains here; this is the rule.

What that means, precisely, because the halves are easy to confuse:

1. **The file is always his to type in.** Opening a document for editing never waits on the agent's turn and is never read-only for being near one.
2. **While he has unsaved edits in a file, the agent's write to that path is refused** — told plainly, in the agent's own turn, so it can say what it wanted to change instead of losing it silently. His buffer is never reloaded under him and never merged into.
3. **With no unsaved edits, the agent writes freely** and the tab says `changed on disk`. Nothing is auto-reloaded: a reload while he is reading is its own small theft.
4. **His save is still a compare-and-swap** on the bytes the file had when he opened it. Rule 2 makes a clash rare rather than impossible — another window, another machine, a hook — and a refused save says what happened rather than overwriting.

Rules 1, 3 and 4 are the window's. **Rule 2 is the kernel's**, and it is the one thing this needs that does not exist yet: a client must be able to say "a person is editing this path", and the kernel must refuse the agent's writes to it while that holds. That is handover 6, with `write_if_unchanged` beside it for rule 4.

Until the kernel can refuse, the window keeps the half it owns — his buffer is never overwritten and never silently reloaded — and a document tab stays read-only rather than pretending his edit is defended. Offering an edit we cannot keep is worse than not offering it yet.
