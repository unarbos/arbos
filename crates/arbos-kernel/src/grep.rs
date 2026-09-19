use anyhow::Result;
use arbos_engine::{Grep, GrepHit};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
};
use tgrep_core::{
    builder::{self, BuildOptions},
    hybrid::HybridIndex,
    query,
    reader::IndexReader,
};

/// Searches share the index; only `upsert` and the initial build take the
/// write side, so parallel tool calls do not queue behind each other here.
pub struct PlaceGrep {
    root: PathBuf,
    inner: RwLock<Option<HybridIndex>>,
    ready: Mutex<bool>,
}

impl PlaceGrep {
    pub fn start(root: PathBuf) -> Arc<Self> {
        let index_dir = cache_dir(&root);
        let this = Arc::new(Self {
            root: root.clone(),
            inner: RwLock::new(None),
            ready: Mutex::new(false),
        });
        let boot = Arc::clone(&this);
        std::thread::spawn(move || {
            let _ = std::fs::create_dir_all(&index_dir);
            // Hidden files are in (`.github/`, dotfiles); the repository's
            // own `.git/` is not: its loose objects are most of the files
            // under an old project's root and none of them the project.
            let opts = BuildOptions {
                include_hidden: true,
                exclude_dirs: vec![".git".to_string()],
                ..Default::default()
            };
            let _ = builder::build_index_with_options(&root, Some(&index_dir), &opts);
            if let Ok(idx) = HybridIndex::open(&index_dir, &root) {
                *boot.inner.write().unwrap() = Some(idx);
                *boot.ready.lock().unwrap() = true;
            }
        });
        this
    }

    pub fn upsert(&self, rel: &str, content: &[u8]) {
        if let Some(idx) = self.inner.write().unwrap().as_mut() {
            idx.live.upsert_file(rel, content);
        }
    }
}

impl Grep for PlaceGrep {
    fn ready(&self) -> bool {
        *self.ready.lock().unwrap()
    }

    fn search(&self, pattern: &str, glob: Option<&str>) -> Result<Vec<GrepHit>> {
        let guard = self.inner.read().unwrap();
        let Some(idx) = guard.as_ref() else {
            return Ok(Vec::new());
        };
        let plan = query::build_query_plan(pattern, false)
            .unwrap_or_else(|_| query::build_literal_plan(pattern, false));
        let (ids, reader) = idx.execute_query_with_masks(&plan);
        let re = regex::Regex::new(pattern)
            .or_else(|_| regex::Regex::new(&regex::escape(pattern)))
            .unwrap();
        let file_glob = glob.and_then(|g| glob::Pattern::new(g).ok());
        let mut hits = Vec::new();
        for id in ids {
            let Some(path) = idx.resolve_full_path(id, reader.as_ref()) else {
                continue;
            };
            let rel = path
                .strip_prefix(&self.root)
                .unwrap_or(&path)
                .to_string_lossy()
                .into_owned();
            if let Some(g) = &file_glob {
                if !g.matches(&rel) {
                    continue;
                }
            }
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            let (text, _) = arbos_engine::decode_text(bytes);
            for (i, line) in text.lines().enumerate() {
                if re.is_match(line) {
                    hits.push(GrepHit {
                        path: rel.clone(),
                        line: i + 1,
                        text: line.to_string(),
                    });
                    if hits.len() >= 500 {
                        return Ok(hits);
                    }
                }
            }
        }
        let _ = IndexReader::empty();
        Ok(hits)
    }
}

fn cache_dir(root: &Path) -> PathBuf {
    let mut h = 0u64;
    for b in root.display().to_string().bytes() {
        h = h.wrapping_mul(16777619) ^ b as u64;
    }
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join("arbos").join("tgrep").join(format!("{h:016x}"))
}
