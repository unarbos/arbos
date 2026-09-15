//! Auto-generated settings — written with defaults on first run, read on
//! launch. Editable, but never requires user maintenance.

use crate::{memory, model::watch};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Debug, Serialize, Deserialize)]
pub struct Settings {
    /// The ceiling decoded covers run under, in megabytes. A bare key, so it
    /// is declared above `features`: one written after that table would belong
    /// to it.
    #[serde(default = "cover_memory")]
    pub cover_memory: u64,
    /// How long a project's `.arbos/` has to go quiet before the watch
    /// re-reads it, in milliseconds — see [`crate::model::watch`]. Bare, so it
    /// belongs above `features` for the reason above.
    ///
    /// Read through [`watch::bounce`] and never used raw: this file is edited
    /// by hand, and the ends of the range are what a hand cannot reach past.
    #[serde(default = "watch_bounce")]
    pub watch_bounce: u64,
    /// What the app will show. Every bare key has to go above it.
    #[serde(default)]
    pub features: Features,
    /// Which builds this machine follows. A table, so it goes below the bare
    /// keys for the reason above.
    #[serde(default)]
    pub update: Update,
    /// Leftover from when this shell spawned ACP binaries. Ignored: one
    /// kernel, model switch via `set_model`. Kept so an old file still parses.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<Agent>,
}

/// The surfaces a project can hold, minus articles — the one thing the app is
/// for, and so not something to be able to switch off. Every one of these is
/// off until it is asked for, which makes a fresh install articles and
/// nothing else.
#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Features {
    /// Whether sessions may be opened. On by default: this shell talks to one
    /// Arbos kernel, and a session is how you talk to it.
    pub sessions: bool,
    pub boards: bool,
    pub tables: bool,
}

impl Default for Features {
    fn default() -> Self {
        Self {
            sessions: true,
            boards: false,
            tables: false,
        }
    }
}

/// Which builds this machine updates itself to.
///
/// `stable` is the tagged releases and is what anybody who has not asked
/// otherwise gets. `dev` is every green commit on `main` — the channel to be
/// on to test what just merged.
#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Update {
    /// `stable` or `dev`. A string rather than an enum so a file naming a
    /// channel this build has never heard of falls back to the default
    /// instead of refusing to parse — see [`crate::update::channel_of`].
    pub channel: String,
}

impl Default for Update {
    fn default() -> Self {
        Self {
            channel: arbos_update::Channel::default().as_str().to_owned(),
        }
    }
}

/// Put the channel in the file.
///
/// Edited with `toml_edit` for the reason [`set_feature`] is: this file is
/// meant to be opened by hand, and a round trip would drop every comment in
/// it.
pub fn set_update_channel(channel: arbos_update::Channel) -> Result<()> {
    let path = dir()?.join("settings.toml");
    let body = std::fs::read_to_string(&path).unwrap_or_default();
    let mut doc: toml_edit::DocumentMut =
        body.parse().context("settings.toml is not valid toml")?;
    let update = doc["update"].or_insert(toml_edit::table());
    let Some(update) = update.as_table_mut() else {
        anyhow::bail!("`update` in settings.toml is not a table");
    };
    update.set_implicit(false);
    update["channel"] = toml_edit::value(channel.as_str());
    std::fs::write(&path, doc.to_string())?;
    Ok(())
}

/// One switchable surface, named rather than reached as a field so the settings
/// section can list them and one writer can put any of them in the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feature {
    Sessions,
    Boards,
    Tables,
}

impl Feature {
    /// The order the Features section lists them in. Sessions first: it is the
    /// one that decides whether anything runs on this machine.
    pub const ALL: [Self; 3] = [Self::Sessions, Self::Boards, Self::Tables];

    /// The key it is written under, inside `[features]`.
    fn key(self) -> &'static str {
        match self {
            Self::Sessions => "sessions",
            Self::Boards => "boards",
            Self::Tables => "tables",
        }
    }

    pub fn on(self, features: &Features) -> bool {
        match self {
            Self::Sessions => features.sessions,
            Self::Boards => features.boards,
            Self::Tables => features.tables,
        }
    }

    pub fn set(self, features: &mut Features, on: bool) {
        match self {
            Self::Sessions => features.sessions = on,
            Self::Boards => features.boards = on,
            Self::Tables => features.tables = on,
        }
    }
}

/// A label for the one kernel this shell talks to. `command` / `args` /
/// `env` are unused leftovers from when a row named an ACP binary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

/// The only runtime this window has: the attached Arbos kernel.
pub fn kernel_agent() -> Agent {
    Agent {
        name: "Arbos".into(),
        id: Some("arbos".into()),
        command: String::new(),
        args: Vec::new(),
        env: BTreeMap::new(),
    }
}

/// What the cover ceiling is when the file does not say.
fn cover_memory() -> u64 {
    memory::DEFAULT_LIMIT / 1_000_000
}

/// And the watch's bounce, which the watch itself owns.
fn watch_bounce() -> u64 {
    watch::BOUNCE
}

/// The launchers that resolve a package name on every run. An installed
/// agent's command is a path to an unpacked executable, which resolves nothing.
const RUNNERS: [&str; 3] = ["npx", "bunx", "pnpx"];

impl Agent {
    /// Whether every npm package this entry names carries an exact version.
    /// `npx pkg@latest` resolves against the registry on every launch, which is
    /// a different program each time.
    pub fn pinned(&self) -> bool {
        if !RUNNERS.contains(&self.command.as_str()) {
            return true;
        }
        self.args
            .iter()
            .filter(|arg| !arg.starts_with('-'))
            .all(|spec| {
                let name = cacp_agents::package_name(spec);
                spec.len() > name.len()
                    && spec[name.len() + 1..].starts_with(|c: char| c.is_ascii_digit())
            })
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            cover_memory: cover_memory(),
            watch_bounce: watch_bounce(),
            features: Features::default(),
            update: Update::default(),
            agents: Vec::new(),
        }
    }
}

/// Where this shell keeps machine-local data — `$XDG_DATA_HOME/arbos-desktop`,
/// defaulting to `~/.local/share/arbos-desktop`. Separate from a Arbos install.
pub fn data_dir() -> Result<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME")
        && !xdg.is_empty()
    {
        return Ok(PathBuf::from(xdg).join("arbos-desktop"));
    }
    Ok(dirs::home_dir()
        .context("no home directory on this system")?
        .join(".local")
        .join("share")
        .join("arbos-desktop"))
}

/// Config directory: `$XDG_CONFIG_HOME/arbos-desktop`, defaulting to
/// `~/.config/arbos-desktop` — on macOS too, so a hand-edited settings.toml
/// sits where its neighbours do rather than in Application Support.
pub fn dir() -> Result<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME")
        && !xdg.is_empty()
    {
        return Ok(PathBuf::from(xdg).join("arbos-desktop"));
    }
    Ok(dirs::home_dir()
        .context("no home directory on this system")?
        .join(".config")
        .join("arbos-desktop"))
}

pub fn load() -> Result<Settings> {
    let dir = dir()?;
    let path = dir.join("settings.toml");
    if !path.exists() {
        let settings = Settings::default();
        std::fs::create_dir_all(&dir)?;
        let body = format!(
            "# generated by arbos-desktop — edits are kept, deleting regenerates defaults\n\n{}",
            toml::to_string_pretty(&settings)?
        );
        std::fs::write(&path, body)?;
        return Ok(settings);
    }
    let content = std::fs::read_to_string(&path)?;
    let mut settings: Settings = toml::from_str(&content)
        .with_context(|| format!("invalid settings: {}", path.display()))?;
    // Leftover `[[agents]]` rows are not a runtime. Drop floating npm
    // tags so an old file cannot look like it launches a catalog binary.
    settings.agents.retain(Agent::pinned);
    Ok(settings)
}

/// Switch a feature on or off in the file.
///
/// Edited with `toml_edit` for the reason [`put_agent`] is: the file is meant
/// to be opened by hand, and a round trip would drop every comment in it. The
/// table is put in explicitly rather than sprung from the index, because a
/// table that arrives that way is implicit and prints no header of its own.
pub fn set_feature(feature: Feature, on: bool) -> Result<()> {
    let path = dir()?.join("settings.toml");
    let body = std::fs::read_to_string(&path).unwrap_or_default();
    let mut doc: toml_edit::DocumentMut =
        body.parse().context("settings.toml is not valid toml")?;
    let features = doc["features"].or_insert(toml_edit::table());
    let Some(features) = features.as_table_mut() else {
        anyhow::bail!("`features` in settings.toml is not a table");
    };
    features.set_implicit(false);
    features[feature.key()] = toml_edit::value(on);
    std::fs::write(&path, doc.to_string())?;
    Ok(())
}

/// Move the cover ceiling in the file, in megabytes.
pub fn set_cover_memory(mb: u64) -> Result<()> {
    let path = dir()?.join("settings.toml");
    let body = std::fs::read_to_string(&path).unwrap_or_default();
    let mut doc: toml_edit::DocumentMut =
        body.parse().context("settings.toml is not valid toml")?;
    doc["cover_memory"] = toml_edit::value(mb as i64);
    std::fs::write(&path, doc.to_string())?;
    Ok(())
}

/// Move the watch's bounce in the file, in milliseconds.
pub fn set_watch_bounce(ms: u64) -> Result<()> {
    let path = dir()?.join("settings.toml");
    let body = std::fs::read_to_string(&path).unwrap_or_default();
    let mut doc: toml_edit::DocumentMut =
        body.parse().context("settings.toml is not valid toml")?;
    doc["watch_bounce"] = toml_edit::value(ms as i64);
    std::fs::write(&path, doc.to_string())?;
    Ok(())
}

/// Put `agent` in the file, replacing whichever entry already launches it.
///
/// `supersedes` is the npm package the agent is published as, which is how an
/// install claims the hand-written `@latest` entry that shipped as a default
/// instead of sitting next to it. A replaced entry keeps its own `name`: the
/// person who wrote it chose that, and only the command underneath has moved.
///
/// Edited in place with `toml_edit` rather than re-serialised: this file is
/// meant to be opened and changed by hand, and a round trip through a value
/// tree would silently delete every comment in it.
pub fn put_agent(agent: &Agent, supersedes: Option<&str>) -> Result<()> {
    let path = dir()?.join("settings.toml");
    let body = std::fs::read_to_string(&path).unwrap_or_default();
    let mut doc: toml_edit::DocumentMut =
        body.parse().context("settings.toml is not valid toml")?;

    let agents = doc["agents"].or_insert(toml_edit::Item::ArrayOfTables(
        toml_edit::ArrayOfTables::new(),
    ));
    let Some(agents) = agents.as_array_of_tables_mut() else {
        anyhow::bail!("`agents` in settings.toml is not a list of tables");
    };
    let existing = agents
        .iter()
        .position(|table| claims(table, agent, supersedes));
    let name = existing
        .and_then(|ix| agents.get(ix))
        .and_then(|table| table.get("name"))
        .and_then(|n| n.as_str())
        .unwrap_or(&agent.name)
        .to_owned();
    // Whatever preceded the entry — the file's header, a note the user left
    // above it — is trivia hanging off the table, and replacing the table
    // throws it away unless it is carried across by hand.
    let decor = existing
        .and_then(|ix| agents.get(ix))
        .map(|table| table.decor().clone());

    let mut entry = toml_edit::Table::new();
    entry["name"] = toml_edit::value(name);
    if let Some(id) = &agent.id {
        entry["id"] = toml_edit::value(id.clone());
    }
    entry["command"] = toml_edit::value(agent.command.clone());
    let mut args = toml_edit::Array::new();
    for arg in &agent.args {
        args.push(arg.as_str());
    }
    entry["args"] = toml_edit::value(args);
    if !agent.env.is_empty() {
        let mut env = toml_edit::InlineTable::new();
        for (key, value) in &agent.env {
            env.insert(key, value.as_str().into());
        }
        entry["env"] = toml_edit::value(env);
    }

    match existing {
        Some(ix) => {
            if let Some(decor) = decor {
                *entry.decor_mut() = decor;
            }
            *agents.get_mut(ix).expect("position is in range") = entry;
        }
        None => agents.push(entry),
    }
    std::fs::write(&path, doc.to_string())?;
    Ok(())
}

/// Drop the entry installed from registry agent `id`.
pub fn remove_agent(id: &str) -> Result<()> {
    let path = dir()?.join("settings.toml");
    let body = std::fs::read_to_string(&path).unwrap_or_default();
    let mut doc: toml_edit::DocumentMut =
        body.parse().context("settings.toml is not valid toml")?;
    let Some(agents) = doc
        .get_mut("agents")
        .and_then(|a| a.as_array_of_tables_mut())
    else {
        return Ok(());
    };
    let Some(ix) = agents
        .iter()
        .position(|table| table.get("id").and_then(|i| i.as_str()) == Some(id))
    else {
        return Ok(());
    };
    // The file's header hangs off whichever entry comes first. If that is the
    // one being dropped, the header has to move down onto its successor or it
    // leaves with it.
    let prefix = agents
        .get(ix)
        .and_then(|table| table.decor().prefix().cloned());
    agents.remove(ix);
    if ix == 0
        && let Some(prefix) = prefix
    {
        match agents.get_mut(0) {
            Some(first) => first.decor_mut().set_prefix(prefix),
            None => doc.as_table_mut().decor_mut().set_prefix(prefix),
        }
    }
    std::fs::write(&path, doc.to_string())?;
    Ok(())
}

/// Whether an existing entry is the one this install replaces: the same
/// registry agent, or a launcher for the same npm package.
fn claims(table: &toml_edit::Table, agent: &Agent, supersedes: Option<&str>) -> bool {
    let field = |key| table.get(key).and_then(|v| v.as_str());
    if agent.id.is_some() && field("id") == agent.id.as_deref() {
        return true;
    }
    let Some(package) = supersedes else {
        return false;
    };
    table
        .get("args")
        .and_then(|args| args.as_array())
        .is_some_and(|args| {
            args.iter()
                .filter_map(|v| v.as_str())
                .any(|arg| cacp_agents::package_name(arg) == package)
        })
}
