use anyhow::{Context, Result, bail};
use fs4::fs_std::FileExt;
use std::{
    fs::{File, OpenOptions},
    path::PathBuf,
};

use crate::place::Place;

/// Exclusive writer for a place. A second serve fails. A viewer attach does
/// not take this lock.
pub struct PlaceLock {
    _file: File,
    path: PathBuf,
}

impl PlaceLock {
    pub fn acquire(place: &Place) -> Result<Self> {
        std::fs::create_dir_all(place.runtime_dir())?;
        let path = place.lock_path();
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
            Ok(false) => bail!("place already served ({})", path.display()),
            Err(err) => return Err(err).with_context(|| format!("lock {}", path.display())),
        }
        let pid = std::process::id();
        file.set_len(0)?;
        use std::io::Write;
        let mut file = file;
        writeln!(&mut file, "{pid}")?;
        file.sync_all()?;
        Ok(Self { _file: file, path })
    }
}

impl Drop for PlaceLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self._file);
        let _ = std::fs::remove_file(&self.path);
    }
}
