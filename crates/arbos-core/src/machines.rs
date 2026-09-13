//! `~/.config/arbos/machines.toml`: the other computers an agent may work
//! on. Today each is reached over SSH and gets one dedicated directory;
//! when the hub lands (see the file-system design, "Remote attach and
//! sharing") a machine gains a `hub` address and the same entry serves.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Machine {
    /// Short name agents use: `spawn host=<name>`.
    pub name: String,
    /// `user@host` or an alias from `~/.ssh/config`.
    pub ssh: String,
    pub port: Option<u16>,
    /// Private key: a path, or an `op://` reference the kernel reads once
    /// into `~/.config/arbos/keys/<name>` (mode 600).
    pub key: Option<String>,
    /// The one directory Arbos uses on that machine. Projects are synced to
    /// `<dir>/<project>/`, the kernel binary to `<dir>/bin/`.
    pub dir: String,
    /// Path of `arbos-kernel` there. Empty = `<dir>/bin/arbos-kernel`,
    /// installed from this machine when missing and the architecture matches.
    pub kernel: String,
    /// Free words for the roster: "linux", "x86_64", "gpu", "macos", "ios".
    pub tags: Vec<String>,
    pub note: String,
    /// Reserved for the hub route: `wss://host/p/<id>`.
    pub hub: Option<String>,
    /// Build `arbos-kernel` from source on that machine when no binary of
    /// this machine's architecture can be copied over. Needs cargo and a C
    /// compiler there. Off by default: the window refuses and says where
    /// to put a binary instead.
    #[serde(default)]
    pub build: bool,
}

impl Default for Machine {
    fn default() -> Self {
        Self {
            name: String::new(),
            ssh: String::new(),
            port: None,
            key: None,
            dir: String::new(),
            kernel: String::new(),
            tags: Vec::new(),
            note: String::new(),
            hub: None,
            build: false,
        }
    }
}

impl Machine {
    /// The label `ssh` sees.
    pub fn target(&self) -> &str {
        &self.ssh
    }

    pub fn kernel_path(&self) -> String {
        if self.kernel.trim().is_empty() {
            format!("{}/bin/arbos-kernel", self.dir.trim_end_matches('/'))
        } else {
            self.kernel.clone()
        }
    }

    /// `XDG_CONFIG_HOME` for the kernels there: `<dir>/config`, so the
    /// model config lives inside the dedicated directory too.
    pub fn config_home(&self) -> String {
        format!("{}/config", self.dir.trim_end_matches('/'))
    }

    /// Where a project named `project` lives on this machine.
    pub fn place_for(&self, project: &str) -> String {
        format!("{}/{project}", self.dir.trim_end_matches('/'))
    }

    /// One child's own copy of `project`: `<dir>/<project>--<child>`. Two
    /// dashes, so a project whose name has one dash still reads.
    pub fn place_for_child(&self, project: &str, child: &str) -> String {
        format!("{}/{project}--{child}", self.dir.trim_end_matches('/'))
    }

    /// One roster line for the prompt.
    pub fn describe(&self) -> String {
        let mut s = format!("{} (ssh {}", self.name, self.ssh);
        if !self.tags.is_empty() {
            s.push_str("; ");
            s.push_str(&self.tags.join(", "));
        }
        s.push(')');
        if !self.note.trim().is_empty() {
            s.push_str(" — ");
            s.push_str(self.note.trim());
        }
        s
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Machines {
    pub machine: Vec<Machine>,
}

impl Machines {
    pub fn path() -> PathBuf {
        crate::host_dir().join("machines.toml")
    }

    /// Absent file = no machines. A malformed one is an error the caller
    /// shows; it is the user's file.
    pub fn load() -> Result<Self> {
        let path = Self::path();
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Ok(Self::default());
        };
        let m: Self = toml::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
        for machine in &m.machine {
            if machine.name.trim().is_empty()
                || machine.ssh.trim().is_empty()
                || machine.dir.trim().is_empty()
            {
                anyhow::bail!(
                    "{}: every [[machine]] needs name, ssh, and dir",
                    path.display()
                );
            }
        }
        Ok(m)
    }

    pub fn get(&self, name: &str) -> Option<&Machine> {
        let q = name.trim();
        self.machine
            .iter()
            .find(|m| m.name.eq_ignore_ascii_case(q) || m.ssh == q)
    }

    /// The prompt's machine roster, or None when there is only this one.
    pub fn roster(&self) -> Option<String> {
        if self.machine.is_empty() {
            return None;
        }
        Some(format!(
            "Machines: this one (local); {}. spawn host=<name> runs a child on that machine, in its own copy of this project.",
            self.machine
                .iter()
                .map(Machine::describe)
                .collect::<Vec<_>>()
                .join("; ")
        ))
    }
}
