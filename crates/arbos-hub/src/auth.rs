//! Who may talk to the hub, and as what.
//!
//! `hub-server.toml` (path from `--config` or `ARBOS_HUB_CONFIG`):
//!
//! ```toml
//! bind = "127.0.0.1:7010"
//!
//! [[machine]]
//! name = "arboslife"                 # a kernel or worker registering as this machine
//! token_env = "ARBOS_HUB_TOKEN_ARBOSLIFE"   # or token = "…" (file mode 0600)
//!
//! [[client]]
//! name = "phone"                     # a client that only attaches
//! token = "…"
//! role = "owner"                     # owner | writer | reader
//! ```
//!
//! A machine token also attaches, as `machine:<name>` with the owner
//! role: one of Jacob's machines is Jacob. Later this file gains a
//! `trust = "cloudflare-access"` mode where the identity comes from the
//! `Cf-Access-Jwt-Assertion` header instead (see the file-system design).

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
struct MachineRow {
    name: String,
    #[serde(default)]
    token: Option<String>,
    #[serde(default)]
    token_env: Option<String>,
    /// Whose machine this is. Projects registered from it belong to this
    /// user for `[share] mode = "private"`. Default: the hub's operator.
    #[serde(default = "owner_user")]
    user: String,
}

#[derive(Debug, Clone, Deserialize)]
struct ClientRow {
    name: String,
    #[serde(default)]
    token: Option<String>,
    #[serde(default)]
    token_env: Option<String>,
    #[serde(default = "owner")]
    role: String,
    /// The person behind the token; `owner` is the hub's operator.
    #[serde(default = "owner_user")]
    user: String,
}

fn owner() -> String {
    "owner".into()
}

/// The hub operator's user name: what every row means when it names none.
pub const OWNER_USER: &str = "owner";

fn owner_user() -> String {
    OWNER_USER.into()
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(default)]
    bind: String,
    #[serde(default)]
    machine: Vec<MachineRow>,
    #[serde(default)]
    client: Vec<ClientRow>,
}

/// Who a token turned out to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Identity {
    /// A machine's own token: registers as `name`, attaches as owner.
    Machine { name: String, user: String },
    Client {
        name: String,
        role: String,
        user: String,
    },
}

impl Identity {
    /// The `who` and `role` the kernel is told when this identity attaches.
    pub fn as_client(&self) -> (String, String) {
        match self {
            Identity::Machine { name, .. } => (format!("machine:{name}"), "owner".into()),
            Identity::Client { name, role, .. } => (name.clone(), role.clone()),
        }
    }

    /// The person behind the token, for `[share] mode = "private"`.
    pub fn user(&self) -> &str {
        match self {
            Identity::Machine { user, .. } | Identity::Client { user, .. } => user,
        }
    }

    /// The token's role: a machine is an owner of its user's stores.
    pub fn role(&self) -> &str {
        match self {
            Identity::Machine { .. } => "owner",
            Identity::Client { role, .. } => role,
        }
    }
}

#[derive(Debug, Clone)]
struct Entry {
    token: String,
    identity: Identity,
}

#[derive(Debug, Default)]
pub struct Auth {
    pub bind: String,
    entries: Vec<Entry>,
}

impl Auth {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("read {} (see arbos-hub --help)", path.display()))?;
        Self::parse(&text, path)
    }

    fn parse(text: &str, path: &Path) -> Result<Self> {
        let file: File =
            toml::from_str(text).with_context(|| format!("{}: bad hub config", path.display()))?;
        let mut entries = Vec::new();
        for row in &file.machine {
            let token = resolve(
                &row.name,
                row.token.as_deref(),
                row.token_env.as_deref(),
                path,
            )?;
            entries.push(Entry {
                token,
                identity: Identity::Machine {
                    name: row.name.clone(),
                    user: row.user.clone(),
                },
            });
        }
        for row in &file.client {
            if !matches!(row.role.as_str(), "owner" | "writer" | "reader") {
                bail!(
                    "{}: client {:?}: role must be owner, writer, or reader",
                    path.display(),
                    row.name
                );
            }
            let token = resolve(
                &row.name,
                row.token.as_deref(),
                row.token_env.as_deref(),
                path,
            )?;
            entries.push(Entry {
                token,
                identity: Identity::Client {
                    name: row.name.clone(),
                    role: row.role.clone(),
                    user: row.user.clone(),
                },
            });
        }
        if entries.is_empty() {
            bail!(
                "{}: no [[machine]] or [[client]] rows; nothing could connect",
                path.display()
            );
        }
        Ok(Self {
            bind: file.bind,
            entries,
        })
    }

    /// The identity this token belongs to, compared in constant time.
    pub fn authenticate(&self, token: &str) -> Option<Identity> {
        let presented = token.trim().as_bytes();
        let mut hit = None;
        for e in &self.entries {
            if constant_time_eq(e.token.as_bytes(), presented) {
                hit = Some(e.identity.clone());
            }
        }
        hit
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

fn resolve(name: &str, token: Option<&str>, env: Option<&str>, path: &Path) -> Result<String> {
    let token = match (token.map(str::trim), env) {
        (Some(t), _) if !t.is_empty() => t.to_string(),
        (_, Some(var)) => match std::env::var(var) {
            Ok(v) if !v.trim().is_empty() => v.trim().to_string(),
            _ => bail!(
                "{}: {name:?} names token_env {var}, which is not set",
                path.display()
            ),
        },
        _ => bail!("{}: {name:?} needs token or token_env", path.display()),
    };
    if token.len() < 16 {
        bail!(
            "{}: {name:?}: a token needs at least 16 characters",
            path.display()
        );
    }
    Ok(token)
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let mut acc = u8::from(a.len() != b.len());
    for (x, y) in a.iter().zip(b.iter().chain(std::iter::repeat(&0))) {
        acc |= x ^ y;
    }
    acc == 0
}

/// The token in a request: `Authorization: Bearer …` or `?token=…`.
pub fn token_from_request(query: &str, authorization: Option<&str>) -> Option<String> {
    if let Some(auth) = authorization {
        let auth = auth.trim();
        if let Some(rest) = auth
            .strip_prefix("Bearer ")
            .or_else(|| auth.strip_prefix("bearer "))
        {
            return Some(rest.trim().to_string());
        }
    }
    query.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == "token").then(|| v.to_string())
    })
}
