# Desktop: files editor, file tree, Browser, terminal `%`

Jacob on Mac. Four drawer bugs. One PR: they share the side panel chrome (`drawer.rs`, surfaces, kernel frames). Do not publish `v0.2.0`. Do not touch GPT Live, Jev, iOS, the Project page, the This Mac composer control, or typing `clear`.

Prior panel work: [#544](https://github.com/unarbos/arbos/pull/544).

## What he sees

1. **A file is not an editor.** Opening `finalturns.json` (and any other file) paints a read-only markdown / JSON view in the drawer. He must type in the file.
2. **Browse files is not a tree.** `+` → Browse files… opens the system picker. He wants the project's local folder as a real tree: folders, expand and collapse, the disk.
3. **Browser does nothing.** The Browser card and the `+` row put "Open a browser page." in the composer. The drawer never opens a page.
4. **A new Terminal starts with `%`.** Tab "Terminal yours", then `%`, then the prompt. That mark is zsh `PROMPT_EOL_MARK` / `PROMPT_SP` on first paint.

## What we will do

**Editor.** The kernel already has `claim` and `save` (handover 6). Turn `DOCUMENTS_EDITABLE` on. A text file opens in `bezel-editor` (the same editor articles use), not a preview. While he has unsaved edits, claim the path so the agent cannot write it. A save is compare-and-swap on the hash from open. Never reload under him. Pictures stay pictures.

**Browse files.** The `+` row and the File card open a Files tab on the project folder. That tab is a VS Code-style tree of the real disk: directories first, expand and collapse, click a file to open it in the editor. Not a flat 80-row list. Not the OS picker.

**Browser.** Same shape as Terminal / `Frame::Shell`. A `browse` frame asks the kernel to open its Chromium page (`by: user`). The drawer fronts that tab. The existing page view (URL + picture) is what he sees. Start a watch so the picture stays live.

**Terminal `%`.** When the kernel starts `$SHELL -il`, unset zsh `PROMPT_SP` and set `PROMPT_EOL_MARK` empty. A new shell shows only the prompt.

## Files we expect to touch

- `desktop/src/view/drawer.rs` — cards and `+` menu
- `desktop/src/view/files.rs` (new) — tree
- `desktop/src/view/file_editor.rs` (new) — editor pane
- `desktop/src/model/file_tree.rs` (new) — list one folder, expand set
- `desktop/src/model/panel.rs` — `DOCUMENTS_EDITABLE`
- `desktop/src/model/workspace.rs` — open files tree, open browser, claim / save
- `desktop/src/agent/acp.rs` — `browse`, `claim`, `save`
- `crates/arbos-core/src/wire.rs` — `Frame::Browse`
- `crates/arbos-kernel/src/serve.rs` — handle `browse`
- `crates/arbos-kernel/src/pty.rs` — no leading `%`

## How to check

- Open a project, `⌘B`, `+` → Browse files…: a tree of that folder, expand a directory, open a `.json` or `.md` and type. The tab is an editor, not a graph.
- Click Browser: a Browser tab opens with a page (not the composer).
- Click Terminal: the first line is the prompt, with no `%` above it.
