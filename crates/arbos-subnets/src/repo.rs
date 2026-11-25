//! A subnet's repository, read without ever checking it out.
//!
//! The URL comes from the chain, where whoever registered the subnet typed it, and the contents
//! are a stranger's. So: only `https://github.com/<owner>/<repo>` is fetched, git may speak
//! nothing but https, the clone is bare (no working tree, so no symlink is followed and no file
//! of theirs is ever opened by path), and nothing in it is run.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use tokio::process::Command;

/// A hung or huge repository costs this long at most. Ceiling: a repository that cannot be
/// shallow-cloned in two minutes is skipped every cycle; raise it if a real subnet needs it.
const GIT_TIMEOUT: Duration = Duration::from_secs(120);
/// What one digest is written from.
const MAX_FILES: usize = 12;
const MAX_FILE_BYTES: u64 = 40_000;
const MAX_TOTAL_BYTES: u64 = 60_000;

/// `https://github.com/<owner>/<repo>` from what an identity gives, or nothing. Anything after
/// the repository (`/tree/main`, `.git`, a trailing slash) is dropped.
pub fn github(url: &str) -> Option<String> {
    let mut parts = url.trim().strip_prefix("https://github.com/")?.split('/');
    let owner = parts.next()?;
    let repo = parts.next()?;
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    let ok = |s: &str| {
        (1..=100).contains(&s.len())
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
            && !s.starts_with(['-', '.'])
    };
    (ok(owner) && ok(repo)).then(|| format!("https://github.com/{owner}/{repo}"))
}

async fn git(args: &[&str]) -> Result<Vec<u8>> {
    let run = Command::new("git")
        .args([
            "-c",
            "protocol.allow=never",
            "-c",
            "protocol.https.allow=always",
        ])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output();
    let out = tokio::time::timeout(GIT_TIMEOUT, run)
        .await
        .context("git timed out")??;
    if !out.status.success() {
        bail!(
            "git {}: {}",
            args[0],
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(out.stdout)
}

/// The commit the default branch is at, without fetching anything.
pub async fn head(url: &str) -> Result<String> {
    let out = git(&["ls-remote", "--", url, "HEAD"]).await?;
    let sha = String::from_utf8_lossy(&out)
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_owned();
    if sha.len() != 40 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("no HEAD");
    }
    Ok(sha)
}

/// Lower is read first: the root README, then anything about mining, then other docs.
fn rank(path: &str) -> Option<u8> {
    let p = path.to_ascii_lowercase();
    let name = p.rsplit('/').next().unwrap_or(&p);
    if name == "min_compute.yml" {
        return Some(1);
    }
    if !(p.ends_with(".md") || p.ends_with(".rst")) {
        return None;
    }
    Some(if !p.contains('/') && name.starts_with("readme") {
        0
    } else if p.contains("miner") || p.contains("mining") {
        1
    } else {
        2
    })
}

/// Which files a digest reads, from `git ls-tree -r -l -z` output: regular files only (a
/// symlink's mode is 120000), each under [`MAX_FILE_BYTES`], [`MAX_TOTAL_BYTES`] in all.
fn pick(tree: &str) -> Vec<String> {
    let mut found: Vec<(u8, &str, u64)> = tree
        .split('\0')
        .filter_map(|line| {
            let (meta, path) = line.split_once('\t')?;
            let mut meta = meta.split_whitespace();
            let (mode, kind, _sha, size) = (meta.next()?, meta.next()?, meta.next()?, meta.next()?);
            let size: u64 = size.parse().ok()?;
            (mode.starts_with("100") && kind == "blob" && size <= MAX_FILE_BYTES)
                .then(|| Some((rank(path)?, path, size)))?
        })
        .collect();
    found.sort();
    let mut total = 0;
    found
        .into_iter()
        .filter(|(_, _, size)| {
            total += size;
            total <= MAX_TOTAL_BYTES
        })
        .take(MAX_FILES)
        .map(|(_, path, _)| path.to_owned())
        .collect()
}

/// The repository's docs at its default branch, as one text: `## <path>` then the file.
pub async fn docs(url: &str, scratch: &Path) -> Result<String> {
    let _ = tokio::fs::remove_dir_all(scratch).await;
    let dir = scratch.to_str().context("scratch path")?;
    let read = async {
        git(&[
            "clone",
            "--bare",
            "--depth",
            "1",
            "--no-tags",
            "-q",
            "--",
            url,
            dir,
        ])
        .await?;
        let tree = git(&["--git-dir", dir, "ls-tree", "-r", "-l", "-z", "HEAD"]).await?;
        let mut text = String::new();
        for path in pick(&String::from_utf8_lossy(&tree)) {
            let blob = git(&["--git-dir", dir, "show", &format!("HEAD:{path}")]).await?;
            text.push_str(&format!(
                "## {path}\n{}\n\n",
                String::from_utf8_lossy(&blob)
            ));
        }
        Ok(text)
    }
    .await;
    let _ = tokio::fs::remove_dir_all(scratch).await;
    read
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_github_repository_is_a_repository() {
        let ok = Some("https://github.com/o-rg/re.po_1".to_owned());
        assert_eq!(github("https://github.com/o-rg/re.po_1"), ok);
        assert_eq!(github(" https://github.com/o-rg/re.po_1.git "), ok);
        assert_eq!(github("https://github.com/o-rg/re.po_1/tree/main"), ok);
        for bad in [
            "",
            "https://github.com/onlyowner",
            "http://github.com/o/r",
            "https://github.com.evil.io/o/r",
            "https://github.com/o/--upload-pack=x",
            "https://github.com/../r",
            "https://github.com/o/r?x=1",
            "https://github.com/o/r r",
            "ext::sh -c id",
            "file:///etc/passwd",
            "git@github.com:o/r.git",
        ] {
            assert_eq!(github(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_digest_reads_docs_and_never_a_symlink_or_a_big_file() {
        let tree = [
            "100644 blob a 900\tsrc/main.py",
            "100644 blob a 900\tdocs/other.md",
            "100644 blob a 900\tdocs/miner.md",
            "120000 blob a 20\tdocs/mining-link.md",
            "100644 blob a 900000\tdocs/mining-huge.md",
            "100644 blob a 900\tREADME.md",
            "160000 commit a -\tvendor/sub.md",
        ]
        .join("\0");
        assert_eq!(pick(&tree), ["README.md", "docs/miner.md", "docs/other.md"]);
    }
}
