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

Three states and never a default, per the standing rule that a read answers present, absent or unknown:

- **Link lost.** Every live row freezes, greys, and reads `link lost · 40s`. Content stays on screen; nothing is claimed to be running.
- **Reconnected.** Rows are re-resolved against the kernel's record. Anything the kernel does not know becomes `gone` — it is never drawn as working. That is the F-137 rule, and it is why the window may not draw a row from its own memory alone.
- **A job whose folder is gone.** `journal gone · the process may still be running`, with Stop still offered. Not an empty log, which reads as finished — that is how 164 GB went unnoticed.
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

1. **Open surfaces in the attach snapshot** — the kernel's own list of live jobs, pty pages and browser pages, so a reconnecting or relaunching window rebuilds rows from the kernel's record instead of its memory. This is the F-137 fix at the protocol level, and it is what makes a *live* job's tab survive a relaunch: today only a finished one does, because its journal and `exit` file are a record on disk the window can read for itself.
2. **`by: user | agent` on `Frame::Board`, and a client frame that asks for a shell.** Together these are what make "open the terminal" open it: the window would know the route, and `PtyHub::spawn_shell` would be reachable from a click. Without them the Terminal card can only put the words in the composer.
3. **Job metadata on the frame** — the command line, cwd, owning agent, started-at, and `journal: present | gone`. Today a `Job` frame carries deltas but not enough to title a row honestly.
4. **Stop a job from a client**, routed through the kernel's own group kill, with the reason written by a single writer. There is already a known race between two writers on the killed reason; do not add a third.
5. **Per-turn changed paths** — expose the rewind checkpoint's diff as a frame (`turn N`, paths, `+`/`−`), plus the paths each edit tool call touched. The data exists for rewind and is not readable by a client.
6. **`write_if_unchanged(path, expected_hash)`** and a `changed` frame per watched path, so an editor can save safely and can tell when the agent wrote underneath it.
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

## Against Cursor

"Copy exactly how Cursor is doing this" is being measured rather than remembered: the parity loop is timing its side panel — tab behaviour, what its chords do, widths, drag and reorder, the empty state, one tab and many — and will write it up for this. Where their measurement and this document disagree, Cursor wins and the difference gets noted here. Three questions are already out to them: whether its tab cycling wraps (ours does, following our own project tabs), what the two other icons at the top right of your screenshot are, and its width floor and ceiling.

One note on the screenshot: the four-card empty state in it is not in Arbos today — it is Cursor's own. The cards here are built fresh.

---

## One decision for you

**When you open a file to edit while the agent may be writing it, which of these?**

1. **Always yours to type in.** Saving is checked against the file on disk, and if the agent got there first you choose: keep mine, take theirs, or see the difference. Most freedom, and the clash lands at save time.
2. **Read-only while the agent's turn is running**, with a *Take it anyway* button that switches to option 1. Safest, and it costs you a click on a file the agent happens to be near.
3. **Opening it holds the agent off that one file** until you save or close, with the agent told plainly. Never a clash — but your open editor can now block a working agent, and you would have to remember it is open.

My recommendation is **1**. Clashes on the same file in the same minute will be rare, the compare-and-swap save makes a silent overwrite impossible, and 2 and 3 both pay a standing cost to prevent a rare event.
