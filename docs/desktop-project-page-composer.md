# Desktop: composer machine pill, Project page, `clear`

Jacob asked for three desktop changes on 2026-09-18. One PR on `unarbos/arbos`. No v0.2.0 publish. No GPT Live, no Jev slices A–G, no iOS. The files-editor / terminal slice is another worker's.

Terms are defined the first time they appear.

**Status:** being built now.

---

## 1. Remove the **This Mac** control

**What it is.** The row under the composer (the text box at the bottom of chat) has a pill that says **This Mac** on a Mac, or **This Computer** elsewhere. A click opens the machine / folder picker. The control's id is `composer-machine`.

**Why it goes.** Projects open from the **project tabs** at the top of the window. A second machine picker under the chat is the wrong place. Jacob's shot: the composer with **This Mac** under it.

**What stays in that row.**

- The **branch** pill (the git branch name, when the folder is a local repo). This is not a machine picker.
- Voice / call status.
- The spinner while a turn runs.
- A **plain status line** when the kernel is reconnecting or the link is lost. Not a button. Not a chevron.

**What is gone.**

- The laptop glyph, the words **This Mac** / **This Computer**, the remote alias as a picker, and the down-chevron.
- The click that opens a new tab / opener.

The opener itself (⌘T, the + on the tab strip) does not change. `This Mac` as a row **inside** the opener stays. That is how you open a folder on this machine.

---

## 2. Project page matches Cursor's project home

**What it is.** ⌘2, or **Open the project page** in the right panel, fills the main column with the **Project page**. Today that page is a dump: a small header, then notes, then a grid of file cards, then Context. It does not read like Cursor.

**Target.** Cursor's project home: a clean hierarchy, a **Recents** list, spacing and type. Not Agents / Processes / Resources / Files as a pile. Those lists stay in the **right panel**. They do not move onto the page.

**What the page shows, top to bottom.**

1. **Header.** Project glyph, large name, path as a caption. The notes button and **Back to chat** stay (Escape and ⌘1 still leave).
2. **Recents.** Chats in this project, newest first, not archived. Each row is a name and how long ago it last spoke. A click opens that chat. No Recents section when the project has no chats yet.
3. **Status page.** The parsed `notes.md` (the project's live status file): tldr, headings, checklist rows. Same data as today. More space between sections. Headings read as headings, not as a caption dump.
4. **Files.** A simple list, not a card grid. Name, folder, age. A click opens the file.
5. **Context.** The context document as prose under a heading, when it exists.

**What the page does not show.**

- Agents, Processes, or Resources. Those belong in the right panel.
- A card grid.

**Right panel.** Unchanged in this PR, except that it already holds the lists the page must not dump.

---

## 3. `clear` empties the chat view

**What you type.** `clear` in the composer, alone, then Enter. `/clear` does the same. Extra words, or attachments, go to the agent as a normal message. The top-right Clear control does the same hide-and-recenter. Jacob's shot of the broken send (kernel replied "Cleereed…"): `/home/ubuntu/.cursor/projects/workspace/assets/8ed86ca8-7ff1-4214-8cd1-7b56b90fd7aa.png`.

**What happens.**

- The chat view hides every line that is already on screen.
- The column returns to the **centered empty chat**: title above the composer, composer in the middle of the column.
- The composer field clears.
- The line is **not** sent to the kernel.
- The line is **not** written to the transcript.

**What does not happen.**

- The project does not change.
- The transcript on disk does not change. Same `transcript.jsonl`. Same context for the agent.
- No archive. No new chat. No rewind.

**After `clear`.**

- A new message you type is a normal send. The view shows **only** lines from that send onward. Older lines stay on disk and stay hidden.
- `clear` again hides the new lines too.
- The empty-chat placeholder comes back (`Plan, search, build anything`).
- Kickoff (the first-run "Setting up environment" block) does **not** come back. This is not a new project.
- Switching project tabs and coming back keeps the hide for that chat, in this window only.
- A relaunch of the app shows the full transcript again. The hide is memory-only. That is the "UI only" rule.

---

## Out of scope

- Publishing `v0.2.0`.
- GPT Live.
- Jev slices A–G.
- iOS.
- Files opening in a real editor, Browse as a VS Code-like tree, Browser opening, Terminal's leading `%`. Another worker owns that slice.

---

## How to check

1. Open a project. The row under the composer has no **This Mac** / **This Computer** pill. ⌘T still opens a project.
2. Press ⌘2. The page has a large name, Recents, then the status page, then a file **list**. Not a card dump. Not Agent