//! A project file as an editor, not a preview.
//!
//! Opening a text file in the drawer must let him type. The kernel's
//! `claim` holds the path while this buffer is dirty; a save is
//! compare-and-swap on the hash from open.

use crate::model::{panel::DOCUMENTS_EDITABLE, workspace::Workspace};
use arbos_core::hub::content_hash;
use bezel::{
    gpui::{App, Context, Entity, Focusable, SharedString, Task, Window, div, prelude::*, px},
    theme::{TextStyle, Theme, Typeset},
};
use editor::Editor;
use std::{
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

/// How long after the last keystroke a dirty buffer writes.
const SAVE_AFTER: Duration = Duration::from_millis(400);

pub struct FileDoc {
    path: PathBuf,
    workspace: Entity<Workspace>,
    editor: Entity<Editor>,
    saved: String,
    hash: String,
    claimed: bool,
    notice: Option<String>,
    save: Option<Task<()>>,
}

impl FileDoc {
    pub fn new(path: PathBuf, workspace: Entity<Workspace>, cx: &mut Context<Self>) -> Self {
        let on_disk = std::fs::read_to_string(&path).unwrap_or_default();
        let hash = content_hash(on_disk.as_bytes());
        let editor = cx.new(|cx| Editor::new(&on_disk, cx));
        // "Saved" is the editor's own reading of the file, not the bytes:
        // the editor normalises what it loads (the trailing newline), and
        // measured against the bytes every file opened from the panel read
        // *· unsaved* before a keystroke (F-262, cycle 85).
        let saved = editor.read(cx).source();
        cx.observe(&editor, |this, _, cx| this.on_edit(cx)).detach();
        Self {
            path,
            workspace,
            editor,
            saved,
            hash,
            claimed: false,
            notice: None,
            save: None,
        }
    }

    fn on_edit(&mut self, cx: &mut Context<Self>) {
        let source = self.editor.read(cx).source();
        if source == self.saved {
            if self.claimed {
                self.set_claim(false, cx);
            }
            self.notice = None;
            cx.notify();
            return;
        }
        if !self.claimed {
            self.set_claim(true, cx);
        }
        self.notice = None;
        let this = cx.entity().downgrade();
        self.save = Some(cx.spawn(async move |_, cx| {
            cx.background_executor().timer(SAVE_AFTER).await;
            let _ = this.update(cx, |this, cx| this.save(cx));
        }));
        cx.notify();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let source = self.editor.read(cx).source();
        if source == self.saved {
            return;
        }
        let current = match std::fs::read(&self.path) {
            Ok(bytes) => content_hash(&bytes),
            Err(_) => String::new(),
        };
        if current != self.hash {
            self.notice = Some(
                "The file changed on disk. Your buffer is kept. Save again after you decide."
                    .into(),
            );
            cx.notify();
            return;
        }
        let tmp = self.path.with_file_name(format!(
            ".{}.{}.tmp",
            self.path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("save"),
            std::process::id()
        ));
        if let Err(error) =
            std::fs::write(&tmp, &source).and_then(|()| std::fs::rename(&tmp, &self.path))
        {
            let _ = std::fs::remove_file(&tmp);
            self.notice = Some(error.to_string());
            cx.notify();
            return;
        }
        self.saved = source.clone();
        self.hash = content_hash(source.as_bytes());
        self.notice = None;
        self.set_claim(false, cx);
        self.workspace.update(cx, |workspace, _| {
            workspace.save_file(&self.path, &source, &self.hash);
        });
        cx.notify();
    }

    fn set_claim(&mut self, held: bool, cx: &mut Context<Self>) {
        self.claimed = held;
        self.workspace.update(cx, |workspace, _| {
            workspace.claim_path(&self.path, held);
        });
    }
}

impl Drop for FileDoc {
    fn drop(&mut self) {
        // The workspace may already be gone with the window.
    }
}

impl Focusable for FileDoc {
    fn focus_handle(&self, cx: &App) -> bezel::gpui::FocusHandle {
        self.editor.focus_handle(cx)
    }
}

impl Render for FileDoc {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let dirty = self.editor.read(cx).source() != self.saved;
        div()
            .id("file-editor")
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_none()
                    .px(px(12.))
                    .py(px(6.))
                    .text_style(TextStyle::Caption)
                    .text_color(theme.text_faint)
                    .child(SharedString::from(if dirty {
                        format!("{} · unsaved", self.path.display())
                    } else {
                        self.path.display().to_string()
                    })),
            )
            .children(self.notice.as_ref().map(|line| {
                div()
                    .flex_none()
                    .px(px(12.))
                    .py(px(4.))
                    .text_style(TextStyle::Caption)
                    .text_color(theme.text_muted)
                    .child(SharedString::from(line.clone()))
            }))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .px(px(12.))
                    .child(self.editor.clone()),
            )
    }
}

/// Whether this path should open as an editor rather than a preview.
pub fn is_editable(path: &Path) -> bool {
    if !DOCUMENTS_EDITABLE || !path.is_file() {
        return false;
    }
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "ico" | "bmp" | "avif" | "pdf" => false,
        _ => {
            let mut file = match std::fs::File::open(path) {
                Ok(file) => file,
                Err(_) => return false,
            };
            let mut buf = [0u8; 8192];
            match file.read(&mut buf) {
                Ok(n) => std::str::from_utf8(&buf[..n]).is_ok(),
                Err(_) => false,
            }
        }
    }
}
