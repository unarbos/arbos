//! Sparkle 2 appcast: read it (the app) and write it (the release pipeline).
//!
//! ```xml
//! <rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle">
//!  <channel><title>Arbos</title>
//!   <item>
//!     <title>0.1.0.212</title>
//!     <sparkle:version>212</sparkle:version>
//!     <sparkle:shortVersionString>0.1.0.212</sparkle:shortVersionString>
//!     <sparkle:minimumSystemVersion>14.0</sparkle:minimumSystemVersion>
//!     <pubDate>Thu, 15 Oct 2026 04:00:00 +0000</pubDate>
//!     <link>https://github.com/unarbos/arbos/releases/tag/v0.1.0.212</link>
//!     <description><![CDATA[…]]></description>
//!     <enclosure url="…/Arbos-0.1.0.212-macos-universal.tar.gz" length="…"
//!                type="application/octet-stream" sparkle:os="macos"
//!                sparkle:edSignature="…"/>
//!   </item>
//!  </channel></rss>
//! ```
//!
//! One `<item>` per build; one `<enclosure>` per OS inside it (Sparkle reads
//! `sparkle:os`; macOS is the default when it is absent).

use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};

use crate::version::{is_newer, Version};

pub const SPARKLE_NS: &str = "http://www.andymatuschak.org/xml-namespaces/sparkle";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Enclosure {
    pub url: String,
    pub length: u64,
    /// `macos`, `linux`, … (`macos` when the feed omits it).
    pub os: String,
    pub ed_signature: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub title: String,
    /// The full build version the app orders on (`0.1.0.212`).
    pub version: String,
    /// `sparkle:version`: the build number alone (`212`), for Sparkle.
    pub build: String,
    pub minimum_system_version: Option<String>,
    pub pub_date: Option<String>,
    pub link: Option<String>,
    pub notes: Option<String>,
    pub enclosures: Vec<Enclosure>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Appcast {
    pub title: String,
    pub items: Vec<Item>,
}

impl Item {
    pub fn enclosure_for(&self, os: &str) -> Option<&Enclosure> {
        self.enclosures.iter().find(|e| e.os == os)
    }

    pub fn parsed_version(&self) -> Option<Version> {
        Version::parse(&self.version)
    }
}

impl Appcast {
    pub fn parse(xml: &str) -> anyhow::Result<Appcast> {
        let doc = roxmltree::Document::parse(xml).context("appcast is not well-formed XML")?;
        let rss = doc.root_element();
        if rss.tag_name().name() != "rss" {
            bail!("appcast root is <{}>, want <rss>", rss.tag_name().name());
        }
        let channel = rss
            .children()
            .find(|n| n.has_tag_name("channel"))
            .context("appcast has no <channel>")?;
        let title = child_text(channel, "title").unwrap_or_else(|| "Arbos".to_string());
        let mut items = Vec::new();
        for node in channel.children().filter(|n| n.has_tag_name("item")) {
            let short = sparkle_text(node, "shortVersionString");
            let build = sparkle_text(node, "version").unwrap_or_default();
            let version = short.clone().unwrap_or_else(|| build.clone());
            if Version::parse(&version).is_none() {
                tracing::warn!(version, "skipping appcast item with an unreadable version");
                continue;
            }
            let mut enclosures = Vec::new();
            for enc in node.children().filter(|n| n.has_tag_name("enclosure")) {
                let url = enc.attribute("url").unwrap_or_default().to_string();
                let length = enc
                    .attribute("length")
                    .and_then(|l| l.parse().ok())
                    .unwrap_or(0);
                let os = enc
                    .attribute((SPARKLE_NS, "os"))
                    .unwrap_or("macos")
                    .to_string();
                let ed_signature = enc
                    .attribute((SPARKLE_NS, "edSignature"))
                    .unwrap_or_default()
                    .to_string();
                if url.is_empty() {
                    continue;
                }
                enclosures.push(Enclosure {
                    url,
                    length,
                    os,
                    ed_signature,
                });
            }
            items.push(Item {
                title: child_text(node, "title").unwrap_or_else(|| version.clone()),
                version,
                build,
                minimum_system_version: sparkle_text(node, "minimumSystemVersion"),
                pub_date: child_text(node, "pubDate"),
                link: child_text(node, "link"),
                notes: child_text(node, "description"),
                enclosures,
            });
        }
        Ok(Appcast { title, items })
    }

    /// The newest item that is newer than `current`, has a payload for `os`,
    /// and (on macOS) does not need a newer system than `system_version`.
    pub fn newest_for(
        &self,
        current: &str,
        os: &str,
        system_version: Option<&str>,
    ) -> Option<&Item> {
        self.items
            .iter()
            .filter(|item| is_newer(&item.version, current))
            .filter(|item| item.enclosure_for(os).is_some())
            .filter(
                |item| match (&item.minimum_system_version, system_version) {
                    (Some(min), Some(have)) => !is_newer(min, have),
                    _ => true,
                },
            )
            .max_by(|a, b| a.parsed_version().cmp(&b.parsed_version()))
    }

    /// Every item of every feed, newest first: what the app knows about once it
    /// has asked each place that serves the feed.
    ///
    /// A version named by more than one feed is taken from the first feed that
    /// named it. Two feeds disagreeing about one version's payload is not
    /// settled here and does not need to be — the Ed25519 check over the bytes
    /// decides what installs, whichever feed described it.
    pub fn union(feeds: Vec<Appcast>) -> Appcast {
        let mut out = Appcast::default();
        for feed in feeds {
            if out.title.is_empty() {
                out.title = feed.title;
            }
            for item in feed.items {
                if !out.items.iter().any(|i| i.version == item.version) {
                    out.items.push(item);
                }
            }
        }
        out.items
            .sort_by_key(|i| std::cmp::Reverse(i.parsed_version()));
        out
    }

    /// Prepend `item` (replacing any item of the same version) and keep the
    /// newest `keep` items.
    pub fn push_newest(&mut self, item: Item, keep: usize) {
        self.items.retain(|i| i.version != item.version);
        self.items.insert(0, item);
        self.items
            .sort_by_key(|i| std::cmp::Reverse(i.parsed_version()));
        self.items.truncate(keep.max(1));
    }

    pub fn to_xml(&self) -> String {
        let mut out = String::new();
        out.push_str("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
        out.push_str(&format!(
            "<rss version=\"2.0\" xmlns:sparkle=\"{SPARKLE_NS}\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\n"
        ));
        out.push_str("  <channel>\n");
        out.push_str(&format!("    <title>{}</title>\n", escape(&self.title)));
        for item in &self.items {
            out.push_str("    <item>\n");
            out.push_str(&format!("      <title>{}</title>\n", escape(&item.title)));
            out.push_str(&format!(
                "      <sparkle:version>{}</sparkle:version>\n",
                escape(&item.build)
            ));
            out.push_str(&format!(
                "      <sparkle:shortVersionString>{}</sparkle:shortVersionString>\n",
                escape(&item.version)
            ));
            if let Some(min) = &item.minimum_system_version {
                out.push_str(&format!(
                    "      <sparkle:minimumSystemVersion>{}</sparkle:minimumSystemVersion>\n",
                    escape(min)
                ));
            }
            if let Some(date) = &item.pub_date {
                out.push_str(&format!("      <pubDate>{}</pubDate>\n", escape(date)));
            }
            if let Some(link) = &item.link {
                out.push_str(&format!("      <link>{}</link>\n", escape(link)));
            }
            if let Some(notes) = &item.notes {
                out.push_str(&format!(
                    "      <description><![CDATA[{}]]></description>\n",
                    notes.replace("]]>", "]]]]><![CDATA[>")
                ));
            }
            for enc in &item.enclosures {
                out.push_str(&format!(
                    "      <enclosure url=\"{}\" length=\"{}\" type=\"application/octet-stream\" sparkle:os=\"{}\" sparkle:edSignature=\"{}\"/>\n",
                    escape(&enc.url),
                    enc.length,
                    escape(&enc.os),
                    escape(&enc.ed_signature)
                ));
            }
            out.push_str("    </item>\n");
        }
        out.push_str("  </channel>\n</rss>\n");
        out
    }
}

fn child_text(node: roxmltree::Node, name: &str) -> Option<String> {
    node.children()
        .find(|n| n.has_tag_name(name))
        .and_then(|n| n.text())
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

fn sparkle_text(node: roxmltree::Node, name: &str) -> Option<String> {
    node.children()
        .find(|n| n.has_tag_name((SPARKLE_NS, name)))
        .and_then(|n| n.text())
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// RFC 2822 date for `<pubDate>`, now.
pub fn pub_date_now() -> String {
    chrono::Utc::now().to_rfc2822()
}
