//! Reboot persistence: a systemd user unit on Linux, a launchd agent on macOS (spec §7).

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

use crate::config::{Paths, EXIT_KEY_REJECTED};

pub const LAUNCHD_LABEL: &str = "life.arbos.sidecar";
pub const SYSTEMD_UNIT: &str = "arbos-sidecar.service";

pub fn render_systemd(bin: &Path, home: Option<&Path>) -> String {
    let env = home
        .map(|h| format!("Environment=ARBOS_SIDECAR_HOME={}\n", h.display()))
        .unwrap_or_default();
    format!(
        "[Unit]\n\
         Description=Arbos sidecar (keeps your agent connected to Arbos)\n\
         After=network-online.target\n\
         Wants=network-online.target\n\
         \n\
         [Service]\n\
         ExecStart={bin} run\n\
         Restart=on-failure\n\
         RestartSec=2\n\
         RestartPreventExitStatus={EXIT_KEY_REJECTED}\n\
         {env}\
         Environment=RUST_LOG=info\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        bin = bin.display(),
    )
}

pub fn render_launchd(bin: &Path, log: &Path, home: Option<&Path>) -> String {
    let env = home
        .map(|h| {
            format!(
                "  <key>EnvironmentVariables</key>\n  <dict>\n    <key>ARBOS_SIDECAR_HOME</key>\n    <string>{}</string>\n    <key>RUST_LOG</key>\n    <string>info</string>\n  </dict>\n",
                h.display()
            )
        })
        .unwrap_or_else(|| {
            "  <key>EnvironmentVariables</key>\n  <dict>\n    <key>RUST_LOG</key>\n    <string>info</string>\n  </dict>\n".to_owned()
        });
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
         \x20 <key>Label</key>\n\
         \x20 <string>{LAUNCHD_LABEL}</string>\n\
         \x20 <key>ProgramArguments</key>\n\
         \x20 <array>\n\
         \x20   <string>{bin}</string>\n\
         \x20   <string>run</string>\n\
         \x20 </array>\n\
         \x20 <key>RunAtLoad</key>\n\
         \x20 <true/>\n\
         \x20 <key>KeepAlive</key>\n\
         \x20 <dict>\n\
         \x20   <key>SuccessfulExit</key>\n\
         \x20   <false/>\n\
         \x20 </dict>\n\
         \x20 <key>ThrottleInterval</key>\n\
         \x20 <integer>2</integer>\n\
         \x20 <key>StandardOutPath</key>\n\
         \x20 <string>{log}</string>\n\
         \x20 <key>StandardErrorPath</key>\n\
         \x20 <string>{log}</string>\n\
         {env}\
         </dict>\n\
         </plist>\n",
        bin = bin.display(),
        log = log.display(),
    )
}

fn home_override() -> Option<PathBuf> {
    std::env::var_os("ARBOS_SIDECAR_HOME").map(PathBuf::from)
}

fn systemd_unit_path() -> Result<PathBuf> {
    let base = dirs::config_dir().context("no config dir")?;
    Ok(base.join("systemd/user").join(SYSTEMD_UNIT))
}

fn launchd_plist_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("no home dir")?;
    Ok(home
        .join("Library/LaunchAgents")
        .join(format!("{LAUNCHD_LABEL}.plist")))
}

fn run(cmd: &str, args: &[&str]) -> Result<()> {
    let st = Command::new(cmd)
        .args(args)
        .status()
        .with_context(|| format!("running {cmd}"))?;
    if !st.success() {
        bail!("{cmd} {} exited with {st}", args.join(" "));
    }
    Ok(())
}

pub fn install(paths: &Paths) -> Result<String> {
    let bin = std::env::current_exe().context("current exe")?;
    let home = home_override();
    if cfg!(target_os = "macos") {
        let plist = launchd_plist_path()?;
        std::fs::create_dir_all(plist.parent().unwrap())?;
        std::fs::write(
            &plist,
            render_launchd(&bin, &paths.log_file(), home.as_deref()),
        )?;
        let uid = nix::unistd::getuid().as_raw();
        let _ = run(
            "launchctl",
            &["bootout", &format!("gui/{uid}/{LAUNCHD_LABEL}")],
        );
        run(
            "launchctl",
            &["bootstrap", &format!("gui/{uid}"), plist.to_str().unwrap()],
        )?;
        Ok(format!(
            "launchd agent installed: {} (launchctl print gui/{uid}/{LAUNCHD_LABEL})",
            plist.display()
        ))
    } else if cfg!(target_os = "linux") {
        let unit = systemd_unit_path()?;
        std::fs::create_dir_all(unit.parent().unwrap())?;
        std::fs::write(&unit, render_systemd(&bin, home.as_deref()))?;
        run("systemctl", &["--user", "daemon-reload"])?;
        run("systemctl", &["--user", "enable", "--now", SYSTEMD_UNIT])?;
        let user = std::env::var("USER").unwrap_or_else(|_| "$USER".into());
        Ok(format!(
            "systemd user unit installed: {} (systemctl --user status {SYSTEMD_UNIT}).\n\
             To keep it running when you are not logged in: sudo loginctl enable-linger {user}",
            unit.display()
        ))
    } else {
        bail!("service install is not supported on this OS yet; run `arbos-sidecar run` yourself")
    }
}

pub fn uninstall(paths: &Paths, purge: bool) -> Result<String> {
    let mut notes = Vec::new();
    if cfg!(target_os = "macos") {
        let uid = nix::unistd::getuid().as_raw();
        let _ = run(
            "launchctl",
            &["bootout", &format!("gui/{uid}/{LAUNCHD_LABEL}")],
        );
        if let Ok(p) = launchd_plist_path() {
            if p.exists() {
                std::fs::remove_file(&p)?;
                notes.push(format!("removed {}", p.display()));
            }
        }
    } else if cfg!(target_os = "linux") {
        let _ = run("systemctl", &["--user", "disable", "--now", SYSTEMD_UNIT]);
        if let Ok(p) = systemd_unit_path() {
            if p.exists() {
                std::fs::remove_file(&p)?;
                notes.push(format!("removed {}", p.display()));
            }
        }
        let _ = run("systemctl", &["--user", "daemon-reload"]);
    }
    let _ = std::fs::remove_file(paths.socket_file());
    let _ = std::fs::remove_file(paths.pid_file());
    if purge {
        for d in [&paths.config_dir, &paths.data_dir] {
            if d.exists() {
                std::fs::remove_dir_all(d)?;
                notes.push(format!("deleted {}", d.display()));
            }
        }
    } else {
        notes.push(format!(
            "kept the key and mirror under {} and {} (add --purge to delete them)",
            paths.config_dir.display(),
            paths.data_dir.display()
        ));
    }
    Ok(notes.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Buzz L18: both service files render with restart-on-failure, no restart on exit 78, and
    /// run-at-boot.
    #[test]
    fn service_files_render_for_linux_and_macos() {
        let bin = Path::new("/home/u/.cargo/bin/arbos-sidecar");
        let unit = render_systemd(bin, None);
        assert!(unit.contains("ExecStart=/home/u/.cargo/bin/arbos-sidecar run"));
        assert!(unit.contains("Restart=on-failure"));
        assert!(unit.contains("RestartPreventExitStatus=78"));
        assert!(unit.contains("WantedBy=default.target"));
        assert!(!unit.contains("arbk_"));
        let plist = render_launchd(bin, Path::new("/tmp/sidecar.log"), Some(Path::new("/x")));
        assert!(plist.contains("<string>life.arbos.sidecar</string>"));
        assert!(plist.contains("<key>RunAtLoad</key>\n  <true/>"));
        assert!(plist.contains("<key>SuccessfulExit</key>\n    <false/>"));
        assert!(plist.contains("ARBOS_SIDECAR_HOME"));
        assert!(plist.contains("<string>run</string>"));
    }
}
