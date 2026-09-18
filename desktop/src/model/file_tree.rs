//! The project's local directory as a tree: folders, files, expand and
//! collapse. What Browse files draws. One folder is listed at a time; a
//! closed folder has no children in the list.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

/// One visible row: a folder or a file under the project root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRow {
    pub path: PathBuf,
    pub name: String,
    pub dir: bool,
    pub depth: usize,
    /// `Some` for a folder: whether it is open. `None` for a file.
    pub expanded: Option<bool>,
}

/// How many names one folder may contribute. A huge directory stays a
/// glance, not `ls -la` of everything.
const FOLDER_CAP: usize = 400;

/// Children of `dir`, directories first, then by name. Skips only the
/// names a listing cannot use (`.` and `..`). The disk is otherwise as
/// it is.
pub fn list_dir(dir: &Path) -> Vec<(PathBuf, String, bool)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut rows: Vec<(PathBuf, String, bool)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == "." || name == ".." {
                return None;
            }
            let dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
            Some((entry.path(), name, dir))
        })
        .collect();
    rows.sort_by(|a, b| {
        b.2.cmp(&a.2)
            .then_with(|| a.1.to_ascii_lowercase().cmp(&b.1.to_ascii_lowercase()))
            .then_with(|| a.1.cmp(&b.1))
    });
    rows.truncate(FOLDER_CAP);
    rows
}

/// The rows a tree shows: children of `root`, plus the children of every
/// path in `expanded`.
pub fn visible(root: &Path, expanded: &HashSet<PathBuf>) -> Vec<FileRow> {
    let mut out = Vec::new();
    walk(root, 0, expanded, &mut out);
    out
}

fn walk(dir: &Path, depth: usize, expanded: &HashSet<PathBuf>, out: &mut Vec<FileRow>) {
    for (path, name, is_dir) in list_dir(dir) {
        if is_dir {
            let open = expanded.contains(&path);
            out.push(FileRow {
                path: path.clone(),
                name,
                dir: true,
                depth,
                expanded: Some(open),
            });
            if open {
                walk(&path, depth + 1, expanded, out);
            }
        } else {
            out.push(FileRow {
                path,
                name,
                dir: false,
                depth,
                expanded: None,
            });
        }
    }
}

/// Open a closed folder, or close an open one.
pub fn toggle(expanded: &mut HashSet<PathBuf>, path: &Path) {
    if !expanded.remove(path) {
        expanded.insert(path.to_path_buf());
    }
}
