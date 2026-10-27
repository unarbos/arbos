//! Settings as search results for Swift (`arbos_core::settings_search`): the
//! iPhone's bottom search lists the Settings pages the same index the Mac
//! palette uses ([feedback search-settings-ios-2026-10-26]). The app maps
//! [`SettingsItem`] to its own pages; `settings_index` lets its structure
//! test check that every page it lists is indexed.

use arbos_core::settings_search as core;

use crate::search::SearchMatchKind;

/// One thing search can land on (`arbos_core::settings_search::SettingsItem`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SettingsItem {
    Appearance,
    AppearanceOpacity,
    Notifications,
    Secrets,
    SecretsNew,
    SecretsConnectOnePassword,
    Devices,
    OnThisDevice,
    Machine,
    Updates,
    SendFeedback,
    About,
    Privacy,
    TextSize,
    Credits,
    ConnectedApps,
    Profile,
    AgentModel,
    AgentReasoning,
    AgentVoice,
}

impl From<core::SettingsItem> for SettingsItem {
    fn from(item: core::SettingsItem) -> Self {
        match item {
            core::SettingsItem::Appearance => SettingsItem::Appearance,
            core::SettingsItem::AppearanceOpacity => SettingsItem::AppearanceOpacity,
            core::SettingsItem::Notifications => SettingsItem::Notifications,
            core::SettingsItem::Secrets => SettingsItem::Secrets,
            core::SettingsItem::SecretsNew => SettingsItem::SecretsNew,
            core::SettingsItem::SecretsConnectOnePassword => {
                SettingsItem::SecretsConnectOnePassword
            }
            core::SettingsItem::Devices => SettingsItem::Devices,
            core::SettingsItem::OnThisDevice => SettingsItem::OnThisDevice,
            core::SettingsItem::Machine => SettingsItem::Machine,
            core::SettingsItem::Updates => SettingsItem::Updates,
            core::SettingsItem::SendFeedback => SettingsItem::SendFeedback,
            core::SettingsItem::About => SettingsItem::About,
            core::SettingsItem::Privacy => SettingsItem::Privacy,
            core::SettingsItem::TextSize => SettingsItem::TextSize,
            core::SettingsItem::Credits => SettingsItem::Credits,
            core::SettingsItem::ConnectedApps => SettingsItem::ConnectedApps,
            core::SettingsItem::Profile => SettingsItem::Profile,
            core::SettingsItem::AgentModel => SettingsItem::AgentModel,
            core::SettingsItem::AgentReasoning => SettingsItem::AgentReasoning,
            core::SettingsItem::AgentVoice => SettingsItem::AgentVoice,
        }
    }
}

/// One row of the iPhone's index.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SettingsIndexEntry {
    pub item: SettingsItem,
    pub title: String,
    /// Where it lives ("Settings", "Settings › Secrets"): the subtitle of a title hit.
    pub path: String,
    pub keywords: Vec<String>,
    /// One row per owned agent (Model · Reasoning · Voice).
    pub per_agent: bool,
}

fn entry(e: &core::SettingsEntry) -> SettingsIndexEntry {
    SettingsIndexEntry {
        item: e.item.into(),
        title: e.title_for(core::Platform::Iphone).to_string(),
        path: e.path.to_string(),
        keywords: e.keywords.iter().map(|k| k.to_string()).collect(),
        per_agent: e.per_agent,
    }
}

/// One matching entry, best first in `settings_search`'s answer.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SettingsSearchHit {
    pub entry: SettingsIndexEntry,
    pub kind: SearchMatchKind,
    /// The keyword that matched; `None` when the title did.
    pub keyword: Option<String>,
    /// The order inside the Settings section (0 best).
    pub order: u8,
    /// The query is the title or a keyword: the section jumps first.
    pub exact: bool,
}

/// Every iPhone Settings entry, in the Settings list's order.
#[uniffi::export]
pub fn settings_index() -> Vec<SettingsIndexEntry> {
    core::index(core::Platform::Iphone).map(entry).collect()
}

/// The iPhone Settings entries the typed query finds, best first.
#[uniffi::export]
pub fn settings_search(query: String) -> Vec<SettingsSearchHit> {
    let q = arbos_core::search::normalize_query(&query);
    core::search(&q, core::Platform::Iphone)
        .into_iter()
        .map(|h| SettingsSearchHit {
            entry: entry(h.entry),
            kind: h.kind.into(),
            keyword: match h.matched {
                core::SettingsMatched::Title => None,
                core::SettingsMatched::Keyword(k) => Some(k.to_string()),
            },
            order: h.order(),
            exact: h.is_exact(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_iphone_index_and_search_come_through() {
        let index = settings_index();
        assert!(index.iter().any(|e| e.item == SettingsItem::TextSize));
        assert!(index.iter().all(|e| e.item != SettingsItem::Updates));
        assert_eq!(
            index
                .iter()
                .find(|e| e.item == SettingsItem::OnThisDevice)
                .unwrap()
                .title,
            "On this iPhone"
        );
        let hits = settings_search("Opacity".into());
        assert_eq!(hits[0].entry.item, SettingsItem::AppearanceOpacity);
        assert_eq!(hits[0].keyword.as_deref(), Some("opacity"));
        assert!(hits[0].exact);
        let dev = settings_search("dev".into());
        assert_eq!(dev[0].entry.item, SettingsItem::Devices);
        assert_eq!(dev[0].kind, SearchMatchKind::Prefix);
        assert!(!dev[0].exact);
        assert!(settings_search("".into()).is_empty());
    }
}
