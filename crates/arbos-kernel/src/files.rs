//! Files under `.arbos/` over the attach socket: `read`, `tail`, `list`.
//! For a client that has no view of the folder (a phone through a
//! tunnel), answered on its own connection. Paths are relative to
//! `.arbos/` and stay inside it: `..` is folded before the check and the
//! deepest existing ancestor is canonicalised, so a symlink out of the
//! folder does not get through either. `access.toml` (tokens) is never
//! served. Secrets on disk elsewhere are the redaction layer's job, as
//! for the transcript.

use std::path::{Component, Path, PathBuf};

use arbos_core::Place;
use arbos_core::wire::{Entry, Frame};

/// Longest file `read` returns whole; past it, `truncated` and page with `tail`.
pub const READ_CAP: u64 = 1024 * 1024;
/// `tail` limit when the client sends 0, and its ceiling.
pub const TAIL_DEFAULT: u64 = 256 * 1024;
pub const TAIL_MAX: u64 = 1024 * 1024;

/// Files that are never served, whatever the role.
const NEVER: &[&str] = &["access.toml"];

pub fn handle(place: &Place, frame: Frame) -> Option<Frame> {
    match frame {
        Frame::Read { path } => Some(read(place, &path)),
        Frame::Tail { path, from, limit } => Some(tail(place, &path, from, limit)),
        Frame::List { path } => Some(list(place, &path)),
        _ => None,
    }
}

fn read(place: &Place, rel: &str) -> Frame {
    let refused = |e: String| Frame::File {
        path: rel.to_string(),
        text: String::new(),
        size: 0,
        truncated: false,
        error: Some(e),
    };
    let full = match confine(place, rel) {
        Ok(p) => p,
        Err(e) => return refused(e),
    };
    let meta = match std::fs::metadata(&full) {
        Ok(m) => m,
        Err(e) => return refused(format!("{rel}: {e}")),
    };
    if meta.is_dir() {
        return refused(format!("{rel} is a folder; use list"));
    }
    let size = meta.len();
    let (bytes, truncated) = match read_range(&full, 0, READ_CAP.min(size)) {
        Ok(b) => (b, size > READ_CAP),
        Err(e) => return refused(format!("{rel}: {e}")),
    };
    Frame::File {
        path: rel.to_string(),
        text: String::from_utf8_lossy(&bytes).into_owned(),
        size,
        truncated,
        error: None,
    }
}

fn tail(place: &Place, rel: &str, from: u64, limit: u64) -> Frame {
    let refused = |e: String| Frame::Chunk {
        path: rel.to_string(),
        from,
        to: from,
        size: 0,
        text: String::new(),
        error: Some(e),
    };
    let full = match confine(place, rel) {
        Ok(p) => p,
        Err(e) => return refused(e),
    };
    let meta = match std::fs::metadata(&full) {
        Ok(m) => m,
        Err(e) => return refused(format!("{rel}: {e}")),
    };
    if meta.is_dir() {
        return refused(format!("{rel} is a folder; use list"));
    }
    let size = meta.len();
    let limit = if limit == 0 {
        TAIL_DEFAULT
    } else {
        limit.min(TAIL_MAX)
    };
    let from = from.min(size);
    let want = limit.min(size - from);
    let mut bytes = match read_range(&full, from, want) {
        Ok(b) => b,
        Err(e) => return refused(format!("{rel}: {e}")),
    };
    // Stop at the last newline unless the chunk reaches the end of the
    // file, so a client never gets half a JSON line.
    if from + want < size {
        if let Some(nl) = bytes.iter().rposition(|b| *b == b'\n') {
            bytes.truncate(nl + 1);
        }
    }
    let to = from + bytes.len() as u64;
    Frame::Chunk {
        path: rel.to_string(),
        from,
        to,
        size,
        text: String::from_utf8_lossy(&bytes).into_owned(),
        error: None,
    }
}

fn list(place: &Place, rel: &str) -> Frame {
    let refused = |e: String| Frame::Listing {
        path: rel.to_string(),
        entries: Vec::new(),
        error: Some(e),
    };
    let full = match confine(place, rel) {
        Ok(p) => p,
        Err(e) => return refused(e),
    };
    let dir = match std::fs::read_dir(&full) {
        Ok(d) => d,
        Err(e) => return refused(format!("{rel}: {e}")),
    };
    let mut entries: Vec<Entry> = dir
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if NEVER.contains(&name.as_str()) && rel.trim_matches('/').is_empty() {
                return None;
            }
            let meta = e.metadata().ok()?;
            Some(Entry {
                name,
                dir: meta.is_dir(),
                size: if meta.is_dir() { 0 } else { meta.len() },
                modified: meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as i64),
            })
        })
        .collect();
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Frame::Listing {
        path: rel.to_string(),
        entries,
        error: None,
    }
}

/// `rel` (relative to `.arbos/`) as a path that is inside `.arbos/`.
fn confine(place: &Place, rel: &str) -> Result<PathBuf, String> {
    let root = place.arbos();
    let rel = rel.trim().trim_start_matches("./");
    let rel = rel.strip_prefix(".arbos/").unwrap_or(rel);
    let mut out = PathBuf::new();
    for part in Path::new(rel).components() {
        match part {
            Component::Normal(p) => out.push(p),
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    return Err(format!("{rel}: leaves .arbos/"));
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(format!("{rel}: paths are relative to .arbos/"));
            }
        }
    }
    if out
        .file_name()
        .is_some_and(|n| NEVER.contains(&n.to_string_lossy().as_ref()))
        && out.parent().is_none_or(|p| p.as_os_str().is_empty())
    {
        return Err(format!("{rel}: not served"));
    }
    let full = root.join(&out);
    // A symlink to the outside: follow the deepest existing ancestor and
    // check it still lives under the real .arbos/.
    let root_real = std::fs::canonicalize(&root).unwrap_or(root.clone());
    let mut probe = full.clone();
    while !probe.exists() {
        match probe.parent() {
            Some(p) => probe = p.to_path_buf(),
            None => break,
        }
    }
    let probe_real = std::fs::canonicalize(&probe).unwrap_or(probe);
    if !probe_real.starts_with(&root_real) {
        return Err(format!("{rel}: leaves .arbos/"));
    }
    Ok(full)
}

fn read_range(path: &Path, from: u64, len: u64) -> std::io::Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path)?;
    f.seek(SeekFrom::Start(from))?;
    let mut buf = vec![0u8; len as usize];
    let mut got = 0;
    while got < buf.len() {
        let n = f.read(&mut buf[got..])?;
        if n == 0 {
            break;
        }
        got += n;
    }
    buf.truncate(got);
    Ok(buf)
}
