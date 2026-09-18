//! Putting the new build where the old one was, and putting the old one back
//! when that goes wrong.
//!
//! The rule this file exists to keep: **there is no moment at which the app is
//! half installed.** A user who loses power, or a payload that unpacks into
//! something unrunnable, must end up with the app they had.
//!
//! There is a second rule underneath it, which cost a real fault to learn:
//! **the path never stops resolving.** Not "is restored quickly" — never
//! stops. Anything replacing a live installation has readers, and a reader
//! that looks while the path is empty does not conclude "it is being
//! replaced"; it concludes the file is gone, and acts on that.
//!
//! This file used to do the work as two renames: the old tree aside, then the
//! new tree in. Between them the path resolved to nothing. A kernel's update
//! tick landing in that interval found no binary and waited its full minute
//! for a file that was already back — the fault [#453] makes the kernel side
//! patient about, and the window this side now does not open.
//!
//! So the swap is **one step** wherever the system offers one:
//! `renamex_np(RENAME_SWAP)` on macOS, `renameat2(RENAME_EXCHANGE)` on Linux.
//! The path resolves to the old tree before it and the new tree after it, and
//! to nothing in between only if the machine cannot do either — see [`Swap`]
//! for the ladder and what each rung costs.
//!
//! 1. unpack the payload into a staging directory, beside the target
//! 2. look at what came out; refuse it here if it is not an Arbos
//! 3. exchange the staged tree with the target, in one step
//! 4. look again, now that it is where it will run from
//! 5. delete the old tree
//!
//! Anything that fails from step 3 on puts the old tree back. [`Swap`] does
//! that from its `Drop`, so an early return, a `?`, or a panic unwinds into a
//! working app rather than into no app at all.
//!
//! [`recover`] exists for the one rung of the ladder that still has an
//! interval — a directory on a filesystem with no exchange — where a crash
//! leaves the backup on disk next to where the app should be.
//!
//! [#453]: https://github.com/unarbos/arbos/pull/453

use crate::feed::Format;
use anyhow::{Context, Result, bail};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

/// The suffix the old tree is set aside under, and the one [`recover`] looks
/// for.
const BACKUP_SUFFIX: &str = ".arbos-old";

/// And the one the new tree is unpacked into.
const STAGING_SUFFIX: &str = ".arbos-new";

/// How the old tree got to where it is, which is how it goes back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum How {
    /// Target and staged tree changed places in one step. The path never
    /// stopped resolving, and putting it back is the same step again.
    Exchanged,
    /// The old file was given a second name, then the new one was renamed
    /// over the top — which for a file replaces it in one step. The path
    /// never stopped resolving either.
    Linked,
    /// The old tree was renamed aside and the new one renamed in. There is
    /// an interval between those two in which the path resolves to
    /// nothing. Only reached for a directory on a filesystem with no
    /// exchange.
    Asided,
}

/// A swap in progress: the new tree is in place, the old one is kept, and
/// nobody has yet said it worked.
///
/// Dropping one that was never committed puts the old tree back. That is the
/// whole design — every failure path after the swap is an early return, and
/// every early return is a rollback.
///
/// # Why this is not just a rename
///
/// A rename cannot replace a directory that has anything in it, so a bundle
/// cannot simply be renamed over. The obvious way round that is to move the
/// old one aside first, and it is what this did — at the cost of an interval
/// in which the path resolved to nothing. That interval is not theoretical:
/// it is the fault behind [`#453`](https://github.com/unarbos/arbos/pull/453),
/// where a kernel's update tick looked during it, found no binary, and waited
/// a minute for a file that was already back.
///
/// So there is a ladder, and only its bottom rung has the interval:
///
/// 1. **Exchange.** `renamex_np(RENAME_SWAP)` on macOS,
///    `renameat2(RENAME_EXCHANGE)` on Linux. Both paths change places in one
///    step; a reader sees the old tree or the new one and nothing else.
/// 2. **Link, then rename.** For a single file, which is what a kernel
///    binary is. A hard link gives the old file a second name without
///    touching the first, and a rename onto an existing file replaces it
///    atomically. Also no interval.
/// 3. **Aside, then in.** A directory on a filesystem that offers no
///    exchange. Two renames, and the interval is back — narrow, but real,
///    which is what [`recover`] is for.
#[derive(Debug)]
pub struct Swap {
    target: PathBuf,
    /// Where the tree that was at `target` is now.
    backup: PathBuf,
    how: How,
    committed: bool,
}

impl Swap {
    /// Put `staged` at `target`, keeping what was there.
    ///
    /// `staged` must be beside `target`: an exchange, a hard link and a
    /// rename all need the two to be on one filesystem.
    pub fn begin(target: &Path, staged: &Path) -> Result<Self> {
        if !target.exists() {
            bail!("nothing at {} to replace", target.display());
        }
        if !staged.exists() {
            bail!("nothing staged at {}", staged.display());
        }
        let backup = sibling(target, BACKUP_SUFFIX)?;
        // A backup from an attempt that died is not evidence of anything: the
        // app is running, so what is at `target` is what works.
        if backup.exists() {
            remove(&backup)?;
        }

        // Rung 1. The exchange leaves the old tree where the new one was
        // staged. Give it the backup name, so that every rung leaves it in
        // the same place and so that what sits beside the app is hidden
        // whatever the caller chose to call its staging path — on a Mac an
        // unhidden second `.app` is one Launch Services will register and
        // ⌘Space may open. The swap has already happened by then, so a
        // failure here costs the tidier name and nothing else.
        // Only "this system has no exchange" walks on. An attempt that
        // failed for its own reasons has already been retried, and taking
        // the windowed rung because of one would give up atomicity on a
        // machine that has it.
        let offered = match exchange(target, staged) {
            Ok(()) => Ok(()),
            Err(e) if exchange_unsupported(&e) => Err(e),
            Err(e) => {
                return Err(e).with_context(|| {
                    format!(
                        "exchanging {} with {} — the system offers an exchange and this one \
                         would not complete, so the swap was not made rather than made \
                         with a gap in it",
                        staged.display(),
                        target.display()
                    )
                });
            }
        };
        if offered.is_ok() {
            let backup = match std::fs::rename(staged, &backup) {
                Ok(()) => backup,
                Err(_) => staged.to_path_buf(),
            };
            return Ok(Self {
                target: target.to_path_buf(),
                backup,
                how: How::Exchanged,
                committed: false,
            });
        }

        // Rung 2. `hard_link` fails on a directory, and on a filesystem
        // with no links, which is exactly when to fall through.
        if target.is_file() && std::fs::hard_link(target, &backup).is_ok() {
            let swap = Self {
                target: target.to_path_buf(),
                backup,
                how: How::Linked,
                committed: false,
            };
            std::fs::rename(staged, target).with_context(|| {
                format!("could not move the new build into {}", target.display())
            })?;
            return Ok(swap);
        }

        // Rung 3, with the interval. `recover` is what finishes this if the
        // machine stops between the two renames.
        std::fs::rename(target, &backup).with_context(|| {
            format!(
                "could not move {} aside — is the app somewhere you can write to?",
                target.display()
            )
        })?;
        let swap = Self {
            target: target.to_path_buf(),
            backup,
            how: How::Asided,
            committed: false,
        };
        // From here on, `swap` going out of scope puts the old tree back.
        std::fs::rename(staged, target)
            .with_context(|| format!("could not move the new build into {}", target.display()))?;
        Ok(swap)
    }

    /// The new tree stays. Deletes the old one.
    ///
    /// Failing to delete the backup is not a failure of the update: the new
    /// app is in place and runnable. The stray directory is left, and
    /// [`recover`] steps over it next time because the target exists.
    pub fn commit(mut self) -> Result<()> {
        self.committed = true;
        remove(&self.backup)
    }
}

impl Drop for Swap {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        match self.how {
            // Back the way it came, and the path does not stop resolving on
            // the way back either — a rollback is the moment least able to
            // afford a second failure. The exchange puts the refused tree
            // at the backup name, where it is no more wanted than it was.
            How::Exchanged => {
                if exchange(&self.target, &self.backup).is_ok() {
                    let _ = remove(&self.backup);
                }
            }
            // Order matters: the new tree has to be out of the way before
            // the old one can have its name back.
            How::Linked | How::Asided => {
                let _ = remove(&self.target);
                let _ = std::fs::rename(&self.backup, &self.target);
            }
        }
    }
}

/// Make `a` and `b` change places in one step.
///
/// Both must exist. The point is not speed but that there is no observable
/// moment between: a reader of either path sees what was there before or
/// what is there after, never nothing and never a half-written tree.
///
/// `Unsupported` where the system has no such call, and the operating
/// system's error where it has one and it failed — an old kernel, or a
/// filesystem that does not implement the flag. Callers fall down the
/// ladder rather than treating it as fatal.
/// Whether a failed exchange means *this system cannot do one*, as against
/// *this attempt did not work*.
///
/// The difference decides whether to walk down the ladder, and getting it
/// wrong is how a swap silently loses its atomicity: a one-off `EBUSY` read
/// as "no exchange here" drops to the rung that has a window, on a machine
/// that could have done it properly. One swap in forty took that path on a
/// CI runner on 2026-09-18 — the exchange was available, the rung-1
/// assertion passed in the same run — and a reader saw the installed path
/// missing once in 4190 looks.
///
/// So only the errors that describe the *system* count: the call is not
/// there, the filesystem does not implement the flag. Everything else is
/// this attempt's problem and is retried.
fn exchange_unsupported(e: &std::io::Error) -> bool {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        if let Some(code) = e.raw_os_error() {
            // ENOTSUP and EOPNOTSUPP are the same number on Linux, so one
            // arm covers both.
            return code == libc::ENOSYS
                || code == libc::EINVAL
                || code == libc::ENOTSUP
                || code == libc::EXDEV;
        }
    }
    e.kind() == std::io::ErrorKind::Unsupported
}

/// Put `a` and `b` in each other's place, waiting out an attempt that
/// failed for a reason of the moment.
///
/// Retried for the same reason `--version` is: the alternative is not "try
/// again later" but "quietly do something weaker", and the weaker thing
/// here is the one interval this file exists to remove.
fn exchange(a: &Path, b: &Path) -> std::io::Result<()> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        match exchange_once(a, b) {
            Ok(()) => return Ok(()),
            Err(e)
                if !exchange_unsupported(&e) && std::time::Instant::now() < deadline =>
            {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            Err(e) => return Err(e),
        }
    }
}

fn exchange_once(a: &Path, b: &Path) -> std::io::Result<()> {
    // A machine with an exchange never walks down the ladder, so without
    // this the rungs below would be written and never run. Thread-local
    // and test-only: tests share a process, and a switch that could turn
    // off the thing this file exists for has no business in a release.
    #[cfg(test)]
    if no_exchange::is_set() {
        let _ = (a, b);
        return Err(std::io::ErrorKind::Unsupported.into());
    }
    // A failure of the moment, as the runner produced one: EBUSY says
    // nothing about whether the system can exchange.
    #[cfg(test)]
    if no_exchange::take_transient() {
        let _ = (a, b);
        return Err(std::io::Error::from_raw_os_error(libc::EBUSY));
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let (Ok(a), Ok(b)) = (
            CString::new(a.as_os_str().as_bytes()),
            CString::new(b.as_os_str().as_bytes()),
        ) else {
            return Err(std::io::ErrorKind::InvalidInput.into());
        };
        #[cfg(target_os = "macos")]
        // SAFETY: two valid NUL-terminated paths, and a flag the call defines.
        let rc = unsafe { libc::renamex_np(a.as_ptr(), b.as_ptr(), libc::RENAME_SWAP) };
        #[cfg(target_os = "linux")]
        let rc = {
            // `RENAME_EXCHANGE`. Stable kernel ABI since 3.15 and not in
            // `libc` for this target. Called through `syscall` rather than
            // the wrapper, which needs glibc 2.28 to link.
            const RENAME_EXCHANGE: libc::c_uint = 2;
            // SAFETY: as above; `syscall` returns -1 and sets errno on
            // failure, including when the kernel does not know the number.
            unsafe {
                libc::syscall(
                    libc::SYS_renameat2,
                    libc::AT_FDCWD,
                    a.as_ptr(),
                    libc::AT_FDCWD,
                    b.as_ptr(),
                    RENAME_EXCHANGE,
                ) as libc::c_int
            }
        };
        match rc {
            0 => Ok(()),
            _ => Err(std::io::Error::last_os_error()),
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (a, b);
        Err(std::io::ErrorKind::Unsupported.into())
    }
}

/// Finish a swap that a crash interrupted.
///
/// Called on launch. There is exactly one state worth repairing: a backup
/// beside a target that is not there, which is the instant between the two
/// renames. Anything else — no backup, or a backup beside a target that does
/// exist — is either a healthy install or a commit that did not get to delete
/// its backup, and both are left alone.
///
/// Returns whether it put something back.
pub fn recover(target: &Path) -> Result<bool> {
    let backup = sibling(target, BACKUP_SUFFIX)?;
    if !backup.exists() {
        return Ok(false);
    }
    if target.exists() {
        // A committed swap that could not delete its backup. Tidy it away.
        let _ = remove(&backup);
        return Ok(false);
    }
    std::fs::rename(&backup, target)
        .with_context(|| format!("restoring {} after an interrupted update", target.display()))?;
    Ok(true)
}

/// Where the payload is unpacked: beside the target, so the rename that
/// follows is a rename and not a copy across filesystems.
pub fn staging_for(target: &Path) -> Result<PathBuf> {
    sibling(target, STAGING_SUFFIX)
}

/// Unpack `payload` next to `target` and return the tree that will replace it.
///
/// Both archives carry one directory at the top — `Arbos.app` from `ditto -c -k
/// --keepParent`, `arbos-0.2.0-1877-linux-x86_64` from the Linux tarball — so
/// what comes back is that directory and not the staging directory around it.
pub fn unpack(payload: &Path, format: Format, target: &Path) -> Result<PathBuf> {
    let staging = staging_for(target)?;
    if staging.exists() {
        remove(&staging)?;
    }
    std::fs::create_dir_all(&staging).with_context(|| format!("making {}", staging.display()))?;

    let status = match format {
        // `ditto`, not `unzip`: it is the only unpacker that restores a code
        // signature, the symlinks inside a framework, and the extended
        // attributes a bundle carries. `unzip` produces a bundle macOS refuses
        // to launch.
        Format::Zip => Command::new("/usr/bin/ditto")
            .arg("-x")
            .arg("-k")
            .arg(payload)
            .arg(&staging)
            .status(),
        Format::TarGz => Command::new("tar")
            .arg("-xzf")
            .arg(payload)
            .arg("-C")
            .arg(&staging)
            .status(),
    };
    let status = status.with_context(|| format!("unpacking {}", payload.display()))?;
    if !status.success() {
        let _ = remove(&staging);
        bail!("the download would not unpack ({status})");
    }

    let mut entries: Vec<PathBuf> = std::fs::read_dir(&staging)
        .with_context(|| format!("reading {}", staging.display()))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        // `ditto` leaves `__MACOSX` beside the bundle when the archive carries
        // resource forks. It is not the app.
        .filter(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_none_or(|name| name != "__MACOSX" && !name.starts_with('.'))
        })
        .collect();
    entries.sort();
    match entries.as_slice() {
        [one] => Ok(one.clone()),
        [] => {
            let _ = remove(&staging);
            bail!("the download unpacked to nothing")
        }
        many => {
            let _ = remove(&staging);
            bail!("the download unpacked to {} things, not one", many.len())
        }
    }
}

/// Launch Services' own register. Not on `PATH`, and not somewhere Apple
/// promises it will stay, so every use of it is guarded.
const LSREGISTER: &str = "/System/Library/Frameworks/CoreServices.framework/Frameworks/\
     LaunchServices.framework/Support/lsregister";

/// Tell macOS the app at this path is new, so ⌘Space finds it now rather than
/// whenever the system next gets round to looking.
///
/// Two systems have to be told, and they are not the same one. Launch Services
/// is the register of what applications exist — it is what resolves a bundle
/// identifier to a bundle, and so what actually decides which Arbos opens;
/// Spotlight's index is what matches the name typed into the search field.
/// Replacing a bundle in place updates both eventually, and eventually is not
/// what "⌘Space, arbos, return" means to somebody who just pressed Update.
///
/// Both are best-effort. Neither failing is a reason to call an installed
/// update a failed one, and on the next login both happen anyway.
pub fn reindex(target: &Path) {
    if cfg!(not(target_os = "macos")) {
        return;
    }
    if Path::new(LSREGISTER).is_file() {
        let _ = Command::new(LSREGISTER).arg("-f").arg(target).status();
    }
    let _ = Command::new("/usr/bin/mdimport").arg(target).status();
}

/// Take a bundle out of Launch Services' register.
///
/// Every copy of Arbos carries one `CFBundleIdentifier`, and Launch Services
/// answers "open `life.arbos.desktop`" with whichever registered copy it
/// likes. That is not a hypothetical: on Jacob's Mac five bundles claimed the
/// identifier — build outputs and disk-image staging folders — and ⌘Space
/// opened a stale scratch build instead of the installed app.
///
/// So every bundle this module is about to delete is unregistered first. A
/// directory that is gone should stop answering on its own, and "should" is
/// how the five got there.
fn forget(path: &Path) {
    if cfg!(not(target_os = "macos")) || !Path::new(LSREGISTER).is_file() {
        return;
    }
    let _ = Command::new(LSREGISTER).arg("-u").arg(path).status();
    // A staging directory holds the bundle rather than being one.
    if let Ok(entries) = std::fs::read_dir(path) {
        for bundle in entries
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|path| path.extension().is_some_and(|e| e == "app"))
        {
            let _ = Command::new(LSREGISTER).arg("-u").arg(bundle).status();
        }
    }
}

/// Whether an unpacked tree is an Arbos that will run: the executable is
/// there, and so is the kernel beside it.
///
/// Checked before the swap and again after it. The kernel ships inside the
/// bundle and the two are replaced together, so a payload missing one of them
/// is a payload that would leave the app talking to a kernel from a different
/// build.
pub fn check_tree(root: &Path, executable: &str, kernel: &str) -> Result<()> {
    // `Arbos.app/Contents/MacOS/Arbos` on a Mac, `arbos-desktop` in the
    // directory itself on Linux.
    let dir = match root.join("Contents/MacOS").is_dir() {
        true => root.join("Contents/MacOS"),
        false => root.to_path_buf(),
    };
    for (what, name) in [("the app", executable), ("its kernel", kernel)] {
        let path = dir.join(name);
        if !path.is_file() {
            bail!(
                "the download has no {what} in it ({} is missing)",
                path.display()
            );
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path)
                .with_context(|| format!("reading {}", path.display()))?
                .permissions()
                .mode();
            if mode & 0o111 == 0 {
                bail!("{} came out of the download unrunnable", path.display());
            }
        }
    }
    Ok(())
}

/// `/Applications/Arbos.app` → `/Applications/.Arbos.app.arbos-old`.
///
/// Beside the target, because both renames have to stay within one
/// filesystem. Hidden — the leading dot — because for the second or two that
/// the swap takes, the directory it is in is `/Applications`, and neither
/// Finder nor Spotlight should ever see two things called Arbos there.
fn sibling(target: &Path, suffix: &str) -> Result<PathBuf> {
    let name = target
        .file_name()
        .and_then(|n| n.to_str())
        .with_context(|| format!("{} has no name", target.display()))?;
    let parent = target
        .parent()
        .with_context(|| format!("{} has nowhere beside it", target.display()))?;
    Ok(parent.join(format!(".{name}{suffix}")))
}

/// Delete a tree, and take it out of Launch Services' register on the way.
///
/// Every path this module deletes is, or holds, a copy of the app — a backup,
/// a staging tree, a rolled-back install. Unregistering before deleting is
/// what keeps an update from leaving behind a second bundle that can answer
/// for `life.arbos.desktop`, which is the bug this whole dance exists around.
fn remove(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    forget(path);
    let removed = match path.is_dir() {
        true => std::fs::remove_dir_all(path),
        false => std::fs::remove_file(path),
    };
    removed.with_context(|| format!("removing {}", path.display()))
}

/// Pretending, on this thread only, that the system has no exchange.
#[cfg(test)]
mod no_exchange {
    use std::cell::Cell;
    thread_local! {
        static OFF: Cell<bool> = const { Cell::new(false) };
    }
    pub(super) fn is_set() -> bool {
        OFF.with(Cell::get)
    }
    /// Restores itself when it goes out of scope, so a failing assertion
    /// cannot leave the rest of the thread's tests on a lower rung.
    /// How many of the next attempts should fail with a reason of the
    /// moment rather than a reason about the system. Lets a test produce
    /// the exact thing that cost the swap its atomicity on CI.
    thread_local! {
        pub(super) static TRANSIENT: Cell<u32> = const { Cell::new(0) };
    }
    pub(super) fn take_transient() -> bool {
        TRANSIENT.with(|n| {
            let left = n.get();
            if left > 0 {
                n.set(left - 1);
                true
            } else {
                false
            }
        })
    }

    pub(super) struct Off;
    impl Off {
        pub(super) fn new() -> Self {
            OFF.with(|f| f.set(true));
            Self
        }
    }
    impl Drop for Off {
        fn drop(&mut self) {
            OFF.with(|f| f.set(false));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{How, Swap, check_tree, no_exchange, recover, staging_for, unpack};
    use crate::feed::Format;
    use std::{
        fs,
        path::{Path, PathBuf},
    };

    /// A tree that looks enough like an installed Arbos to swap.
    fn tree(at: &Path, marker: &str) -> PathBuf {
        fs::create_dir_all(at).unwrap();
        fs::write(at.join("arbos-desktop"), format!("app {marker}")).unwrap();
        fs::write(at.join("arbos-kernel"), format!("kernel {marker}")).unwrap();
        executable(&at.join("arbos-desktop"));
        executable(&at.join("arbos-kernel"));
        at.to_path_buf()
    }

    fn executable(path: &Path) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    fn marker_of(at: &Path) -> String {
        fs::read_to_string(at.join("arbos-desktop")).unwrap()
    }

    fn check(root: &Path) -> anyhow::Result<()> {
        check_tree(root, "arbos-desktop", "arbos-kernel")
    }

    #[test]
    fn the_new_build_takes_the_old_ones_place() {
        let home = tempfile::tempdir().unwrap();
        let target = tree(&home.path().join("Arbos.app"), "old");
        let staged = tree(&home.path().join("staged"), "new");

        let swap = Swap::begin(&target, &staged).unwrap();
        check(&target).unwrap();
        swap.commit().unwrap();

        assert_eq!(marker_of(&target), "app new");
        assert!(!staged.exists(), "the staged tree moved rather than copied");
        // Nothing left behind: no backup, no staging.
        let left: Vec<_> = fs::read_dir(home.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(left, ["Arbos.app"], "{left:?}");
    }

    #[test]
    fn a_payload_that_fails_its_check_leaves_the_old_app_exactly_as_it_was() {
        // The case the whole file exists for. The new tree is in place when
        // the check runs, and the check says no.
        let home = tempfile::tempdir().unwrap();
        let target = tree(&home.path().join("Arbos.app"), "old");
        let staged = home.path().join("staged");
        fs::create_dir_all(&staged).unwrap();
        fs::write(staged.join("arbos-desktop"), "app new").unwrap();
        executable(&staged.join("arbos-desktop"));
        // No kernel: the app would come up talking to the old build's kernel.

        let installed = (|| -> anyhow::Result<()> {
            let swap = Swap::begin(&target, &staged)?;
            check(&target)?;
            swap.commit()
        })();

        let err = installed.unwrap_err().to_string();
        assert!(err.contains("kernel"), "{err}");
        assert_eq!(marker_of(&target), "app old", "the old app must be back");
        assert!(target.join("arbos-kernel").is_file());
        let left: Vec<_> = fs::read_dir(home.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(left, ["Arbos.app"], "nothing left behind: {left:?}");
    }

    #[test]
    fn dropping_a_swap_that_was_never_committed_rolls_it_back() {
        // Every early return between the two renames and the commit is this.
        let home = tempfile::tempdir().unwrap();
        let target = tree(&home.path().join("Arbos.app"), "old");
        let staged = tree(&home.path().join("staged"), "new");
        {
            let _swap = Swap::begin(&target, &staged).unwrap();
            assert_eq!(marker_of(&target), "app new", "in place mid-swap");
        }
        assert_eq!(marker_of(&target), "app old");
    }

    #[test]
    fn a_panic_mid_install_still_leaves_a_working_app() {
        let home = tempfile::tempdir().unwrap();
        let target = tree(&home.path().join("Arbos.app"), "old");
        let staged = tree(&home.path().join("staged"), "new");
        let at = target.clone();
        let panicked = std::panic::catch_unwind(move || {
            let _swap = Swap::begin(&at, &staged).unwrap();
            panic!("the power went out");
        });
        assert!(panicked.is_err());
        assert_eq!(marker_of(&target), "app old");
    }

    #[test]
    fn refuses_to_replace_something_that_is_not_there() {
        let home = tempfile::tempdir().unwrap();
        let staged = tree(&home.path().join("staged"), "new");
        let missing = home.path().join("Arbos.app");
        let err = Swap::begin(&missing, &staged).unwrap_err().to_string();
        assert!(err.contains("to replace"), "{err}");
        assert!(staged.exists(), "a refused swap moves nothing");

        let target = tree(&home.path().join("Other.app"), "old");
        let err = Swap::begin(&target, &home.path().join("nope"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("staged"), "{err}");
        assert_eq!(marker_of(&target), "app old");
    }

    #[test]
    fn the_app_keeps_its_own_path_and_leaves_no_second_arbos_beside_it() {
        // ⌘Space has to find it: the app is replaced where it stands, so its
        // path, its name and its `.app` extension are the ones they were, and
        // what is beside it while the swap runs is hidden and then gone.
        let home = tempfile::tempdir().unwrap();
        let applications = home.path().join("Applications");
        let target = tree(&applications.join("Arbos.app"), "old");
        let staged = tree(&applications.join("staged"), "new");

        let swap = Swap::begin(&target, &staged).unwrap();
        let visible: Vec<String> = fs::read_dir(&applications)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| !name.starts_with('.'))
            .collect();
        assert_eq!(visible, ["Arbos.app"], "mid-swap: {visible:?}");
        swap.commit().unwrap();

        assert!(target.is_dir(), "still at {}", target.display());
        assert_eq!(marker_of(&target), "app new");
        let left: Vec<String> = fs::read_dir(&applications)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(left, ["Arbos.app"], "afterwards: {left:?}");
    }

    /// The guard that cannot be wrong without failing. Where the system
    /// offers a one-step exchange, the swap must take it — because the
    /// alternative is a rung with an interval, and the interval is the
    /// fault. A regression that quietly drops back to two renames would
    /// pass every behavioural test here and still reopen #453.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn the_swap_is_one_step_where_the_system_offers_one() {
        let home = tempfile::tempdir().unwrap();
        let target = tree(&home.path().join("Arbos.app"), "old");
        let staged = tree(&home.path().join(".Arbos.app.arbos-new"), "new");

        let swap = Swap::begin(&target, &staged).unwrap();
        assert_eq!(
            swap.how,
            How::Exchanged,
            "this filesystem has an atomic exchange and the swap did not use it"
        );
        assert_eq!(marker_of(&target), "app new");
        assert_eq!(marker_of(&swap.backup), "app old", "the old tree is kept");
        swap.commit().unwrap();
        assert_eq!(marker_of(&target), "app new");
    }

    /// The failure the CI runner actually produced, made to happen.
    ///
    /// One swap in forty took the windowed rung on a machine that had an
    /// exchange — the rung-1 assertion passed in the same run — and a
    /// reader saw the installed path missing once in 4190 looks. The cause
    /// was `is_ok()`: any error at all read as "no exchange here".
    ///
    /// A transient must be waited out, not answered by quietly doing the
    /// weaker thing.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn a_failure_of_the_moment_does_not_cost_the_swap_its_atomicity() {
        let home = tempfile::tempdir().unwrap();
        let target = tree(&home.path().join("Arbos.app"), "old");
        let staged = tree(&home.path().join(".Arbos.app.arbos-new"), "new");

        // The first three attempts fail with EBUSY, as the runner's did.
        no_exchange::TRANSIENT.with(|n| n.set(3));
        let swap = Swap::begin(&target, &staged).unwrap();
        assert_eq!(
            swap.how,
            How::Exchanged,
            "a transient sent the swap down to a rung with a window"
        );
        assert_eq!(marker_of(&target), "app new");
        swap.commit().unwrap();
        no_exchange::TRANSIENT.with(|n| n.set(0));
    }

    /// And a transient that never clears is a refusal, not a quiet
    /// downgrade: better no swap than a swap with a gap in it.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn an_exchange_that_keeps_failing_refuses_rather_than_widening() {
        let home = tempfile::tempdir().unwrap();
        let target = tree(&home.path().join("Arbos.app"), "old");
        let staged = tree(&home.path().join(".Arbos.app.arbos-new"), "new");

        no_exchange::TRANSIENT.with(|n| n.set(u32::MAX));
        let err = Swap::begin(&target, &staged).unwrap_err().to_string();
        no_exchange::TRANSIENT.with(|n| n.set(0));
        assert!(err.contains("gap in it"), "{err}");
        assert_eq!(marker_of(&target), "app old", "the old app is untouched");
    }

    /// The property itself, watched rather than reasoned about.
    ///
    /// Honest about what it is: an observer sampling as fast as it can,
    /// so it could miss a narrow interval rather than prove there is
    /// none. The test above is the one that fails by itself; this one
    /// says what the path actually looked like to somebody reading it,
    /// which is how the fault was met in the first place — a kernel's
    /// update tick, not a proof.
    #[test]
    fn a_reader_watching_the_path_never_sees_it_missing() {
        use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
        use std::sync::Arc;

        let home = tempfile::tempdir().unwrap();
        let target = tree(&home.path().join("Arbos.app"), "old");
        let bin = target.join("arbos-kernel");

        let stop = Arc::new(AtomicBool::new(false));
        let missing = Arc::new(AtomicU64::new(0));
        let looks = Arc::new(AtomicU64::new(0));
        let watcher = {
            let (stop, missing, looks, bin) =
                (stop.clone(), missing.clone(), looks.clone(), bin.clone());
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    // What a reader asks: is there a binary at that path?
                    if !bin.is_file() {
                        missing.fetch_add(1, Ordering::Relaxed);
                    }
                    looks.fetch_add(1, Ordering::Relaxed);
                }
            })
        };

        for i in 0..40 {
            let staged = tree(&home.path().join(".Arbos.app.arbos-new"), &format!("{i}"));
            Swap::begin(&target, &staged).unwrap().commit().unwrap();
        }
        stop.store(true, Ordering::Relaxed);
        watcher.join().unwrap();

        assert!(
            looks.load(Ordering::Relaxed) > 0,
            "the observer never got to look"
        );
        assert_eq!(
            missing.load(Ordering::Relaxed),
            0,
            "the path stopped resolving during a swap, after {} looks over 40 swaps",
            looks.load(Ordering::Relaxed)
        );
    }

    /// A kernel binary is a single file, and a file can be replaced with
    /// no interval even where there is no exchange: give the old one a
    /// second name, then rename over it. This is the rung that matters
    /// most, because the file a kernel polls is a file.
    #[test]
    fn a_file_is_replaced_with_no_interval_even_without_an_exchange() {
        let _off = no_exchange::Off::new();
        let home = tempfile::tempdir().unwrap();
        let target = home.path().join("arbos-kernel");
        fs::write(&target, "old").unwrap();
        let staged = home.path().join(".arbos-kernel.arbos-new");
        fs::write(&staged, "new").unwrap();

        let swap = Swap::begin(&target, &staged).unwrap();
        assert_eq!(swap.how, How::Linked, "a file should not need the aside");
        assert_eq!(fs::read_to_string(&target).unwrap(), "new");
        assert_eq!(fs::read_to_string(&swap.backup).unwrap(), "old");
        swap.commit().unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "new");
    }

    /// And the same rung rolls back.
    #[test]
    fn a_file_swap_without_an_exchange_still_rolls_back() {
        let _off = no_exchange::Off::new();
        let home = tempfile::tempdir().unwrap();
        let target = home.path().join("arbos-kernel");
        fs::write(&target, "old").unwrap();
        let staged = home.path().join(".arbos-kernel.arbos-new");
        fs::write(&staged, "new").unwrap();

        drop(Swap::begin(&target, &staged).unwrap());
        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            "old",
            "an uncommitted swap puts the old file back"
        );
    }

    /// The bottom rung, which a directory on a filesystem with no
    /// exchange still falls to. It has the interval, and `recover` is
    /// what covers it — so this checks the swap is correct and that the
    /// interval is the *only* thing it gives up.
    #[test]
    fn a_directory_without_an_exchange_still_swaps_and_rolls_back() {
        let _off = no_exchange::Off::new();
        let home = tempfile::tempdir().unwrap();
        let target = tree(&home.path().join("Arbos.app"), "old");
        let staged = tree(&home.path().join(".Arbos.app.arbos-new"), "new");

        let swap = Swap::begin(&target, &staged).unwrap();
        assert_eq!(swap.how, How::Asided);
        assert_eq!(marker_of(&target), "app new");
        swap.commit().unwrap();
        assert_eq!(marker_of(&target), "app new");

        let staged = tree(&home.path().join(".Arbos.app.arbos-new"), "newer");
        drop(Swap::begin(&target, &staged).unwrap());
        assert_eq!(marker_of(&target), "app new", "rolled back");
    }

    #[test]
    fn a_crash_between_the_two_renames_is_repaired_on_the_next_launch() {
        // No running code can catch this one: the process is gone between the
        // rename that moves the old app aside and the one that puts the new
        // app in its place. What is left is a backup and no app.
        let home = tempfile::tempdir().unwrap();
        let target = home.path().join("Arbos.app");
        tree(&target, "old");
        let backup = home.path().join(".Arbos.app.arbos-old");
        fs::rename(&target, &backup).unwrap();
        assert!(!target.exists());

        assert!(recover(&target).unwrap(), "it should have put one back");
        assert_eq!(marker_of(&target), "app old");
        assert!(!backup.exists());
    }

    #[test]
    fn recovery_leaves_a_healthy_install_alone() {
        let home = tempfile::tempdir().unwrap();
        let target = tree(&home.path().join("Arbos.app"), "current");
        assert!(!recover(&target).unwrap());
        assert_eq!(marker_of(&target), "app current");

        // A commit that could not delete its backup: the app is fine, and the
        // leftovers are tidied rather than restored over the top of it.
        let backup = tree(&home.path().join(".Arbos.app.arbos-old"), "stale");
        assert!(!recover(&target).unwrap());
        assert_eq!(marker_of(&target), "app current");
        assert!(!backup.exists());
    }

    #[test]
    fn a_stale_backup_does_not_stop_the_next_update() {
        let home = tempfile::tempdir().unwrap();
        let target = tree(&home.path().join("Arbos.app"), "old");
        tree(&home.path().join(".Arbos.app.arbos-old"), "older still");
        let staged = tree(&home.path().join("staged"), "new");

        let swap = Swap::begin(&target, &staged).unwrap();
        check(&target).unwrap();
        swap.commit().unwrap();
        assert_eq!(marker_of(&target), "app new");
    }

    #[test]
    fn unpacks_a_tarball_and_finds_the_one_tree_in_it() {
        let home = tempfile::tempdir().unwrap();
        let target = tree(&home.path().join("Arbos.app"), "old");
        let source = home.path().join("source");
        tree(&source.join("arbos-0.2.0-1878-linux-x86_64"), "new");
        let payload = home.path().join("payload.tar.gz");
        assert!(
            std::process::Command::new("tar")
                .args(["-czf".as_ref(), payload.as_os_str()])
                .arg("-C")
                .arg(&source)
                .arg("arbos-0.2.0-1878-linux-x86_64")
                .status()
                .unwrap()
                .success()
        );

        let staged = unpack(&payload, Format::TarGz, &target).unwrap();
        assert_eq!(marker_of(&staged), "app new");
        check(&staged).unwrap();
        // Beside the target, or the rename that follows would be a copy.
        assert!(staged.starts_with(staging_for(&target).unwrap()));

        let swap = Swap::begin(&target, &staged).unwrap();
        check(&target).unwrap();
        swap.commit().unwrap();
        assert_eq!(marker_of(&target), "app new");
    }

    #[test]
    fn refuses_a_download_that_is_not_an_archive() {
        let home = tempfile::tempdir().unwrap();
        let target = tree(&home.path().join("Arbos.app"), "old");
        let payload = home.path().join("payload.tar.gz");
        fs::write(&payload, "this is not a tarball").unwrap();
        let err = unpack(&payload, Format::TarGz, &target)
            .unwrap_err()
            .to_string();
        assert!(err.contains("unpack"), "{err}");
        assert!(!staging_for(&target).unwrap().exists(), "no mess left");
        assert_eq!(marker_of(&target), "app old");
    }

    #[test]
    fn checks_a_mac_bundles_insides_where_they_actually_are() {
        let home = tempfile::tempdir().unwrap();
        let app = home.path().join("Arbos.app");
        tree(&app.join("Contents/MacOS"), "mac");
        fs::rename(
            app.join("Contents/MacOS/arbos-desktop"),
            app.join("Contents/MacOS/Arbos"),
        )
        .unwrap();
        check_tree(&app, "Arbos", "arbos-kernel").unwrap();
        let err = check_tree(&app, "Arbos", "arbos-hub")
            .unwrap_err()
            .to_string();
        assert!(err.contains("kernel"), "{err}");
    }

    #[test]
    fn refuses_a_tree_whose_binary_cannot_run() {
        let home = tempfile::tempdir().unwrap();
        let root = tree(&home.path().join("tree"), "new");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                root.join("arbos-desktop"),
                fs::Permissions::from_mode(0o644),
            )
            .unwrap();
            let err = check(&root).unwrap_err().to_string();
            assert!(err.contains("unrunnable"), "{err}");
        }
    }
}
