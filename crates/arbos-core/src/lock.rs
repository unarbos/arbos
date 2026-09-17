use anyhow::{Context, Result, bail};
use fs4::fs_std::FileExt;
use std::{
    fs::{File, OpenOptions},
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
            use std::io::Write;
            writeln!(file, "{pid}")?;
            file.sync_all()?;
        }
        Ok(Self {
            _files: files,
            paths,
        })
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
            let _ = std::fs::remove_file(path);
        }
    }
}
