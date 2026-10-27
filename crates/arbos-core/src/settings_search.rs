//! Settings as search results ([feedback search-settings-mac-2026-10-26] /
//! [feedback search-settings-ios-2026-10-26]).
//!
//! Jacob, 2026-10-26: the Mac palette and the iPhone bottom search should also
//! find Settings — every page of the Settings list, its sub-pages, and the
//! words people type for them ("theme", "opacity" → Appearance; "voice",
//! "model" → an agent's setting; "token", "1Password" → Secrets). Picking a
//! row opens the settings pane straight on that page.
//!
//! This module is the one index both apps read (the Mac directly, the iPhone
//! through `arbos-ffi`): what is indexed, under which words, and on which
//! platform. Matching goes through the shared matcher ([`crate::search`]):
//! a page's title and each of its keywords are "names", so "dev" finds
//! Devices the way it finds Devin, and "opacity" is an exact hit on
//! Appearance's keyword. What a hit opens is each app's business — they map
//! [`SettingsItem`] to their own page enum — and each app's structure test
//! checks that every page it lists is here.
//!
//! Not indexed on purpose (Jacob, 14:18 CR): Sign out and Pause all secrets
//! (a search result must never act), the pages a platform hides (the Mac's
//! Credits / Connected apps until they return), and a server's or group's own
//! Settings tab (its chat row already finds it).

use crate::search::{match_text, MatchKind};

/// Which app is asking: the index differs a little per platform (the Mac has
/// Updates and On this Mac, the iPhone has Privacy, Text Size, Credits and
/// Connected apps).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Platform {
    Mac,
    Iphone,
}

/// One thing search can land on. Each app maps a variant to its own page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SettingsItem {
    Appearance,
    /// The Glass opacity slider on the Mac's Appearance page; the iPhone has
    /// no slider, Surface = Glass is its translucency control, so the same
    /// words land on Appearance there.
    AppearanceOpacity,
    Notifications,
    Secrets,
    SecretsNew,
    SecretsConnectOnePassword,
    Devices,
    /// "On this Mac" (the paired `arbos-local` helpers) / "On this iPhone"
    /// (the storage line in the Settings list; no page of its own).
    OnThisDevice,
    Machine,
    /// The Mac's Updates page; on the iPhone the About page carries updates.
    Updates,
    SendFeedback,
    About,
    Privacy,
    TextSize,
    Credits,
    ConnectedApps,
    /// My Profile tab (name, handle, picture).
    Profile,
    /// An agent's Model row — one result per owned agent.
    AgentModel,
    /// An agent's Reasoning row — one result per owned agent.
    AgentReasoning,
    /// An agent's Voice row — one result per owned agent.
    AgentVoice,
}

/// One row of the index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SettingsEntry {
    pub item: SettingsItem,
    /// The row title ("Devices"); the Mac wording where the platforms differ.
    pub title: &'static str,
    /// The iPhone's wording when it differs from `title` ("On this iPhone").
    pub title_iphone: Option<&'static str>,
    /// Where it lives, as the subtitle of a title hit ("Settings", "Settings › Secrets").
    pub path: &'static str,
    /// What people type for it; each is matched like a name (folded, prefix, word-start).
    pub keywords: &'static [&'static str],
    pub platforms: &'static [Platform],
    /// Expanded to one row per owned agent by the app (the title is the row's
    /// name, the agent's name goes in the subtitle).
    pub per_agent: bool,
}

impl SettingsEntry {
    /// The title as `platform` words it.
    pub fn title_for(&self, platform: Platform) -> &'static str {
        match platform {
            Platform::Mac => self.title,
            Platform::Iphone => self.title_iphone.unwrap_or(self.title),
        }
    }

    pub fn on(&self, platform: Platform) -> bool {
        self.platforms.contains(&platform)
    }
}

const BOTH: &[Platform] = &[Platform::Mac, Platform::Iphone];
const MAC: &[Platform] = &[Platform::Mac];
const IPHONE: &[Platform] = &[Platform::Iphone];

macro_rules! entry {
    ($item:ident, $title:literal, $path:literal, [$($kw:literal),* $(,)?], $platforms:expr) => {
        SettingsEntry {
            item: SettingsItem::$item,
            title: $title,
            title_iphone: None,
            path: $path,
            keywords: &[$($kw),*],
            platforms: $platforms,
            per_agent: false,
        }
    };
}

/// The index, in the Settings list's order (top-level page, then its sub-pages).
pub const INDEX: &[SettingsEntry] = &[
    entry!(
        Appearance,
        "Appearance",
        "Settings",
        ["theme", "dark", "light", "system", "matte", "glass", "surface", "look"],
        BOTH
    ),
    entry!(
        AppearanceOpacity,
        "Appearance",
        "Settings",
        ["opacity", "transparency", "blur", "frost", "translucent"],
        BOTH
    ),
    entry!(
        Notifications,
        "Notifications",
        "Settings",
        [
            "alerts",
            "banner",
            "sound",
            "read receipts",
            "push",
            "permission",
            "badge"
        ],
        BOTH
    ),
    entry!(
        Secrets,
        "Secrets",
        "Settings",
        [
            "token",
            "api key",
            "password",
            "env",
            "1password",
            "vault",
            "credentials"
        ],
        BOTH
    ),
    entry!(
        SecretsNew,
        "New secret",
        "Settings › Secrets",
        ["add secret", "new key", "login", "env name", "add token"],
        BOTH
    ),
    entry!(
        SecretsConnectOnePassword,
        "Connect 1Password",
        "Settings › Secrets",
        ["1password", "service account", "vault", "op"],
        BOTH
    ),
    entry!(
        Devices,
        "Devices",
        "Settings",
        [
            "signed in",
            "other devices",
            "sessions",
            "revoke",
            "this mac",
            "this iphone"
        ],
        BOTH
    ),
    SettingsEntry {
        item: SettingsItem::OnThisDevice,
        title: "On this Mac",
        title_iphone: Some("On this iPhone"),
        path: "Settings",
        keywords: &[
            "local",
            "helper",
            "arbos-local",
            "install",
            "read files",
            "storage",
            "this mac",
            "this iphone",
        ],
        platforms: BOTH,
        per_agent: false,
    },
    entry!(
        Machine,
        "Machine",
        "Settings",
        [
            "computer",
            "cloud",
            "daytona",
            "restart",
            "spec",
            "awake",
            "stop machine",
            "start machine"
        ],
        BOTH
    ),
    entry!(
        Updates,
        "Updates",
        "Settings",
        [
            "update",
            "version",
            "automatic",
            "install",
            "check now",
            "new version"
        ],
        MAC
    ),
    entry!(
        SendFeedback,
        "Send feedback",
        "Settings",
        [
            "feedback",
            "bug",
            "report",
            "screenshot",
            "request",
            "issue"
        ],
        BOTH
    ),
    entry!(
        About,
        "About",
        "Settings",
        [
            "version",
            "build",
            "relay",
            "protocol",
            "turn details",
            "show details",
            "updates",
            "testflight"
        ],
        BOTH
    ),
    entry!(
        Privacy,
        "Privacy",
        "Settings",
        ["privacy", "groups", "who can add me", "spam", "invites"],
        IPHONE
    ),
    entry!(
        TextSize,
        "Text Size",
        "Settings",
        [
            "text size",
            "font",
            "bigger",
            "smaller",
            "zoom",
            "larger text"
        ],
        IPHONE
    ),
    entry!(
        Credits,
        "Credits",
        "Settings",
        ["credits", "balance", "spend", "top-up", "cost", "billing"],
        IPHONE
    ),
    entry!(
        ConnectedApps,
        "Connected apps",
        "Settings",
        [
            "connected apps",
            "google",
            "slack",
            "github",
            "oauth",
            "connectors"
        ],
        IPHONE
    ),
    entry!(
        Profile,
        "Profile",
        "Settings",
        [
            "profile",
            "name",
            "handle",
            "picture",
            "avatar",
            "display name"
        ],
        BOTH
    ),
    SettingsEntry {
        item: SettingsItem::AgentModel,
        title: "Model",
        title_iphone: None,
        path: "Settings",
        keywords: &["model", "gpt", "claude", "grok", "llm", "agent model"],
        platforms: BOTH,
        per_agent: true,
    },
    SettingsEntry {
        item: SettingsItem::AgentReasoning,
        title: "Reasoning",
        title_iphone: None,
        path: "Settings",
        keywords: &["reasoning", "effort", "thinking", "low", "medium", "high"],
        platforms: BOTH,
        per_agent: true,
    },
    SettingsEntry {
        item: SettingsItem::AgentVoice,
        title: "Voice",
        title_iphone: None,
        path: "Settings",
        keywords: &["voice", "speak", "tts", "speech", "call voice"],
        platforms: BOTH,
        per_agent: true,
    },
];

/// The entries `platform` indexes, in list order.
pub fn index(platform: Platform) -> impl Iterator<Item = &'static SettingsEntry> {
    INDEX.iter().filter(move |e| e.on(platform))
}

/// What a hit matched on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsMatched {
    /// The row's own title.
    Title,
    /// One of its keywords (shown as the subtitle: "**Opacity** · Settings").
    Keyword(&'static str),
}

/// One matching entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SettingsHit {
    pub entry: &'static SettingsEntry,
    pub kind: MatchKind,
    pub matched: SettingsMatched,
}

impl SettingsHit {
    /// The row order inside the Settings section: the title hits (exact ›
    /// prefix › word-start), then the keyword hits in the same order — the
    /// shared matcher's own name-before-title rule.
    pub fn order(&self) -> u8 {
        let kind = self.kind as u8;
        match self.matched {
            SettingsMatched::Title => kind,
            SettingsMatched::Keyword(_) => 3 + kind,
        }
    }

    /// True when the query *is* the title or a keyword: the Settings section
    /// then comes before every other section (Jacob's rule).
    pub fn is_exact(&self) -> bool {
        self.kind == MatchKind::Exact
    }
}

/// The best way one entry matches the normalized query, if it does.
pub fn match_entry(
    entry: &'static SettingsEntry,
    platform: Platform,
    q: &str,
) -> Option<SettingsHit> {
    if q.is_empty() {
        return None;
    }
    let title = match_text(entry.title_for(platform), q).map(|kind| SettingsHit {
        entry,
        kind,
        matched: SettingsMatched::Title,
    });
    let keyword = entry
        .keywords
        .iter()
        .filter_map(|kw| {
            match_text(kw, q).map(|kind| SettingsHit {
                entry,
                kind,
                matched: SettingsMatched::Keyword(kw),
            })
        })
        .min_by_key(|h| h.order());
    [title, keyword]
        .into_iter()
        .flatten()
        .min_by_key(|h| h.order())
}

/// Every entry of `platform` that matches `q` (already
/// [`crate::search::normalize_query`]'d), best first, stable inside a tie so
/// the index order (top-level page before its sub-pages) holds. An empty
/// query finds nothing: Settings never show in Recent.
pub fn search(q: &str, platform: Platform) -> Vec<SettingsHit> {
    let mut out: Vec<SettingsHit> = index(platform)
        .filter_map(|e| match_entry(e, platform, q))
        .collect();
    out.sort_by_key(|h| h.order());
    out
}

/// True when any hit is exact: the Settings section jumps to the top.
pub fn jumps_first(hits: &[SettingsHit]) -> bool {
    hits.iter().any(SettingsHit::is_exact)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::normalize_query;

    fn titles(q: &str, platform: Platform) -> Vec<(&'static str, SettingsItem)> {
        search(&normalize_query(q), platform)
            .into_iter()
            .map(|h| (h.entry.title_for(platform), h.entry.item))
            .collect()
    }

    #[test]
    fn a_prefix_of_a_title_finds_the_page() {
        let hits = search("dev", Platform::Mac);
        assert_eq!(hits[0].entry.item, SettingsItem::Devices);
        assert_eq!(hits[0].kind, MatchKind::Prefix);
        assert_eq!(hits[0].matched, SettingsMatched::Title);
        assert!(
            !jumps_first(&hits),
            "a prefix hit keeps the section in its place"
        );
    }

    #[test]
    fn a_keyword_finds_the_page_and_an_exact_keyword_jumps_first() {
        let hits = search("opacity", Platform::Mac);
        assert_eq!(hits[0].entry.item, SettingsItem::AppearanceOpacity);
        assert_eq!(hits[0].matched, SettingsMatched::Keyword("opacity"));
        assert!(jumps_first(&hits));
        // The iPhone has no slider; the words still land on Appearance.
        assert_eq!(
            search("opacity", Platform::Iphone)[0].entry.item,
            SettingsItem::AppearanceOpacity
        );
        assert_eq!(
            titles("theme", Platform::Mac)[0].1,
            SettingsItem::Appearance
        );
        assert_eq!(
            titles("1password", Platform::Mac)[0].1,
            SettingsItem::Secrets
        );
        assert_eq!(
            titles("token", Platform::Iphone)[0].1,
            SettingsItem::Secrets
        );
    }

    #[test]
    fn agent_settings_are_per_agent_rows() {
        let hits = search("voice", Platform::Mac);
        assert_eq!(hits[0].entry.item, SettingsItem::AgentVoice);
        assert!(hits[0].entry.per_agent);
        assert!(jumps_first(&hits));
        assert_eq!(
            search("model", Platform::Iphone)[0].entry.item,
            SettingsItem::AgentModel
        );
    }

    #[test]
    fn title_hits_come_before_keyword_hits_and_exact_before_prefix() {
        // "about" is About's title and a keyword nowhere else; "version" is a
        // keyword of both Updates (Mac) and About — stable index order, Updates first.
        assert_eq!(titles("about", Platform::Mac)[0].1, SettingsItem::About);
        let version: Vec<SettingsItem> = titles("version", Platform::Mac)
            .into_iter()
            .map(|t| t.1)
            .collect();
        assert_eq!(version, [SettingsItem::Updates, SettingsItem::About]);
        // "secrets" exact title beats "New secret" (word-start on "secret").
        let secrets = titles("secret", Platform::Mac);
        assert_eq!(secrets[0].1, SettingsItem::Secrets);
        assert!(secrets.iter().any(|t| t.1 == SettingsItem::SecretsNew));
        // A prefix on a title outranks an exact keyword: "up" → Updates (title)
        // before About (keyword "updates" is only a prefix hit too) — and
        // "updates" exact title beats About's exact keyword.
        assert_eq!(
            titles("updates", Platform::Mac)
                .iter()
                .map(|t| t.1)
                .collect::<Vec<_>>(),
            [SettingsItem::Updates, SettingsItem::About]
        );
    }

    #[test]
    fn folding_and_word_starts_apply_to_settings_too() {
        assert_eq!(
            titles("Thème", Platform::Mac)[0].1,
            SettingsItem::Appearance
        );
        assert_eq!(
            titles("send feed", Platform::Mac)[0].1,
            SettingsItem::SendFeedback
        );
        assert_eq!(
            titles("read rec", Platform::Mac)[0].1,
            SettingsItem::Notifications
        );
    }

    #[test]
    fn platforms_see_their_own_pages_only() {
        assert!(index(Platform::Mac).any(|e| e.item == SettingsItem::Updates));
        assert!(index(Platform::Iphone).all(|e| e.item != SettingsItem::Updates));
        assert!(index(Platform::Iphone).any(|e| e.item == SettingsItem::TextSize));
        assert!(index(Platform::Mac).all(|e| e.item != SettingsItem::TextSize));
        assert_eq!(
            INDEX
                .iter()
                .find(|e| e.item == SettingsItem::OnThisDevice)
                .unwrap()
                .title_for(Platform::Iphone),
            "On this iPhone"
        );
        assert!(search("updates", Platform::Iphone)
            .iter()
            .any(|h| h.entry.item == SettingsItem::About));
    }

    #[test]
    fn the_empty_query_finds_nothing_and_each_entry_has_enough_keywords() {
        assert!(search("", Platform::Mac).is_empty());
        for e in INDEX {
            assert!(
                e.keywords.len() >= 3,
                "{:?} has {} keywords",
                e.item,
                e.keywords.len()
            );
            assert!(!e.platforms.is_empty(), "{:?} is on no platform", e.item);
        }
    }
}
