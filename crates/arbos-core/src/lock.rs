use anyhow::{Context, Result, bail};
use fs4::fs_std::FileExt;
use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::PathBuf,
};

use crate::place::Place;

/// Exclusive writer for a place. A second serve fails. A viewer attach does
/// not take this lock.
///
/// Two files, both held: the legacy `.arbos/lock` that kernels before the
/// `runtime/` split take, and `.arbos/runtime/lock`. Holding only the new
/// one let an old kernel and a new kernel serve the same place at once,
/// each holding its own file and neither seeing the other — two writers
/// on one store through every update's mixed-version window. The legacy
/// file is taken first, so a new kernel contends with an old one where
/// the old one looks, and an old kernel started later finds its file
/// held.
#[derive(Debug)]
pub struct PlaceLock {
    _files: Vec<File>,
    paths: Vec<PathBuf>,
}

impl PlaceLock {
    pub fn acquire(place: &Place) -> Result<Self> {
        std::fs::create_dir_all(place.runtime_dir())?;
        let mut files = Vec::new();
        let mut paths = Vec::new();
        for path in place.lock_paths() {
            let file = OpenOptions::new()
                .create(true)
                .read(true)
                .write(true)
                .truncate(false)
                .open(&path)
                .with_context(|| format!("open {}", path.display()))?;
            // fs4 0.13 reports a held lock as `Ok(false)`, not as an error.
            match file.try_lock_exclusive() {
                Ok(true) => {}
                // Dropping what was taken so far releases it.
                Ok(false) => bail!("place already served ({})", path.display()),
                Err(err) => return Err(err).with_context(|| format!("lock {}", path.display())),
            }
            files.push(file);
            paths.push(path);
        }
        let pid = std::process::id();
        for file in &mut files {
            file.set_len(0)?;
            writeln!(file, "{pid}")?;
            file.sync_all()?;
        }
        Ok(Self {
            _files: files,
            paths,
        })
    }

    /// The lock files are gone from under `place_now` — the folder this
    /// kernel's store is at *now*, after the project folder was renamed
    /// under it. `Drop` removes by the paths the lock was taken at, which
    /// after a move name nothing of ours; the flock is the truth and the
    /// files are advisory, but a lock file left behind reads to the next
    /// kernel and to `check` as a holder that is not there. Only a file
    /// that is our own (same device and inode as the one we hold) goes.
    pub fn release_at(&self, place_now: &Place) {
        for (file, path) in self._files.iter().zip(place_now.lock_paths()) {
            if same_file(file, &path) {
                let _ = std::fs::remove_file(&path);
            }
        }
    }

    /// Whether every lock file at `place`'s paths is still the very file
    /// this holder has open. A flock lives on the inode, not the path:
    /// `rm -rf .arbos/runtime .arbos/lock` under a running kernel leaves
    /// its locks on unlinked inodes — held, unreachable, and protecting
    /// nothing — and the next kernel creates fresh files at the same
    /// paths, locks those, and serves the same place (qal-j40). The
    /// holder is the one that can notice, on the tick it already uses to
    /// look at its store.
    pub fn still_at(&self, place: &Place) -> bool {
        self._files
            .iter()
            .zip(place.lock_paths())
            .all(|(file, path)| same_file(file, &path))
    }

    /// Take back every lock file that is no longer ours at `place`'s
    /// paths — fresh file, fresh flock, our pid — keeping the ones still
    /// held. `Err` when another kernel already holds a fresh file: the
    /// place has two kernels, and the caller must not serve on.
    pub fn retake(&mut self, place: &Place) -> Result<()> {
        std::fs::create_dir_all(place.runtime_dir())?;
        let pid = std::process::id();
        for (i, path) in place.lock_paths().into_iter().enumerate() {
            if same_file(&self._files[i], &path) {
                continue;
            }
            let mut file = OpenOptions::new()
                .create(true)
                .read(true)
                .write(true)
                .truncate(false)
                .open(&path)
                .with_context(|| format!("open {}", path.display()))?;
            match file.try_lock_exclusive() {
                Ok(true) => {}
                Ok(false) => bail!(
                    "another kernel holds {} (pid {})",
                    path.display(),
                    std::fs::read_to_string(&path)
                        .ok()
                        .and_then(|t| t.trim().parse::<u32>().ok())
                        .map(|p| p.to_string())
                        .unwrap_or_else(|| "unknown".into())
                ),
                Err(err) => return Err(err).with_context(|| format!("lock {}", path.display())),
            }
            file.set_len(0)?;
            writeln!(file, "{pid}")?;
            file.sync_all()?;
            // The old descriptor's flock is on an unlinked inode; letting
            // it go frees nothing anyone can reach.
            let _ = FileExt::unlock(&self._files[i]);
            self._files[i] = file;
            self.paths[i] = path;
        }
        Ok(())
    }

    /// The pid written in whichever lock file names one — the holder, for a
    /// refusal that says who has the place.
    pub fn holder_pid(place: &Place) -> Option<u32> {
        place
            .lock_paths()
            .iter()
            .find_map(|p| std::fs::read_to_string(p).ok()?.trim().parse::<u32>().ok())
    }
}

impl Drop for PlaceLock {
    fn drop(&mut self) {
        for (file, path) in self._files.iter().zip(&self.paths) {
            let _ = FileExt::unlock(file);
            // Only our own file: after the project folder was renamed
            // under this kernel, the path may hold another project's lock
            // — a new folder made where ours was — and removing that would
            // take a lock file from a kernel that is serving.
            if same_file(file, path) {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

/// Whether the file at `path` is the very file `held` has open: same
/// device and inode. False when nothing is at the path, or something else
/// is.
fn same_file(held: &File, path: &std::path::Path) -> bool {
    let (Ok(ours), Ok(there)) = (held.metadata(), std::fs::metadata(path)) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ours.dev() == there.dev() && ours.ino() == there.ino()
    }
    #[cfg(not(unix))]
    {
        let _ = (ours, there);
        true
    }
}

#[cfg(test)]
mod own_file_tests {
    use super::*;

    fn fresh(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("arbos-lock-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The project folder renamed under a running kernel, and a new
    /// project made where it was: the kernel's `Drop` must not take the
    /// new project's lock file, and `release_at` takes ours where they
    /// went — by inode, never by path alone.
    #[test]
    fn drop_removes_only_our_own_file_and_release_at_finds_it_where_the_folder_went() {
        let root = fresh("moved");
        let old = Place::new(root.join("p"));
        std::fs::create_dir_all(old.arbos()).unwrap();
        let lock = PlaceLock::acquire(&old).unwrap();
        for p in old.lock_paths() {
            assert!(p.exists());
        }

        // The folder moves; another kernel's project appears at the old
        // path with lock files of its own.
        let moved = Place::new(root.join("p-moved"));
        std::fs::rename(old.path(), moved.path()).unwrap();
        std::fs::create_dir_all(old.runtime_dir()).unwrap();
        for p in old.lock_paths() {
            std::fs::write(&p, "4242\n").unwrap();
        }

        // Ours, where they are now.
        lock.release_at(&moved);
        for p in moved.lock_paths() {
            assert!(
                !p.exists(),
                "our lock file taken from the moved store: {}",
                p.display()
            );
        }
        // Drop looks at the old paths and finds a stranger's files: kept.
        drop(lock);
        for p in old.lock_paths() {
            assert_eq!(
                std::fs::read_to_string(&p).unwrap().trim(),
                "4242",
                "another project's lock file was left alone: {}",
                p.display()
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The ordinary case is unchanged: a lock dropped in place takes its
    /// files with it.
    #[test]
    fn drop_in_place_removes_the_files_as_before() {
        let root = fresh("in-place");
        let place = Place::new(root.join("p"));
        std::fs::create_dir_all(place.arbos()).unwrap();
        let lock = PlaceLock::acquire(&place).unwrap();
        drop(lock);
        for p in place.lock_paths() {
            assert!(!p.exists(), "{}", p.display());
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
