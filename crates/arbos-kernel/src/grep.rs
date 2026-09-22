use anyhow::Result;
use arbos_engine::{Grep, GrepHit};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, Ordering},
    },
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
    /// The tree changed since the index was built (a tool wrote): `grep`
    /// walks until the rebuild lands. The index built at start never
    /// learned of a file written after it — `upsert` existed and was
    /// called from nowhere — and the agent's own edits were invisible to
    /// its own grep for the life of the kernel.
    stale: AtomicBool,
    /// Wakes the one rebuild thread.
    wake: Mutex<std::sync::mpsc::Sender<()>>,
}

/// How long the rebuild thread waits after a touch for more to land: a
/// turn's burst of edits is one rebuild, not one per file.
const REBUILD_SETTLE: std::time::Duration = std::time::Duration::from_millis(750);

impl PlaceGrep {
    pub fn start(root: PathBuf) -> Arc<Self> {
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let this = Arc::new(Self {
            root,
            inner: RwLock::new(None),
            ready: Mutex::new(false),
            stale: AtomicBool::new(false),
            wake: Mutex::new(tx),
        });
        let worker = Arc::clone(&this);
        std::thread::spawn(move || {
            worker.build();
            // Then: one rebuild per burst of touches, for as long as the
            // kernel lives.
            while rx.recv().is_ok() {
                std::thread::sleep(REBUILD_SETTLE);
                while rx.try_recv().is_ok() {}
                worker.build();
            }
        });
        this
    }

    /// Build the index and swap it in. `stale` is cleared as the build
    /// starts: a touch during the build sets it again and wakes another.
    fn build(&self) {
        self.stale.store(false, Ordering::SeqCst);
        let index_dir = cache_dir(&self.root);
        let _ = std::fs::create_dir_all(&index_dir);
        // Hidden files are in (`.github/`, dotfiles); the repository's
        // own `.git/` is not: its loose objects are most of the files
        // under an old project's root and none of them the project.
        let opts = BuildOptions {
            include_hidden: true,
            exclude_dirs: vec![".git".to_string()],
            ..Default::default()
        };
        let _ = builder::build_index_with_options(&self.root, Some(&index_dir), &opts);
        if let Ok(idx) = HybridIndex::open(&index_dir, &self.root) {
            *self.inner.write().unwrap() = Some(idx);
            *self.ready.lock().unwrap() = true;
        }
    }

    pub fn upsert(&self, rel: &str, content: &[u8]) {
        if let Some(idx) = self.inner.write().unwrap().as_mut() {
            idx.live.upsert_file(rel, content);
        }
    }
}

impl Grep for PlaceGrep {
    fn ready(&self) -> bool {
        *self.ready.lock().unwrap() && !self.stale.load(Ordering::SeqCst)
    }

    fn touched(&self) {
        self.stale.store(true, Ordering::SeqCst);
        let _ = self.wake.lock().unwrap().send(());
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
                let line = if i == 0 {
                    arbos_engine::without_bom(line)
                } else {
                    line
                };
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
