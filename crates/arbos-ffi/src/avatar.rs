//! Avatar colours for Swift: the D79 index rule and the sender accent bar
//! (`arbos_core::avatar`), so the iPhone draws the same hex as the Mac.

use arbos_core::avatar::{self, disc, identity};

/// A sender accent in both appearances, as `#RRGGBB`.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AccentHex {
    pub dark: String,
    pub light: String,
}

impl From<avatar::Accent> for AccentHex {
    fn from(a: avatar::Accent) -> Self {
        Self {
            dark: a.dark.hex(),
            light: a.light.hex(),
        }
    }
}

/// D79: the gradient index for a stable id, or the owner's override (0–6).
#[uniffi::export]
pub fn avatar_color_index(seed: String, override_index: Option<u8>) -> u8 {
    avatar::index_for(&seed, override_index)
}

/// The dominant vibrant colour of a decoded picture (`width × height` RGBA,
/// any size), or `None` when it is colourless and the D79 fallback applies.
#[uniffi::export]
pub fn avatar_dominant_accent(rgba: Vec<u8>, width: u32, height: u32) -> Option<AccentHex> {
    avatar::dominant_accent(&rgba, width, height).map(Into::into)
}

/// The D79 fallback accent: the midpoint of the sender's gradient.
#[uniffi::export]
pub fn avatar_fallback_accent(seed: String, override_index: Option<u8>) -> AccentHex {
    avatar::fallback_accent(&seed, override_index).into()
}

/// Bump of the extraction rules; part of any persisted cache key.
#[uniffi::export]
pub fn avatar_accent_version() -> u32 {
    avatar::ACCENT_VERSION
}

// ---- D92 solid-colour discs --------------------------------------------------------------------

/// D92: the disc colour for an agent id, honouring the owner's pick.
#[uniffi::export]
pub fn disc_color(seed: String, color: Option<u8>) -> u8 {
    disc::color_for(&seed, color)
}

/// Number of disc colours.
#[uniffi::export]
pub fn disc_color_count() -> u8 {
    disc::COLOR_COUNT
}

/// Picker labels of the colours, index order.
#[uniffi::export]
pub fn disc_color_names() -> Vec<String> {
    disc::COLORS.iter().map(|c| c.name.to_owned()).collect()
}

/// Disc colour `index` as `#RRGGBB`.
#[uniffi::export]
pub fn disc_color_hex(index: u8) -> String {
    disc::fill(index).hex()
}

/// The sentinel stored for "no picture": the name's initial on a neutral grey disc.
#[uniffi::export]
pub fn disc_neutral_index() -> u8 {
    disc::NEUTRAL
}

/// Bump of the index rule; clients refresh cached defaults when it moves.
#[uniffi::export]
pub fn disc_rule_version() -> u32 {
    disc::DISC_RULE_VERSION
}

/// The bubble-bar accent of an agent drawing its disc: the disc colour itself.
#[uniffi::export]
pub fn disc_accent(seed: String, color: Option<u8>) -> AccentHex {
    disc::accent(disc::color_for(&seed, color)).into()
}

// ---- One agent identity for every surface (#1592) ---------------------------------------------

/// What a roster row or a project record knows about an agent (`identity::AgentCard`).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentCard {
    pub id: String,
    pub name: String,
    pub handle: String,
    pub disc_color: Option<u8>,
    pub color_index: Option<u8>,
    pub image_key: Option<String>,
    /// The relay's stamp of the last avatar change (`0` when the source has none); among
    /// copies of one agent the highest stamp is drawn.
    pub avatar_version: i64,
}

impl From<AgentCard> for identity::AgentCard {
    fn from(c: AgentCard) -> Self {
        Self {
            id: c.id,
            name: c.name,
            handle: c.handle,
            disc_color: c.disc_color,
            color_index: c.color_index,
            image_key: c.image_key,
            avatar_version: c.avatar_version,
        }
    }
}

/// The one answer every surface draws from (`identity::AgentIdentity`).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentIdentity {
    pub id: String,
    pub name: String,
    pub handle: String,
    pub disc_color: u8,
    pub color_index: u8,
    pub image_key: Option<String>,
    /// Stamp of the copy drawn; part of any picture cache key.
    pub avatar_version: i64,
    pub known: bool,
}

impl From<identity::AgentIdentity> for AgentIdentity {
    fn from(a: identity::AgentIdentity) -> Self {
        Self {
            id: a.id,
            name: a.name,
            handle: a.handle,
            disc_color: a.disc_color,
            color_index: a.color_index,
            image_key: a.image_key,
            avatar_version: a.avatar_version,
            known: a.known,
        }
    }
}

/// Which card a row is drawn from: `sender` (`agent:<id>` / `project:<id>` / bare id), the
/// row's `senderHandle` and `senderName`, against `cards` in the caller's order (the open
/// chat's roster, then my projects, then other rosters). Never fails: an unknown agent gets
/// the id-seeded disc and the row's own name.
#[uniffi::export]
pub fn resolve_agent(
    sender: String,
    handle: Option<String>,
    name: Option<String>,
    cards: Vec<AgentCard>,
) -> AgentIdentity {
    let cards: Vec<identity::AgentCard> = cards.into_iter().map(Into::into).collect();
    identity::resolve(
        &identity::AgentQuery {
            sender: &sender,
            handle: handle.as_deref(),
            name: name.as_deref(),
        },
        &cards,
    )
    .into()
}

/// `agent:<id>` / `project:<id>` → `<id>`.
#[uniffi::export]
pub fn agent_sender_id(sender: String) -> String {
    identity::sender_id(&sender).to_owned()
}

/// The `@slug` an agent answers to (the relay roster's `handle` rule).
#[uniffi::export]
pub fn agent_slug(name: String) -> String {
    identity::agent_slug(&name)
}

/// The words a surface shows: the name, else `@handle`, else "Agent".
#[uniffi::export]
pub fn agent_display_name(name: Option<String>, handle: Option<String>) -> String {
    identity::display_name(name.as_deref(), handle.as_deref())
}
