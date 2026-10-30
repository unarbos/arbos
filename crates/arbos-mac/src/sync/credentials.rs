//! Credential storage. macOS: the login keychain via `security(1)`. Elsewhere:
//! a 0600 JSON file under the config dir. `ARBOS_RELAY_TOKEN` (+
//! `ARBOS_DEVICE_ID`) override both.
//!
//! Two shapes live here: the self-serve JWT pair from `POST /auth/token`
//! (access + rotating refresh token, both bound to the device) and the legacy
//! `adv_` device token from `/pair`, which the relay keeps honouring as a
//! bearer. A legacy token has no refresh token and never expires.

use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

use crate::model::now_ms;

const KEYCHAIN_SERVICE: &str = "sh.arbos.mac.relay";

/// `user` from `/auth/token` and `/me`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AccountUser {
    #[serde(default)]
    pub id: String,
    #[serde(default, deserialize_with = "crate::model::null_to_empty")]
    pub login: String,
    #[serde(default, deserialize_with = "crate::model::null_to_empty")]
    pub avatar_url: String,
    #[serde(default, deserialize_with = "crate::model::null_to_empty")]
    pub name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeviceCredentials {
    pub relay_url: String,
    pub device_id: String,
    /// Bearer for every HTTP call and `?token=` on WebSocket dials.
    #[serde(alias = "device_token")]
    pub access_token: String,
    /// `arf_…`, rotates on every refresh. `None` for legacy `adv_` tokens.
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// Unix ms when the access token expires; 0 = never (legacy).
    #[serde(default)]
    pub expires_at: u64,
    #[serde(default)]
    pub user: Option<AccountUser>,
}

impl DeviceCredentials {
    /// Refresh when the relay's 5-minute rule says so.
    pub fn needs_refresh(&self) -> bool {
        self.refresh_token.is_some()
            && self.expires_at > 0
            && self.expires_at.saturating_sub(now_ms()) < 5 * 60_000
    }
}

pub fn config_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("ARBOS_MAC_CONFIG") {
        return PathBuf::from(dir);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".config").join("arbos")
}

fn file_path(relay_url: &str) -> PathBuf {
    let slug: String = relay_url
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    config_dir().join(format!("mac-device-{slug}.json"))
}

pub fn load(relay_url: &str) -> Option<DeviceCredentials> {
    if let Ok(token) = std::env::var("ARBOS_RELAY_TOKEN") {
        if !token.trim().is_empty() {
            return Some(DeviceCredentials {
                relay_url: relay_url.to_string(),
                device_id: std::env::var("ARBOS_DEVICE_ID").unwrap_or_else(|_| "mac:env".into()),
                access_token: token.trim().to_string(),
                refresh_token: None,
                expires_at: 0,
                user: None,
            });
        }
    }
    if cfg!(target_os = "macos") {
        if let Some(c) = load_keychain(relay_url) {
            return Some(c);
        }
    }
    let bytes = std::fs::read(file_path(relay_url)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub fn store(creds: &DeviceCredentials) -> Result<()> {
    if cfg!(target_os = "macos") && store_keychain(creds).is_ok() {
        return Ok(());
    }
    let path = file_path(&creds.relay_url);
    if let Some(parent) = path.parent() {
        super::store::make_owner_only_dir(parent);
    }
    let json = serde_json::to_vec_pretty(creds)?;
    std::fs::write(&path, json).with_context(|| format!("write {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// Sign-out: forget the tokens everywhere they could be.
pub fn clear(relay_url: &str) {
    if cfg!(target_os = "macos") {
        let _ = Command::new("security")
            .args([
                "delete-generic-password",
                "-s",
                KEYCHAIN_SERVICE,
                "-a",
                relay_url,
            ])
            .output();
    }
    let _ = std::fs::remove_file(file_path(relay_url));
}

fn load_keychain(relay_url: &str) -> Option<DeviceCredentials> {
    let out = Command::new("security")
        .args([
            "find-generic-password",
            "-s",
            KEYCHAIN_SERVICE,
            "-a",
            relay_url,
            "-w",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let secret = String::from_utf8(out.stdout).ok()?;
    serde_json::from_str(secret.trim()).ok()
}

fn store_keychain(creds: &DeviceCredentials) -> Result<()> {
    let secret = serde_json::to_string(creds)?;
    let status = Command::new("security")
        .args([
            "add-generic-password",
            "-U",
            "-s",
            KEYCHAIN_SERVICE,
            "-a",
            &creds.relay_url,
            "-w",
            &secret,
        ])
        .status()?;
    anyhow::ensure!(status.success(), "security add-generic-password failed");
    Ok(())
}

/// Where downloaded blobs go.
/// Cached voice samples (`GET /voices` `sample_url`), keyed by URL hash.
pub fn voice_sample_cache_dir() -> PathBuf {
    config_dir().join("voice-samples")
}

pub fn downloads_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    let dl = PathBuf::from(&home).join("Downloads");
    if dl.is_dir() {
        dl
    } else {
        PathBuf::from(home)
    }
}
