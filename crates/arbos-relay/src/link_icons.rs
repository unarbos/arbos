//! `GET /link-icon?domain=<host>` — the small picture the apps draw before an inline link
//! (Jacob, 2026-10-25: every `[Name](https://…)` in a reply carries its site's icon, as in
//! Grok Bot). Since [feedback link-icons-relay-2026-10-27] this is an alias of
//! `GET /unfurl/icon?host=`: both answer from the one site-icon resolver
//! ([`crate::site_icons`] — apple-touch-icon, then the largest declared icon, then
//! `/favicon.ico`, only ever ≥ 64 px, scaled down to 128, fetched once per host through the
//! pinned-DNS fetcher and cached by host + version). This module keeps the host and HTML
//! helpers the resolver and the unfurl module share: [`public_domain`], [`is_private_ip`],
//! [`meta_content`], [`tags`], [`attr`], [`absolute`].

use std::net::IpAddr;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde::Deserialize;

use crate::auth::Principal;
use crate::signin::api_error;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct LinkIconQuery {
    pub domain: String,
}

/// `GET /link-icon?domain=` for any signed-in principal (device, box or admin): the same PNG
/// (200) or 204 as `GET /unfurl/icon?host=`.
pub async fn get(
    State(state): State<AppState>,
    _principal: Principal,
    Query(q): Query<LinkIconQuery>,
) -> Response {
    let Some(domain) = public_domain(&q.domain) else {
        return api_error(
            StatusCode::BAD_REQUEST,
            "bad_domain",
            "domain must be a public host name",
        );
    };
    crate::site_icons::respond(crate::site_icons::resolve(&state, &domain).await)
}

/// The domain the apps may ask for: lower-case ASCII host name with at least one dot, no
/// port, no IP literal, nothing that names the machine itself or a private network by name.
pub fn public_domain(raw: &str) -> Option<String> {
    let d = raw.trim().trim_end_matches('.').to_ascii_lowercase();
    if d.is_empty() || d.len() > 253 || !d.contains('.') {
        return None;
    }
    if !d
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.')
    {
        return None;
    }
    if d.split('.')
        .any(|l| l.is_empty() || l.starts_with('-') || l.ends_with('-'))
    {
        return None;
    }
    if d.parse::<IpAddr>().is_ok() || d.split('.').all(|l| l.chars().all(|c| c.is_ascii_digit())) {
        return None;
    }
    if d == "localhost"
        || d.ends_with(".localhost")
        || d.ends_with(".local")
        || d.ends_with(".internal")
        || d.ends_with(".arpa")
    {
        return None;
    }
    Some(d)
}

/// True for addresses the relay must never fetch from: loopback, private, link-local,
/// multicast, unspecified, and the IPv4-mapped forms of those.
pub fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_multicast()
                || v4.is_unspecified()
                || v4.is_documentation()
                // Shared address space (CGNAT) and the "this network" block.
                || (v4.octets()[0] == 100 && (64..=127).contains(&v4.octets()[1]))
                || v4.octets()[0] == 0
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_private_ip(IpAddr::V4(v4));
            }
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                // fc00::/7 unique local, fe80::/10 link local.
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                || (v6.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}

/// `content` of the first `<meta property|name="<prop>" …>` in `html`, in either attribute order.
pub fn meta_content(html: &str, prop: &str) -> Option<String> {
    for tag in tags(html, "meta") {
        let is_prop = attr(&tag, "property")
            .or_else(|| attr(&tag, "name"))
            .is_some_and(|v| v.eq_ignore_ascii_case(prop));
        if is_prop {
            if let Some(c) = attr(&tag, "content") {
                if !c.trim().is_empty() {
                    return Some(c.trim().to_owned());
                }
            }
        }
    }
    None
}

/// Every `<name …>` tag of `html` (lower-cased name match), attributes text only.
pub(crate) fn tags(html: &str, name: &str) -> Vec<String> {
    let lower = html.to_ascii_lowercase();
    let open = format!("<{name}");
    let mut out = Vec::new();
    let mut from = 0usize;
    while let Some(i) = lower[from..].find(&open) {
        let start = from + i;
        let after = start + open.len();
        if !lower[after..].starts_with([' ', '\t', '\n', '\r', '/', '>']) {
            from = after;
            continue;
        }
        let Some(end) = lower[after..].find('>') else {
            break;
        };
        out.push(html[after..after + end].to_owned());
        from = after + end + 1;
    }
    out
}

/// The value of attribute `key` in a tag's attribute text (quoted or bare).
pub(crate) fn attr(tag: &str, key: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0usize;
    while let Some(i) = lower[from..].find(key) {
        let at = from + i;
        let before_ok = at == 0 || !lower.as_bytes()[at - 1].is_ascii_alphanumeric();
        let rest = &tag[at + key.len()..];
        let rest_trim = rest.trim_start();
        if before_ok && rest_trim.starts_with('=') {
            let v = rest_trim[1..].trim_start();
            return Some(match v.chars().next() {
                Some(q @ ('"' | '\'')) => v[1..].split(q).next().unwrap_or("").to_owned(),
                _ => v
                    .split([' ', '>', '\t', '\n', '/'])
                    .next()
                    .unwrap_or("")
                    .to_owned(),
            });
        }
        from = at + key.len();
    }
    None
}

/// `href` made absolute against `base_url` (scheme-relative and root-relative forms too).
pub fn absolute(base_url: &str, href: &str) -> Option<String> {
    let base = reqwest::Url::parse(base_url).ok()?;
    let href = href.trim();
    let u = if let Some(rest) = href.strip_prefix("//") {
        format!("{}://{}", base.scheme(), rest)
    } else {
        base.join(href).ok()?.to_string()
    };
    Some(u)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_public_host_names_pass() {
        assert_eq!(
            public_domain(" www.ElVirrey.com. "),
            Some("www.elvirrey.com".into())
        );
        assert_eq!(public_domain("nypost.com"), Some("nypost.com".into()));
        for bad in [
            "",
            "localhost",
            "foo.localhost",
            "box.local",
            "relay.internal",
            "127.0.0.1",
            "10.0.0.1",
            "::1",
            "nodots",
            "a..b",
            "-a.com",
            "a-.com",
            "a.com:443",
            "a.com/x",
            "1.2.3.4.in-addr.arpa",
            "exa mple.com",
        ] {
            assert_eq!(public_domain(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn private_addresses_are_refused() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.9",
            "192.168.1.1",
            "169.254.169.254",
            "0.0.0.0",
            "100.64.0.1",
            "224.0.0.1",
            "::1",
            "fe80::1",
            "fc00::1",
            "::ffff:10.0.0.1",
        ] {
            assert!(is_private_ip(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["8.8.8.8", "104.21.5.6", "2606:4700::1111"] {
            assert!(!is_private_ip(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn page_parsing_finds_meta_content_and_resolves_hrefs() {
        let html = r#"<!doctype html><html><head>
            <META name="viewport" content="width=device-width">
            <meta content="https://cdn.example.com/og.jpg" property="og:image">
            <link rel="icon" sizes="192x192" href="//static.example.com/192.png">
            <link rel="stylesheet" href="/x.css"></head><body><metadata>no</metadata></body></html>"#;
        assert_eq!(
            meta_content(html, "og:image").as_deref(),
            Some("https://cdn.example.com/og.jpg")
        );
        assert_eq!(meta_content(html, "twitter:image"), None);
        assert_eq!(tags(html, "meta").len(), 2, "<metadata> is not a <meta>");
        assert_eq!(
            tags(html, "link")
                .iter()
                .filter_map(|t| attr(t, "href"))
                .collect::<Vec<_>>(),
            ["//static.example.com/192.png", "/x.css"]
        );
        assert_eq!(
            absolute(
                "https://www.example.com/a/b",
                "//static.example.com/192.png"
            )
            .as_deref(),
            Some("https://static.example.com/192.png")
        );
        assert_eq!(
            absolute("https://www.example.com/a/b", "/favicon.ico").as_deref(),
            Some("https://www.example.com/favicon.ico")
        );
        assert_eq!(
            absolute("https://www.example.com/a/b", "icons/x.png").as_deref(),
            Some("https://www.example.com/a/icons/x.png")
        );
    }
}
