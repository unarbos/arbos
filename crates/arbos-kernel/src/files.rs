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
        Frame::Put {
            path,
            text,
            data,
            base_hash,
        } => Some(match data {
            Some(b64) => put_bytes(place, &path, &b64),
            None => put(place, &path, &text, base_hash.as_deref()),
        }),
        _ => None,
    }
}

/// Why a peer's write was refused, in words the writer can act on.
pub const PUT_WHERE: &str =
    "a peer writes only the store's shared folders: docs/, internal/, media/";

/// A write from another node, by address. The same rules as a local
/// write, whoever the peer is: the root-owned pages (`notes.md`,
/// `docs/project-context.md`, `archived.md`) stay this place's root's
/// (propose with `say to=<machine>/<project>/root`); protected files are
/// never written; only the shared folders take a write. Compare-and-swap
/// on `base_hash`: the write happens only if the file still hashes to it
/// (`""` = must not exist yet), so two writers never lose an update. The
/// file lands whole (tmp + rename), as every local write does.
fn put(place: &Place, rel: &str, text: &str, base_hash: Option<&str>) -> Frame {
    let current_of = |full: &Path| -> (u64, String) {
        match std::fs::read(full) {
            Ok(bytes) => (bytes.len() as u64, arbos_core::hub::content_hash(&bytes)),
            Err(_) => (0, String::new()),
        }
    };
    let refused = |e: String, full: Option<&Path>| {
        let (size, hash) = full.map(current_of).unwrap_or((0, String::new()));
        Frame::Written {
            path: rel.to_string(),
            size,
            hash,
            error: Some(e),
        }
    };
    let full = match confine(place, rel) {
        Ok(p) => p,
        Err(e) => return refused(e, None),
    };
    if let Some(entry) = arbos_core::store::protected_by(place.path(), &full) {
        return refused(
            format!("{rel}: {entry} shapes how agents here behave; not written by a peer"),
            None,
        );
    }
    if arbos_core::store::is_root_owned(place.path(), &full) {
        return refused(
            format!("{rel}: {}", arbos_core::store::REFUSAL),
            Some(&full),
        );
    }
    if !arbos_core::store::is_store_path(place.path(), &full) {
        return refused(format!("{rel}: {PUT_WHERE}"), None);
    }
    if full.is_dir() {
        return refused(format!("{rel} is a folder"), None);
    }
    let (_, current) = current_of(&full);
    if let Some(base) = base_hash
        && base != current
    {
        return refused(
            if current.is_empty() {
                format!("{rel}: conflict — the file no longer exists; read it again before writing")
            } else if base.is_empty() {
                format!("{rel}: conflict — the file exists now; read it before writing")
            } else {
                format!(
                    "{rel}: conflict — the file changed since you read it; read it again and redo the edit"
                )
            },
            Some(&full),
        );
    }
    if let Some(parent) = full.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        return refused(format!("{rel}: {e}"), None);
    }
    let tmp = full.with_file_name(format!(
        ".{}.{}.tmp",
        full.file_name().and_then(|n| n.to_str()).unwrap_or("put"),
        std::process::id()
    ));
    if let Err(e) = std::fs::write(&tmp, text).and_then(|()| std::fs::rename(&tmp, &full)) {
        let _ = std::fs::remove_file(&tmp);
        return refused(format!("{rel}: {e}"), Some(&full));
    }
    Frame::Written {
        path: rel.to_string(),
        size: text.len() as u64,
        hash: arbos_core::hub::content_hash(text.as_bytes()),
        error: None,
    }
}

/// A client's file, as bytes: a photo attached on the phone, which has
/// no path on this machine. Lands whole under `.arbos/attachments/…` (or
/// one of the shared folders), never over a protected or root-owned
/// file, at most `PUT_MAX_BYTES` decoded. The reply's `path` is what the
/// client puts in the next `user` frame's `attachments`.
fn put_bytes(place: &Place, rel: &str, b64: &str) -> Frame {
    use base64::Engine;
    let refused = |e: String| Frame::Written {
        path: rel.to_string(),
        size: 0,
        hash: String::new(),
        error: Some(e),
    };
    let full = match confine(place, rel) {
        Ok(p) => p,
        Err(e) => return refused(e),
    };
    let under_attachments = full
        .strip_prefix(place.arbos())
        .ok()
        .and_then(|r| r.components().next())
        .is_some_and(|c| c.as_os_str() == arbos_core::wire::ATTACHMENTS_DIR);
    if !under_attachments && !arbos_core::store::is_store_path(place.path(), &full) {
        return refused(format!(
            "{rel}: a file goes under {}/ (or docs/, internal/, media/)",
            arbos_core::wire::ATTACHMENTS_DIR
        ));
    }
    if arbos_core::store::protected_by(place.path(), &full).is_some()
        || arbos_core::store::is_root_owned(place.path(), &full)
    {
        return refused(format!("{rel}: not a file a client may replace"));
    }
    if full.is_dir() {
        return refused(format!("{rel} is a folder"));
    }
    // Rough size before decoding: 4 chars per 3 bytes.
    if b64.len() / 4 * 3 > arbos_core::wire::PUT_MAX_BYTES + 3 {
        return refused(format!(
            "{rel}: {} MB is over the {} MB cap for one file",
            b64.len() / 4 * 3 / (1024 * 1024),
            arbos_core::wire::PUT_MAX_BYTES / (1024 * 1024)
        ));
    }
    let clean: String = b64.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = match base64::engine::general_purpose::STANDARD
        .decode(&clean)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(&clean))
    {
        Ok(b) => b,
        Err(e) => return refused(format!("{rel}: data is not base64: {e}")),
    };
    if bytes.len() > arbos_core::wire::PUT_MAX_BYTES {
        return refused(format!(
            "{rel}: {} MB is over the {} MB cap for one file",
            bytes.len() / (1024 * 1024),
            arbos_core::wire::PUT_MAX_BYTES / (1024 * 1024)
        ));
    }
    if let Some(parent) = full.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        return refused(format!("{rel}: {e}"));
    }
    let tmp = full.with_file_name(format!(
        ".{}.{}.tmp",
        full.file_name().and_then(|n| n.to_str()).unwrap_or("put"),
        std::process::id()
    ));
    if let Err(e) = std::fs::write(&tmp, &bytes).and_then(|()| std::fs::rename(&tmp, &full)) {
        let _ = std::fs::remove_file(&tmp);
        return refused(format!("{rel}: {e}"));
    }
    Frame::Written {
        path: rel.to_string(),
        size: bytes.len() as u64,
        hash: arbos_core::hub::content_hash(&bytes),
        error: None,
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

#[cfg(test)]
mod put_tests {
    use super::*;

    fn place(tag: &str) -> Place {
        let dir = std::env::temp_dir().join(format!("arbos-put-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".arbos")).unwrap();
        let place = Place::new(dir);
        arbos_core::store::ensure(&place).unwrap();
        place
    }

    fn written(f: Frame) -> (Option<String>, String, u64) {
        match f {
            Frame::Written {
                error, hash, size, ..
            } => (error, hash, size),
            other => panic!("expected written, got {other:?}"),
        }
    }

    /// A peer's write lands whole in the shared folders, with its hash;
    /// folders are made on the way; the file reads back by the same
    /// path.
    #[test]
    fn a_put_into_the_shared_folders_lands_whole_with_its_hash() {
        let p = place("ok");
        let (err, hash, size) = written(put(&p, "docs/research/voice.md", "# Voice\n", None));
        assert_eq!(err, None);
        assert_eq!(size, 8);
        assert_eq!(hash, arbos_core::hub::content_hash(b"# Voice\n"));
        assert_eq!(
            std::fs::read_to_string(p.arbos().join("docs/research/voice.md")).unwrap(),
            "# Voice\n"
        );
        match read(&p, "docs/research/voice.md") {
            Frame::File { text, .. } => assert_eq!(text, "# Voice\n"),
            other => panic!("{other:?}"),
        }
        // No temp file left beside it.
        let names: Vec<String> = std::fs::read_dir(p.arbos().join("docs/research"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec!["voice.md".to_string()]);
        for ok in ["internal/notes.md", "media/mesh/1.txt", ".arbos/docs/x.md"] {
            assert_eq!(written(put(&p, ok, "x", None)).0, None, "{ok}");
        }
        let _ = std::fs::remove_dir_all(&p.path);
    }

    /// Decision 1 (Jacob, 2026-09-15): a remote owner may not write
    /// another node's `notes.md`. Root-owned pages, protected files, the
    /// agents' folders, and anything outside the store are refused with
    /// the reason; the file is untouched.
    #[test]
    fn a_put_obeys_the_places_own_rules() {
        let p = place("rules");
        let before = std::fs::read_to_string(p.arbos().join("notes.md")).unwrap();
        for owned in [
            "notes.md",
            "docs/project-context.md",
            "archived.md",
            "GOALS.md",
        ] {
            let (err, ..) = written(put(&p, owned, "mine", None));
            let err = err.unwrap_or_default();
            assert!(err.contains("owned by the main chat"), "{owned}: {err}");
        }
        assert_eq!(
            std::fs::read_to_string(p.arbos().join("notes.md")).unwrap(),
            before
        );
        for protected in [
            "project.toml",
            "access.toml",
            "skills/x/SKILL.md",
            "PROTOCOL.md",
        ] {
            let (err, ..) = written(put(&p, protected, "x", None));
            let err = err.unwrap_or_default();
            assert!(
                err.contains("shapes how agents") || err.contains("not served"),
                "{protected}: {err}"
            );
            assert!(
                !p.arbos().join(protected).exists(),
                "{protected} was written"
            );
        }
        for outside in [
            "agents/root/plan.md",
            "kernel.json",
            "../src/main.rs",
            "/etc/x",
        ] {
            let (err, ..) = written(put(&p, outside, "x", None));
            assert!(err.is_some(), "{outside} was accepted");
        }
        let (err, ..) = written(put(&p, "docs", "x", None));
        assert!(err.unwrap_or_default().contains("is a folder"));
        let _ = std::fs::remove_dir_all(&p.path);
    }

    /// Compare-and-swap: a `base_hash` that no longer matches is a
    /// conflict that names the reason and returns the current hash, so
    /// the writer re-reads; `""` means "must not exist yet".
    #[test]
    fn a_put_with_a_stale_hash_is_a_conflict_not_a_lost_update() {
        let p = place("cas");
        let (_, first, _) = written(put(&p, "docs/a.md", "one", None));
        // Someone else writes in between.
        std::fs::write(p.arbos().join("docs/a.md"), "two").unwrap();
        let (err, current, _) = written(put(&p, "docs/a.md", "three", Some(&first)));
        assert!(
            err.unwrap_or_default()
                .contains("changed since you read it")
        );
        assert_eq!(current, arbos_core::hub::content_hash(b"two"));
        assert_eq!(
            std::fs::read_to_string(p.arbos().join("docs/a.md")).unwrap(),
            "two"
        );
        // The right hash goes through.
        let (err, ..) = written(put(&p, "docs/a.md", "three", Some(&current)));
        assert_eq!(err, None);
        // Create-only.
        let (err, ..) = written(put(&p, "docs/a.md", "four", Some("")));
        assert!(err.unwrap_or_default().contains("exists now"));
        let (err, ..) = written(put(&p, "docs/b.md", "new", Some("")));
        assert_eq!(err, None);
        // Deleted under the writer.
        std::fs::remove_file(p.arbos().join("docs/a.md")).unwrap();
        let (err, ..) = written(put(&p, "docs/a.md", "five", Some(&current)));
        assert!(err.unwrap_or_default().contains("no longer exists"));
        let _ = std::fs::remove_dir_all(&p.path);
    }
}
