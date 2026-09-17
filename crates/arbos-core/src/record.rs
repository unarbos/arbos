//! Reads that know what they found, for the writes that act on them.
//!
//! The rule: **no destructive step — a `git reset`, a `clean`, a file
//! rewritten over its old bytes — on a record we have not confirmed.**
//!
//! Behind qal-j08, j09, j10 and j11 was one shape: a read helper that could
//! not tell *absent* from *unknown*. `read_to_string(..).unwrap_or_default()`
//! turns EIO, EACCES, a lock, a partial view on a mount, into `""`, and the
//! next writer acts on that emptiness with confidence — the project page
//! rewritten to one line, a checkpoint that carried HEAD alone, an `undo`
//! to a stale mark. A file that is not there is a fact (empty is the right
//! start); a file that could not be read is a question, and a question is
//! not a value.
//!
//! [`Read`] keeps the three apart. Readers that only display may still
//! settle for [`Read::present`]; anything that will *write over* or *reset
//! to* what it read must go through [`Read::confirmed`], which turns
//! `Unknown` into an error the caller surfaces instead of a default it
//! trusts. Writes that later readers will act on go through
//! [`write_atomic`], which fails loudly rather than leaving half a record.

use std::path::Path;

use anyhow::{Result, anyhow};

/// What a read found.
#[derive(Debug)]
pub enum Read<T> {
    /// The record exists and this is it.
    Present(T),
    /// The record does not exist (`NotFound`): a fact, and usually "start
    /// empty" is right.
    Absent,
    /// The record may exist but could not be read or parsed: not a value.
    /// Carries the reason, path included.
    Unknown(String),
}

impl<T> Read<T> {
    /// For a destructive step: the record, `None` when confirmed absent,
    /// and an error when unknown — never a default.
    pub fn confirmed(self) -> Result<Option<T>> {
        match self {
            Read::Present(v) => Ok(Some(v)),
            Read::Absent => Ok(None),
            Read::Unknown(why) => Err(anyhow!("{why} — nothing written or reset on its account")),
        }
    }

    /// For a display or a heuristic: the record when it was read, else
    /// nothing. A caller that uses this and then writes has skipped the
    /// rule; grep for it when a page goes missing.
    pub fn present(self) -> Option<T> {
        match self {
            Read::Present(v) => Some(v),
            _ => None,
        }
    }

    pub fn is_unknown(&self) -> bool {
        matches!(self, Read::Unknown(_))
    }

    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Read<U> {
        match self {
            Read::Present(v) => Read::Present(f(v)),
            Read::Absent => Read::Absent,
            Read::Unknown(w) => Read::Unknown(w),
        }
    }
}

/// The file's text; `Absent` when there is no such file.
pub fn read_text(path: &Path) -> Read<String> {
    match std::fs::read_to_string(path) {
        Ok(s) => Read::Present(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Read::Absent,
        Err(e) => Read::Unknown(format!("could not read {}: {e}", path.display())),
    }
}

/// The file parsed as one JSON value; a parse failure is `Unknown` (the
/// file is there, but its content is not a record we can act on).
pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Read<T> {
    match read_text(path) {
        Read::Present(text) => match serde_json::from_str(&text) {
            Ok(v) => Read::Present(v),
            Err(e) => Read::Unknown(format!("could not parse {}: {e}", path.display())),
        },
        Read::Absent => Read::Absent,
        Read::Unknown(w) => Read::Unknown(w),
    }
}

/// Write `bytes` as the whole file, through a sibling temp file and a
/// rename, so a reader never sees half of it and a failure leaves the old
/// file as it was. Errors name the path.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("{} has no parent directory", path.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|e| anyhow!("could not create {}: {e}", parent.display()))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "record".into());
    let tmp = parent.join(format!(".{name}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, bytes).map_err(|e| anyhow!("could not write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        anyhow!("could not replace {}: {e}", path.display())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_and_unknown_are_different_answers() {
        let dir = std::env::temp_dir().join(format!("arbos-record-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let missing = dir.join("missing.md");
        assert!(matches!(read_text(&missing), Read::Absent));
        assert!(read_text(&missing).confirmed().unwrap().is_none());

        let page = dir.join("page.md");
        std::fs::write(&page, "hello\n").unwrap();
        assert_eq!(
            read_text(&page).confirmed().unwrap().as_deref(),
            Some("hello\n")
        );

        // Unreadable, not absent: a directory where a file was expected.
        let unreadable = dir.join("dir");
        std::fs::create_dir_all(&unreadable).unwrap();
        let r = read_text(&unreadable);
        assert!(r.is_unknown(), "{r:?}");
        let err = read_text(&unreadable).confirmed().unwrap_err().to_string();
        assert!(
            err.contains("could not read") && err.contains("nothing written"),
            "{err}"
        );
        assert!(read_text(&unreadable).present().is_none());

        // Unparseable is unknown too.
        std::fs::write(&page, "not json").unwrap();
        assert!(read_json::<serde_json::Value>(&page).is_unknown());

        // An atomic write that cannot land leaves the old bytes.
        std::fs::write(&page, "old\n").unwrap();
        let blocked = dir.join("dir/file"); // parent is a dir we can use
        write_atomic(&blocked, b"x").unwrap();
        assert_eq!(std::fs::read_to_string(&blocked).unwrap(), "x");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
