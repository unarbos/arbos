//! Agent-hosted sites (`arbos.life/<user>/<app>/`, design issue #2060): the rules every side
//! applies the same way — the box when it packs `~/sites/<app>/`, the relay when it accepts a
//! publish, and (mirrored by hand) the edge Worker when it serves. Pure functions, no I/O.
//!
//! A site is a flat snapshot of static files. Names, types and sizes are checked here; the
//! relay derives the owner from the box token, never from the request.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Sum of file sizes of one site.
pub const MAX_SITE_BYTES: u64 = 25 * 1024 * 1024;
/// One file.
pub const MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;
/// Files per site.
pub const MAX_FILES: usize = 500;
/// `a/b/c/file` depth (segments).
pub const MAX_DEPTH: usize = 8;
/// One path segment.
pub const MAX_SEGMENT_CHARS: usize = 100;
/// Default per-user ceilings; the relay may raise them per user.
pub const DEFAULT_MAX_SITES: i64 = 10;
pub const DEFAULT_MAX_USER_BYTES: i64 = 100 * 1024 * 1024;
/// Publishes per user per rolling hour / UTC day.
pub const MAX_PUBLISHES_PER_HOUR: i64 = 30;
pub const MAX_PUBLISHES_PER_DAY: i64 = 200;
/// `<app>` shape: `[a-z0-9][a-z0-9-]{0,39}`.
pub const MAX_APP_CHARS: usize = 40;

/// Subdomains no user may claim (Jacob, 2026-10-27): system and mail names, every DNS record
/// name that exists in the `arbos.life` zone today, and anything a future service is likely to
/// want. Checked after `normalize_handle`, so compare lower-case. Any name with a leading
/// underscore is refused by `check_subdomain` without being listed.
pub const RESERVED_SUBDOMAINS: &[&str] = &[
    // system
    "www",
    "api",
    "app",
    "relay",
    "admin",
    "mail",
    "m",
    "static",
    "cdn",
    "assets",
    "blob",
    "img",
    "media",
    "docs",
    "help",
    "support",
    "status",
    "dev",
    "staging",
    "test",
    "beta",
    "login",
    "auth",
    "oauth",
    "sso",
    "account",
    "accounts",
    "billing",
    "pay",
    "shop",
    "store",
    "blog",
    "news",
    // mail / infra
    "mx",
    "smtp",
    "imap",
    "pop",
    "pop3",
    "ftp",
    "sftp",
    "ns",
    "ns1",
    "ns2",
    "ns3",
    "ns4",
    "autodiscover",
    "autoconfig",
    "webmail",
    "email",
    "mta",
    "dmarc",
    "dkim",
    "spf",
    "acme",
    // records in the arbos.life zone (2026-10-27)
    "livekit",
    "turn",
    "headless-engine",
    "sn120",
    // ours
    "arbos",
    "sites",
    "site",
    "edge",
    "worker",
    "workers",
    "r2",
    "preview",
    "demo",
    "ops",
    "root",
    "localhost",
    "local",
    "internal",
    "proxy",
    "vpn",
    "git",
    "ci",
    "hub",
    "voice",
    "voice-api",
    "hub-api",
    "download",
    "downloads",
    "update",
    "updates",
    "release",
    "releases",
];

/// First URL segments the landing page, the relay or the apps already use. Kept for the `path`
/// scheme (and refused as subdomains too, so the two schemes never disagree about a name).
pub const RESERVED_PREFIXES: &[&str] = &[
    "assets",
    "s",
    "g",
    "i",
    "updates",
    "download",
    "privacy",
    "welcome",
    "favicon.ico",
    "favicon.svg",
    "apple-touch-icon.png",
    "robots.txt",
    "sitemap.xml",
    "index.html",
    "main.js",
    "style.css",
    "api",
    "admin",
    "sites",
    "site",
    "static",
    "u",
    "www",
    "arbos",
    "login",
    "auth",
    "about",
    "blog",
    "docs",
    "help",
    "support",
    "status",
    "terms",
];

/// Allowed file extensions and the `Content-Type` served for each.
pub const ALLOWED_TYPES: &[(&str, &str)] = &[
    ("html", "text/html; charset=utf-8"),
    ("htm", "text/html; charset=utf-8"),
    ("css", "text/css; charset=utf-8"),
    ("js", "text/javascript; charset=utf-8"),
    ("mjs", "text/javascript; charset=utf-8"),
    ("json", "application/json; charset=utf-8"),
    ("map", "application/json; charset=utf-8"),
    ("webmanifest", "application/manifest+json; charset=utf-8"),
    ("txt", "text/plain; charset=utf-8"),
    ("md", "text/markdown; charset=utf-8"),
    ("xml", "application/xml; charset=utf-8"),
    ("svg", "image/svg+xml"),
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("avif", "image/avif"),
    ("ico", "image/x-icon"),
    ("woff", "font/woff"),
    ("woff2", "font/woff2"),
    ("ttf", "font/ttf"),
    ("otf", "font/otf"),
    ("mp3", "audio/mpeg"),
    ("mp4", "video/mp4"),
    ("webm", "video/webm"),
    ("pdf", "application/pdf"),
    ("wasm", "application/wasm"),
];

/// File names that are secrets or tooling by convention, refused whatever their extension.
const SECRET_NAME_PATTERNS: &[&str] = &[".env", ".pem", ".key", ".p12", ".pfx", ".kdbx"];
const SECRET_NAME_PREFIXES: &[&str] = &["id_rsa", "id_ed25519", "id_ecdsa", "id_dsa"];

/// One file of a site as the box announces it and the relay records it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SiteFile {
    /// Relative path inside the site, `/`-separated, no leading slash (`index.html`, `css/a.css`).
    pub path: String,
    pub size: u64,
    /// Lower-case hex SHA-256 of the bytes.
    pub sha256: String,
}

/// What the edge serves from: path → blob, plus the content type per path.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SiteManifest {
    pub prefix: String,
    pub app: String,
    pub version: i64,
    /// `live` | `down` (admin takedown; the edge answers 410).
    pub status: String,
    pub published_at: i64,
    /// path → `{sha256, size, content_type}`.
    pub files: BTreeMap<String, ManifestEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManifestEntry {
    pub sha256: String,
    pub size: u64,
    pub content_type: String,
}

/// Why a name, path or file set is refused. The text is shown to the agent as is.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct SiteRule(pub String);

fn rule(s: impl Into<String>) -> SiteRule {
    SiteRule(s.into())
}

/// `<app>`: `[a-z0-9][a-z0-9-]{0,39}`, no leading/trailing or double dash.
pub fn check_app(app: &str) -> Result<(), SiteRule> {
    if app.is_empty() || app.len() > MAX_APP_CHARS {
        return Err(rule(format!(
            "app name must be 1–{MAX_APP_CHARS} characters of a-z, 0-9 and dashes"
        )));
    }
    if !app
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err(rule(
            "app name may only use lower-case letters, digits and dashes (for example `portfolio`, `recipes-2026`)",
        ));
    }
    if app.starts_with('-') || app.ends_with('-') || app.contains("--") {
        return Err(rule(
            "app name cannot start or end with a dash or contain `--`",
        ));
    }
    Ok(())
}

/// Longest DNS label.
pub const MAX_SUBDOMAIN_CHARS: usize = 63;

/// A handle as a DNS label: lower-case ASCII, every run of characters that are not `a-z0-9`
/// becomes one `-`, leading/trailing `-` stripped, at most 63 characters, never IDN (non-ASCII
/// is a separator, not punycode). `None` when nothing usable is left.
pub fn normalize_handle(handle: &str) -> Option<String> {
    let mut out = String::with_capacity(handle.len());
    let mut dash = false;
    for c in handle.trim().chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        return None;
    }
    let mut s: String = trimmed.chars().take(MAX_SUBDOMAIN_CHARS).collect();
    while s.ends_with('-') {
        s.pop();
    }
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// May this (already normalized) label be a user's subdomain? Refuses the reserved lists,
/// leading underscores (`_dmarc`, `_acme-challenge`, …), bad shapes and over-long labels. The
/// text names the reason so the claim can be refused with it.
pub fn check_subdomain(sub: &str) -> Result<(), SiteRule> {
    if sub.is_empty() || sub.len() > MAX_SUBDOMAIN_CHARS {
        return Err(rule(format!(
            "`{sub}`: a subdomain must be 1–{MAX_SUBDOMAIN_CHARS} characters"
        )));
    }
    if sub.starts_with('_') || sub.contains('_') {
        return Err(rule(format!(
            "`{sub}`: names with an underscore are reserved for DNS service records"
        )));
    }
    if !sub
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        || sub.starts_with('-')
        || sub.ends_with('-')
    {
        return Err(rule(format!(
            "`{sub}`: a subdomain uses lower-case letters, digits and inner dashes only"
        )));
    }
    if RESERVED_SUBDOMAINS.contains(&sub) || RESERVED_PREFIXES.contains(&sub) {
        return Err(rule(format!(
            "`{sub}` is a reserved system name and cannot be a user's site address"
        )));
    }
    if sub.starts_with("xn--") {
        return Err(rule(format!(
            "`{sub}`: internationalised (punycode) names are not supported"
        )));
    }
    Ok(())
}

/// The subdomain a handle would claim, or why it cannot.
pub fn subdomain_for_handle(handle: &str) -> Result<String, SiteRule> {
    let Some(sub) = normalize_handle(handle) else {
        return Err(rule(format!(
            "`{handle}` has no letters or digits that can form a site address"
        )));
    };
    check_subdomain(&sub)?;
    Ok(sub)
}

/// Where a site's URL puts the user and the app. Jacob's choice (2026-10-27) is `Subdomain`;
/// the others stay selectable so the layout can change by config alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Scheme {
    /// `https://<user>.<domain>/<app>/` — one wildcard certificate, one origin per user.
    Subdomain,
    /// `https://<domain>/<user>/<app>/`.
    Path,
    /// `https://<app>.<user>.<domain>/` — needs multi-level certificates.
    TwoLevel,
}

impl Scheme {
    pub fn parse(s: &str) -> Option<Scheme> {
        match s.trim().to_ascii_lowercase().as_str() {
            "subdomain" | "sub" => Some(Scheme::Subdomain),
            "path" => Some(Scheme::Path),
            "two-level" | "twolevel" | "two_level" => Some(Scheme::TwoLevel),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Scheme::Subdomain => "subdomain",
            Scheme::Path => "path",
            Scheme::TwoLevel => "two-level",
        }
    }
}

/// The one routing function: scheme + domain → URLs, and back from a request. The edge Worker
/// mirrors `parse_request` for the same scheme/domain pair it is deployed with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    pub scheme: Scheme,
    /// Apex the sites hang off (`arbos.life`; a separate user-content domain later).
    pub domain: String,
}

/// What a request resolves to: the owner's prefix, the app, and the path inside the site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub prefix: String,
    pub app: String,
    /// Path inside the site without the leading slash (`""` for the site root).
    pub rest: String,
}

impl Route {
    pub fn new(scheme: Scheme, domain: &str) -> Route {
        Route {
            scheme,
            domain: domain.trim().trim_matches('.').to_ascii_lowercase(),
        }
    }

    /// `https://<prefix>.<domain>/<app>/` (or the scheme's equivalent).
    pub fn public_url(&self, prefix: &str, app: &str) -> String {
        match self.scheme {
            Scheme::Subdomain => format!("https://{prefix}.{}/{app}/", self.domain),
            Scheme::Path => format!("https://{}/{prefix}/{app}/", self.domain),
            Scheme::TwoLevel => format!("https://{app}.{prefix}.{}/", self.domain),
        }
    }

    /// The user's landing (list of their sites), where the scheme has one.
    pub fn user_url(&self, prefix: &str) -> Option<String> {
        match self.scheme {
            Scheme::Subdomain => Some(format!("https://{prefix}.{}/", self.domain)),
            Scheme::Path | Scheme::TwoLevel => None,
        }
    }

    /// Hostname (lower-case, no port) + path → the site it names, or `None` when the request is
    /// not a site at all (the apex, a reserved host, a bad segment) and must pass through.
    pub fn parse_request(&self, host: &str, path: &str) -> Option<Resolved> {
        let host = host.split(':').next().unwrap_or("").to_ascii_lowercase();
        let suffix = format!(".{}", self.domain);
        let segs: Vec<&str> = path.trim_start_matches('/').splitn(3, '/').collect();
        match self.scheme {
            Scheme::Subdomain => {
                let sub = host.strip_suffix(suffix.as_str())?;
                if sub.contains('.') || check_subdomain(sub).is_err() {
                    return None;
                }
                let app = *segs.first()?;
                check_app(app).ok()?;
                let rest = match segs.get(1) {
                    Some(r1) => {
                        let mut r = (*r1).to_owned();
                        if let Some(r2) = segs.get(2) {
                            r.push('/');
                            r.push_str(r2);
                        }
                        r
                    }
                    None => String::new(),
                };
                Some(Resolved {
                    prefix: sub.to_owned(),
                    app: app.to_owned(),
                    rest,
                })
            }
            Scheme::Path => {
                if host != self.domain {
                    return None;
                }
                let prefix = *segs.first()?;
                let app = *segs.get(1)?;
                if check_subdomain(prefix).is_err() || check_app(app).is_err() {
                    return None;
                }
                Some(Resolved {
                    prefix: prefix.to_owned(),
                    app: app.to_owned(),
                    rest: segs.get(2).map(|s| (*s).to_owned()).unwrap_or_default(),
                })
            }
            Scheme::TwoLevel => {
                let labels = host.strip_suffix(suffix.as_str())?;
                let (app, prefix) = labels.split_once('.')?;
                if prefix.contains('.')
                    || check_subdomain(prefix).is_err()
                    || check_app(app).is_err()
                {
                    return None;
                }
                Some(Resolved {
                    prefix: prefix.to_owned(),
                    app: app.to_owned(),
                    rest: path.trim_start_matches('/').to_owned(),
                })
            }
        }
    }
}

/// A relative file path inside a site. Refuses absolute paths, `.`/`..` segments, backslashes,
/// control characters, dotfiles and dot-directories, secret-looking names, over-deep or
/// over-long paths, and extensions not in `ALLOWED_TYPES`. Returns the content type.
pub fn check_path(path: &str) -> Result<&'static str, SiteRule> {
    if path.is_empty() || path.len() > 512 {
        return Err(rule(format!("`{path}`: path must be 1–512 characters")));
    }
    if path.starts_with('/') || path.contains('\\') {
        return Err(rule(format!(
            "`{path}`: paths are relative to the site folder and use `/`"
        )));
    }
    if path.chars().any(|c| c.is_control() || c == '\u{0}') {
        return Err(rule(format!(
            "`{path}`: control characters are not allowed"
        )));
    }
    let segments: Vec<&str> = path.split('/').collect();
    if segments.len() > MAX_DEPTH {
        return Err(rule(format!("`{path}`: at most {MAX_DEPTH} path segments")));
    }
    for s in &segments {
        if s.is_empty() || *s == "." || *s == ".." {
            return Err(rule(format!(
                "`{path}`: empty, `.` or `..` path segments are not allowed"
            )));
        }
        if s.chars().count() > MAX_SEGMENT_CHARS {
            return Err(rule(format!(
                "`{path}`: a path segment may have at most {MAX_SEGMENT_CHARS} characters"
            )));
        }
        if s.starts_with('.') {
            return Err(rule(format!(
                "`{path}`: dotfiles and dot-directories (`.git`, `.env`, …) are never published"
            )));
        }
        if s.contains(':')
            || s.contains('*')
            || s.contains('?')
            || s.contains('"')
            || s.contains('<')
            || s.contains('>')
            || s.contains('|')
        {
            return Err(rule(format!(
                "`{path}`: `: * ? \" < > |` are not allowed in file names"
            )));
        }
    }
    let name = segments[segments.len() - 1];
    let lower = name.to_ascii_lowercase();
    if SECRET_NAME_PATTERNS.iter().any(|p| lower.contains(p))
        || SECRET_NAME_PREFIXES.iter().any(|p| lower.starts_with(p))
    {
        return Err(rule(format!(
            "`{path}`: looks like a key or secret file; sites never carry those"
        )));
    }
    let ext = match lower.rsplit_once('.') {
        Some((_, e)) if !e.is_empty() => e,
        _ => {
            return Err(rule(format!(
                "`{path}`: every file needs one of the allowed extensions ({})",
                allowed_extensions()
            )))
        }
    };
    match ALLOWED_TYPES.iter().find(|(e, _)| *e == ext) {
        Some((_, ct)) => Ok(ct),
        None => Err(rule(format!(
            "`{path}`: `.{ext}` is not an allowed type ({})",
            allowed_extensions()
        ))),
    }
}

/// `html, htm, css, …` for messages.
pub fn allowed_extensions() -> String {
    ALLOWED_TYPES
        .iter()
        .map(|(e, _)| *e)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Whole-set rules: file count, per-file and total size, duplicates, `index.html` present,
/// every sha a 64-hex string.
pub fn check_files(files: &[SiteFile]) -> Result<(), SiteRule> {
    if files.is_empty() {
        return Err(rule("a site needs at least `index.html`"));
    }
    if files.len() > MAX_FILES {
        return Err(rule(format!(
            "{} files; a site may have at most {MAX_FILES}",
            files.len()
        )));
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut total: u64 = 0;
    for f in files {
        check_path(&f.path)?;
        if !seen.insert(f.path.as_str()) {
            return Err(rule(format!("`{}` is listed twice", f.path)));
        }
        if f.size > MAX_FILE_BYTES {
            return Err(rule(format!(
                "`{}` is {} MB; one file may be at most {} MB",
                f.path,
                f.size / (1024 * 1024),
                MAX_FILE_BYTES / (1024 * 1024)
            )));
        }
        if f.sha256.len() != 64
            || !f.sha256.bytes().all(|b| b.is_ascii_hexdigit())
            || f.sha256.bytes().any(|b| b.is_ascii_uppercase())
        {
            return Err(rule(format!(
                "`{}`: sha256 must be 64 lower-case hex characters",
                f.path
            )));
        }
        total = total.saturating_add(f.size);
    }
    if total > MAX_SITE_BYTES {
        return Err(rule(format!(
            "site is {:.1} MB; the limit is {} MB in total",
            total as f64 / (1024.0 * 1024.0),
            MAX_SITE_BYTES / (1024 * 1024)
        )));
    }
    if !seen.contains("index.html") {
        return Err(rule("a site needs an `index.html` at its root"));
    }
    Ok(())
}

/// Cheap phishing refusals on an HTML file's text: a form that collects a password, or a
/// meta refresh that sends visitors to another host. Not a classifier; catches the obvious.
pub fn check_html(path: &str, text: &str) -> Result<(), SiteRule> {
    let lower = text.to_ascii_lowercase();
    let password_input = ["type=\"password\"", "type='password'", "type=password"]
        .iter()
        .any(|p| lower.contains(p));
    if lower.contains("<form") && password_input {
        return Err(rule(format!(
            "`{path}`: a form that asks for a password is not allowed on a hosted site (sites are static and cannot collect sign-ins)"
        )));
    }
    if let Some(i) = lower
        .find("http-equiv=\"refresh\"")
        .or_else(|| lower.find("http-equiv='refresh'"))
    {
        let tail = &lower[i..lower.len().min(i + 400)];
        if tail.contains("url=http://") || tail.contains("url=https://") || tail.contains("url=//")
        {
            return Err(rule(format!(
                "`{path}`: a meta refresh to another site is not allowed (no redirects from hosted sites)"
            )));
        }
    }
    Ok(())
}

/// Resolve a request path inside a site to a manifest key: `""` or `index.html` → `index.html`,
/// a directory path → `<dir>/index.html`. Returns `None` for paths the rules refuse.
pub fn resolve_request_path(rest: &str) -> Option<String> {
    let rest = rest.trim_start_matches('/');
    let key = if rest.is_empty() {
        "index.html".to_owned()
    } else if rest.ends_with('/') {
        format!("{rest}index.html")
    } else {
        rest.to_owned()
    };
    check_path(&key).ok().map(|_| key)
}
