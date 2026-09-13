//! PDF input: the text of a PDF, for `read` and for a PDF the user attaches.
//! Extraction is a system tool (`pdftotext` from poppler, else `mutool`);
//! without one the model is told what to install instead of getting bytes.
//! Pages are marked so a citation can say `report.pdf p.3`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use anyhow::{Context, Result, bail};

/// Text beyond this is cut with a note; `read` with `offset`/`limit`
/// reaches the rest.
pub const ATTACHMENT_CAP_CHARS: usize = 40_000;

pub fn is_pdf_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
}

/// The document's text with `--- page N ---` markers, and the page count.
/// Cached by (path, mtime): a projection asks on every model step.
pub fn text(path: &Path) -> Result<(String, usize)> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, (SystemTime, String, usize)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mtime = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .with_context(|| format!("read {}", path.display()))?;
    if let Some((t, text, pages)) = cache.lock().unwrap().get(path)
        && *t == mtime
    {
        return Ok((text.clone(), *pages));
    }
    let raw = extract(path)?;
    let (text, pages) = mark_pages(&raw);
    cache
        .lock()
        .unwrap()
        .insert(path.to_path_buf(), (mtime, text.clone(), pages));
    Ok((text, pages))
}

fn extract(path: &Path) -> Result<String> {
    let file = path.display().to_string();
    if on_path("pdftotext") {
        let out = Command::new("pdftotext")
            .args(["-layout", "-enc", "UTF-8", &file, "-"])
            .stdin(std::process::Stdio::null())
            .output()
            .context("run pdftotext")?;
        if out.status.success() {
            return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
        }
        bail!(
            "pdftotext could not read {file}: {}",
            String::from_utf8_lossy(&out.stderr)
                .lines()
                .next()
                .unwrap_or("failed")
                .trim()
        );
    }
    if on_path("mutool") {
        let out = Command::new("mutool")
            .args(["draw", "-F", "txt", "-o", "-", &file])
            .stdin(std::process::Stdio::null())
            .output()
            .context("run mutool")?;
        if out.status.success() {
            return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
        }
        bail!(
            "mutool could not read {file}: {}",
            String::from_utf8_lossy(&out.stderr)
                .lines()
                .next()
                .unwrap_or("failed")
                .trim()
        );
    }
    bail!(
        "no PDF text tool on this machine: install poppler (`apt install poppler-utils` / `brew install poppler`) for pdftotext, or mupdf-tools for mutool; then read {file} again"
    )
}

/// pdftotext separates pages with a form feed; mutool too. Each becomes a
/// marker line, blank pages included so numbering stays true.
fn mark_pages(raw: &str) -> (String, usize) {
    let pages: Vec<&str> = raw.split('\u{0c}').collect();
    // A trailing form feed leaves an empty last element that is not a page.
    let pages: Vec<&str> = if pages.len() > 1 && pages.last().is_some_and(|p| p.trim().is_empty()) {
        pages[..pages.len() - 1].to_vec()
    } else {
        pages
    };
    let mut out = String::new();
    for (i, page) in pages.iter().enumerate() {
        out.push_str(&format!("--- page {} ---\n", i + 1));
        out.push_str(page.trim_end());
        out.push('\n');
    }
    (out, pages.len())
}

/// The attachment form: header, text under the cap, a note when cut.
pub fn attachment_text(path: &Path) -> String {
    match text(path) {
        Ok((text, pages)) => {
            let total = text.chars().count();
            let shown: String = text.chars().take(ATTACHMENT_CAP_CHARS).collect();
            let mut out = format!(
                "[pdf {} — {pages} page(s), {total} chars of text]\n{shown}",
                path.display()
            );
            if total > ATTACHMENT_CAP_CHARS {
                out.push_str(&format!(
                    "\n[… {} more chars; read {} with offset/limit for the rest]",
                    total - ATTACHMENT_CAP_CHARS,
                    path.display()
                ));
            }
            out
        }
        Err(e) => format!("[pdf {}: {e:#}]", path.display()),
    }
}

fn on_path(bin: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|d| d.join(bin).is_file()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_are_marked_and_counted() {
        let (text, pages) = mark_pages("first\u{0c}second\u{0c}");
        assert_eq!(pages, 2);
        assert!(
            text.starts_with("--- page 1 ---\nfirst\n--- page 2 ---\nsecond\n"),
            "{text}"
        );
        let (one, n) = mark_pages("only");
        assert_eq!(n, 1);
        assert_eq!(one, "--- page 1 ---\nonly\n");
    }

    #[test]
    fn pdf_paths_by_extension() {
        assert!(is_pdf_path(Path::new("a/report.PDF")));
        assert!(!is_pdf_path(Path::new("a/report.pdf.txt")));
    }
}
