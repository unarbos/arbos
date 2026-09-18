//! Who may attach from off the machine, and as what.
//!
//! The attach socket binds loopback by default and trusts every plain-TCP
//! loopback peer (the desktop, the CLI). A WebSocket peer is never trusted
//! by address: a tunnel daemon on this machine (cloudflared) connects from
//! loopback on behalf of someone far away. `--bind 0.0.0.0:7001` (or
//! `ARBOS_ATTACH_BIND`) opens it to the network for a phone behind a
//! Cloudflare tunnel; then every non-loopback peer must present a token
//! from `<place>/.arbos/access.toml` before the kernel says hello:
//!
//! ```toml
//! [[client]]
//! name = "phone"
//! token_env = "ARBOS_PHONE_TOKEN"   # or token = "…" (file mode 0600)
//! role = "writer"                    # owner | writer | reader
//! ```
//!
//! The file has the shape `docs/filesystem-state-design.md` gives
//! `runtime/access.toml`: `[[person]]` rows (an email the hub verifies
//! through Cloudflare Access) are parsed and kept for that day; today only
//! `[[client]]` tokens can log in, because the kernel has nothing but the
//! socket to identify a peer by. When the hub arrives it sits in front of
//! this socket and presents the verified identity; the roles stay.
//!
//! Roles: `owner` and `writer` may send every frame; `reader` may only ask
//! for history and read files under `.arbos/` (`read`, `tail`, `list`). Fail closed: a non-loopback bind with no client tokens
//! refuses to start.

use std::net::{IpAddr, SocketAddr};
use std::path::Path;

use anyhow::{Context, Result, bail};
use arbos_core::Place;
use arbos_core::wire::Frame;
use serde::Deserialize;

/// Env var carrying `--bind`.
pub const BIND_ENV: &str = "ARBOS_ATTACH_BIND";
/// Where the kernel listens when nothing says otherwise.
pub const DEFAULT_BIND: &str = "127.0.0.1:0";
/// How long a remote peer has to present its token.
pub const AUTH_TIMEOUT_SECS: u64 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Owner,
    Writer,
    Reader,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Owner => "owner",
            Role::Writer => "writer",
            Role::Reader => "reader",
        }
    }

    /// Whether this role may send `frame`. Readers get history and the
    /// login itself; everything else changes state on the host.
    pub fn allows(self, frame: &Frame) -> bool {
        match self {
            Role::Owner => true,
            // A writer may not hand the kernel a model key.
            Role::Writer => !matches!(frame, Frame::Configure { .. }),
            Role::Reader => matches!(
                frame,
                Frame::History { .. }
                    | Frame::Auth { .. }
                    | Frame::Read { .. }
                    | Frame::Tail { .. }
                    | Frame::List { .. }
                    | Frame::TurnChanges { .. }
            ),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct ClientRow {
    name: String,
    #[serde(default)]
    token: Option<String>,
    #[serde(default)]
    token_env: Option<String>,
    #[serde(default = "writer")]
    role: Role,
}

fn writer() -> Role {
    Role::Writer
}

/// Kept for the hub: an email Cloudflare Access (or the token file in
/// `token` mode) will vouch for. Not usable over the bare socket.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
struct PersonRow {
    email: String,
    #[serde(default = "writer")]
    role: Role,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct AccessFile {
    #[serde(default)]
    client: Vec<ClientRow>,
    #[serde(default)]
    person: Vec<PersonRow>,
}

/// A resolved client: name, role, and the secret it must present.
#[derive(Debug, Clone)]
pub struct Client {
    pub name: String,
    pub role: Role,
    token: String,
}

/// Who a connection turned out to be.
#[derive(Debug, Clone)]
pub struct Identity {
    pub name: String,
    pub role: Role,
}

impl Identity {
    pub fn local() -> Self {
        Self {
            name: "local".into(),
            role: Role::Owner,
        }
    }
}

#[derive(Debug, Default)]
pub struct Access {
    clients: Vec<Client>,
    persons: usize,
}

impl Access {
    pub fn path(place: &Place) -> std::path::PathBuf {
        place.arbos().join("access.toml")
    }

    /// Read `access.toml`; a missing file is an empty table.
    pub fn load(place: &Place) -> Result<Self> {
        let path = Self::path(place);
        if !path.exists() {
            return Ok(Self::default());
        }
        Self::parse(&std::fs::read_to_string(&path)?, &path)
    }

    fn parse(text: &str, path: &Path) -> Result<Self> {
        let file: AccessFile =
            toml::from_str(text).with_context(|| format!("{}: bad access.toml", path.display()))?;
        let mut clients = Vec::new();
        for row in file.client {
            let token = match (&row.token, &row.token_env) {
                (Some(t), _) if !t.trim().is_empty() => t.trim().to_string(),
                (_, Some(var)) => match std::env::var(var) {
                    Ok(v) if !v.trim().is_empty() => v.trim().to_string(),
                    _ => bail!(
                        "{}: client {:?} names token_env {var}, which is not set",
                        path.display(),
                        row.name
                    ),
                },
                _ => bail!(
                    "{}: client {:?} needs token or token_env",
                    path.display(),
                    row.name
                ),
            };
            if token.len() < 16 {
                bail!(
                    "{}: client {:?}: a token needs at least 16 characters",
                    path.display(),
                    row.name
                );
            }
            clients.push(Client {
                name: row.name,
                role: row.role,
                token,
            });
        }
        Ok(Self {
            clients,
            persons: file.person.len(),
        })
    }

    pub fn has_clients(&self) -> bool {
        !self.clients.is_empty()
    }

    pub fn client_count(&self) -> usize {
        self.clients.len()
    }

    pub fn person_count(&self) -> usize {
        self.persons
    }

    /// The client this token belongs to, compared in constant time.
    pub fn authenticate(&self, token: &str) -> Option<Identity> {
        let presented = token.trim().as_bytes();
        let mut hit = None;
        for c in &self.clients {
            if constant_time_eq(c.token.as_bytes(), presented) {
                hit = Some(Identity {
                    name: c.name.clone(),
                    role: c.role,
                });
            }
        }
        hit
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let mut acc = u8::from(a.len() != b.len());
    for (x, y) in a.iter().zip(b.iter().chain(std::iter::repeat(&0))) {
        acc |= x ^ y;
    }
    acc == 0
}

/// The address to bind: `--bind` (via the env), else loopback.
pub fn bind_addr() -> Result<SocketAddr> {
    let raw = std::env::var(BIND_ENV)
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_BIND.to_string());
    let raw = raw.trim();
    // `0.0.0.0` alone, or `:7001`, or a bare port, are all one idea.
    let candidate = if let Ok(port) = raw.parse::<u16>() {
        format!("0.0.0.0:{port}")
    } else if let Some(port) = raw.strip_prefix(':') {
        format!("0.0.0.0:{port}")
    } else if raw.parse::<IpAddr>().is_ok() {
        format!("{raw}:0")
    } else {
        raw.to_string()
    };
    candidate
        .parse::<SocketAddr>()
        .with_context(|| format!("{BIND_ENV}={raw:?}: not host:port"))
}

pub fn is_local(peer: &SocketAddr) -> bool {
    peer.ip().is_loopback()
}

/// The address a client on this machine dials: a wildcard bind is reached
/// on loopback; a named interface on itself.
pub fn local_url(bound: SocketAddr) -> String {
    let ip = if bound.ip().is_unspecified() {
        match bound.ip() {
            IpAddr::V4(_) => IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            IpAddr::V6(_) => IpAddr::V6(std::net::Ipv6Addr::LOCALHOST),
        }
    } else {
        bound.ip()
    };
    format!("tcp://{}", SocketAddr::new(ip, bound.port()))
}

/// The token in a WebSocket upgrade: `Authorization: Bearer …` or
/// `?token=…` on the request path. Cloudflare passes both through.
pub fn token_from_request(uri: &str, authorization: Option<&str>) -> Option<String> {
    if let Some(auth) = authorization {
        let auth = auth.trim();
        if let Some(rest) = auth
            .strip_prefix("Bearer ")
            .or_else(|| auth.strip_prefix("bearer "))
        {
            return Some(rest.trim().to_string());
        }
    }
    let query = uri.split_once('?')?.1;
    query.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == "token").then(|| v.to_string())
    })
}
