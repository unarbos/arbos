//! Putting the new build where the old one was, and putting the old one back
//! when that goes wrong.
//!
//! The rule this file exists to keep: **there is no moment at which the app is
//! half installed.** A user who loses power, or a payload that unpacks into
//! something unrunnable, must end up with the app they had.
//!
//! So the work is two renames and nothing else. Renaming a directory within
//! one filesystem is atomic, which unpacking over the top of a live bundle is
//! not — and the staging and backup directories are deliberately made beside
//! the target so that both renames stay within one filesystem.
//!
//! 1. unpack the payload into a staging directory, beside the target
//! 2. look at what came out; refuse it here if it is not an Arbos
//! 3. rename the target aside to a backup
//! 4. rename the staged tree into the target's place
//! 5. look again, now that it is where it will run from
//! 6. delete the backup
//!
//! Anything that fails from step 3 on puts the backup back. [`Swap`] does that
//! from its `Drop`, so an early return, a `?`, or a panic unwinds into a
//! working app rather than into no app at all.
//!
//! A crash between the two renames is the one case no running code can catch,
//! and it leaves the backup on disk next to where the app should be.
//! [`recover`] is what the next launch calls to finish the job.

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

/// A swap in progress: the old tree is aside, the new tree is in place, and
/// nobody has yet said it worked.
///
/// Dropping one that was never committed puts the old tree back. That is the
/// whole design — every failure path after the first rename is an early
/// return, and every early return is a rollback.
#[derive(Debug)]
pub struct Swap {
    target: PathBuf,
    backup: PathBuf,
    committed: bool,
}

impl Swap {
    /// Move `staged` to `target`, keeping what was there.
    ///
    /// `staged` must be beside `target`: both renames have to be within one
    /// filesystem or neither is atomic.
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
        std::fs::rename(target, &backup).with_context(|| {
            format!(
                "could not move {} aside — is the app somewhere you can write to?",
                target.display()
            )
        })?;
        let swap = Self {
            target: target.to_path_buf(),
            backup,
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
        // Order matters: the new tree has to be out of the way before the old
        // one can have its name back.
        let _ = remove(&self.target);
        let _ = std::fs::rename(&self.backup, &self.target);
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

#[cfg(test)]
mod tests {
    use super::{Swap, check_tree, recover, staging_for, unpack};
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
