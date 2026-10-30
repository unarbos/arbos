//! Avatar colours for Swift: the D79 index rule and the sender accent bar
//! (`arbos_core::avatar`), so the iPhone draws the same hex as the Mac.

use arbos_core::avatar::{self, identity, leaf};

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

// ---- D92 leaf avatars --------------------------------------------------------------------------

/// The leaf avatar in effect for an agent: shape 0–8 and disc colour 0–9.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct LeafPick {
    pub leaf: u8,
    pub color: u8,
}

impl From<leaf::LeafPick> for LeafPick {
    fn from(p: leaf::LeafPick) -> Self {
        Self {
            leaf: p.leaf,
            color: p.color,
        }
    }
}

/// One absolute outline command in the 100 × 100 leaf box. Fill even-odd.
#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum LeafSegment {
    MoveTo {
        x: f32,
        y: f32,
    },
    LineTo {
        x: f32,
        y: f32,
    },
    CubicTo {
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        x: f32,
        y: f32,
    },
    Close,
}

impl From<&leaf::Segment> for LeafSegment {
    fn from(s: &leaf::Segment) -> Self {
        match *s {
            leaf::Segment::MoveTo { x, y } => LeafSegment::MoveTo { x, y },
            leaf::Segment::LineTo { x, y } => LeafSegment::LineTo { x, y },
            leaf::Segment::CubicTo {
                x1,
                y1,
                x2,
                y2,
                x,
                y,
            } => LeafSegment::CubicTo {
                x1,
                y1,
                x2,
                y2,
                x,
                y,
            },
            leaf::Segment::Close => LeafSegment::Close,
        }
    }
}

/// D92: the leaf and colour for an agent id, honouring the owner's picks.
#[uniffi::export]
pub fn leaf_pick(seed: String, leaf: Option<u8>, color: Option<u8>) -> LeafPick {
    leaf::pick_for(&seed, leaf, color).into()
}

/// Number of leaf shapes.
#[uniffi::export]
pub fn leaf_count() -> u8 {
    leaf::LEAF_COUNT
}

/// Number of disc colours.
#[uniffi::export]
pub fn leaf_color_count() -> u8 {
    leaf::COLOR_COUNT
}

/// Picker labels of the shapes, index order.
#[uniffi::export]
pub fn leaf_names() -> Vec<String> {
    leaf::LEAVES.iter().map(|l| l.name.to_owned()).collect()
}

/// Picker labels of the colours, index order.
#[uniffi::export]
pub fn leaf_color_names() -> Vec<String> {
    leaf::COLORS.iter().map(|c| c.name.to_owned()).collect()
}

/// Disc colour `index` (the pastel) as `#RRGGBB`.
#[uniffi::export]
pub fn leaf_color_hex(index: u8) -> String {
    leaf::COLORS[(index % leaf::COLOR_COUNT) as usize].rgb.hex()
}

/// The leaf ink on disc colour `index` (the darker same hue) as `#RRGGBB`.
#[uniffi::export]
pub fn leaf_ink_hex(index: u8) -> String {
    leaf::COLORS[(index % leaf::COLOR_COUNT) as usize].ink.hex()
}

/// The leaf's span as a share of the disc diameter.
#[uniffi::export]
pub fn leaf_scale() -> f32 {
    leaf::LEAF_SCALE
}

/// The outline of shape `index` in the 100 × 100 box, absolute commands.
#[uniffi::export]
pub fn leaf_segments(index: u8) -> Vec<LeafSegment> {
    leaf::segments(index).iter().map(Into::into).collect()
}

/// The stem pivot of shape `index` in the 100 × 100 box: `[x, y]`.
#[uniffi::export]
pub fn leaf_pivot(index: u8) -> Vec<f32> {
    let p = leaf::LEAVES[(index % leaf::LEAF_COUNT) as usize].pivot;
    vec![p.0, p.1]
}

/// Length of shape `index`'s idle sway loop, seconds (staggered per leaf).
#[uniffi::export]
pub fn leaf_idle_secs(index: u8) -> f32 {
    leaf::LEAVES[(index % leaf::LEAF_COUNT) as usize].idle_secs
}

/// Idle sway angle at `t` seconds, radians.
#[uniffi::export]
pub fn leaf_idle_angle(index: u8, t: f32) -> f32 {
    leaf::idle_angle(index, t)
}

/// Gust angle `t` seconds after it starts, radians; 0 outside `0..leaf_gust_secs()`.
#[uniffi::export]
pub fn leaf_gust_angle(t: f32) -> f32 {
    leaf::gust_angle(t)
}

/// Length of the hover / activity gust, seconds.
#[uniffi::export]
pub fn leaf_gust_secs() -> f32 {
    leaf::GUST_SECS
}

/// Shape `index`'s Lottie file (`gust` = the 1 s hover/activity gust, else the idle loop) as
/// JSON text, for lottie-ios; transparent background, recolour keypath `**.leaf.Color`.
#[uniffi::export]
pub fn leaf_lottie_json(index: u8, gust: bool) -> String {
    let l = &leaf::LEAVES[(index % leaf::LEAF_COUNT) as usize];
    if gust {
        l.gust_lottie.to_owned()
    } else {
        l.idle_lottie.to_owned()
    }
}

/// The bubble-bar accent of a leaf agent: its disc colour.
#[uniffi::export]
pub fn leaf_accent(seed: String, leaf: Option<u8>, color: Option<u8>) -> AccentHex {
    leaf::accent(leaf::pick_for(&seed, leaf, color)).into()
}

// ---- One agent identity for every surface (#1592) ---------------------------------------------

/// What a roster row or a project record knows about an agent (`identity::AgentCard`).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentCard {
    pub id: String,
    pub name: String,
    pub handle: String,
    pub leaf: Option<u8>,
    pub leaf_color: Option<u8>,
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
            leaf: c.leaf,
            leaf_color: c.leaf_color,
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
    pub leaf: LeafPick,
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
            leaf: a.leaf.into(),
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
/// the id-seeded leaf and the row's own name.
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
