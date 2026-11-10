//! Signing the relay address the relay pushes into a box.
//!
//! The address itself has to stay changeable — the relay's public URL moves whenever a quick
//! tunnel restarts, which is why a box reads it from a file at every wake rather than only from
//! the environment it was created with. What must not be changeable is *who may name one*. So the
//! address travels with a detached Ed25519 signature, and the box verifies it against a public key
//! handed to it in its create-time environment.
//!
//! The box only ever holds the public key, so a box cannot name an address either; and the
//! verification is offline, so nothing is sent to an address before it has been shown to be ours.
//! [`arbos_core::relay_url`] owns the format both sides agree on.

use arbos_core::relay_url::{self, SignedRelayUrl};
use base64::Engine;
use ed25519_dalek::{Signer, SigningKey};
use std::sync::OnceLock;

/// Base64 of the 32-byte Ed25519 seed. Unset leaves boxes without a verify key, and agentd then
/// keeps its pre-signature behaviour, so a relay that has never been given a key still works.
pub const SIGNING_KEY_ENV: &str = "ARBOS_RELAY_URL_SIGNING_KEY";
/// `ARBOS_RELAY_URL_TTL_S`. Long by default: the relay re-signs at every wake, and the only thing
/// the lifetime bounds is how far back a signature a box has already seen could be rolled.
const DEFAULT_TTL_S: i64 = 30 * 24 * 60 * 60;

fn parse_signing_key(raw: &str) -> Result<SigningKey, String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(raw.trim())
        .map_err(|e| format!("not base64: {e}"))?;
    let seed: [u8; 32] = bytes
        .try_into()
        .map_err(|_| "an Ed25519 seed is 32 bytes".to_owned())?;
    Ok(SigningKey::from_bytes(&seed))
}

/// Reject a key that is set but unusable at startup, rather than letting a typo read as "signing
/// is off". Absent is a configuration; malformed is a mistake, and it must not be silent.
pub fn check_env_key() -> anyhow::Result<()> {
    match std::env::var(SIGNING_KEY_ENV) {
        Ok(raw) if !raw.trim().is_empty() => parse_signing_key(&raw)
            .map(|_| ())
            .map_err(|e| anyhow::anyhow!("{SIGNING_KEY_ENV} is set but unusable ({e})")),
        _ => Ok(()),
    }
}

fn signing_key() -> Option<&'static SigningKey> {
    static KEY: OnceLock<Option<SigningKey>> = OnceLock::new();
    KEY.get_or_init(|| {
        let raw = std::env::var(SIGNING_KEY_ENV).ok()?;
        if raw.trim().is_empty() {
            return None;
        }
        match parse_signing_key(&raw) {
            Ok(k) => Some(k),
            Err(e) => {
                tracing::error!(error = %e, "{SIGNING_KEY_ENV} is unusable; boxes get no verify key");
                None
            }
        }
    })
    .as_ref()
}

/// The public key to put in a box's environment, when this relay can sign at all.
pub fn verify_key_b64() -> Option<String> {
    signing_key()
        .map(|k| base64::engine::general_purpose::STANDARD.encode(k.verifying_key().to_bytes()))
}

fn ttl_s() -> i64 {
    std::env::var("ARBOS_RELAY_URL_TTL_S")
        .ok()
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(DEFAULT_TTL_S)
}

/// Sign `url` for one box. `None` when this relay has no signing key, which is the case a box
/// without a verify key expects.
pub fn sign(url: &str, box_id: &str, now_ms: i64) -> Option<SignedRelayUrl> {
    let key = signing_key()?;
    let url = url.trim().trim_end_matches('/').to_owned();
    let expires_at_ms = now_ms.saturating_add(ttl_s().saturating_mul(1_000));
    let msg = relay_url::signing_message(&url, box_id, now_ms, expires_at_ms);
    let sig = base64::engine::general_purpose::STANDARD.encode(key.sign(msg.as_bytes()).to_bytes());
    Some(SignedRelayUrl {
        url,
        box_id: box_id.to_owned(),
        // The signing time doubles as the counter: it rises on every push, so of two valid
        // signatures a box holds, the higher one is the newer address.
        counter: now_ms,
        expires_at_ms,
        sig,
    })
}

/// The signature file that sits beside a `relay-url` file.
pub fn sig_path(url_file: &str) -> String {
    format!("{url_file}{}", relay_url::SIG_FILE_SUFFIX)
}

/// The shell for the Daytona push: writes the address and, when this relay can sign, its
/// signature, both owned by root and not writable by any other user.
///
/// The modes are the point. A workspace runs as its own Unix user (D70), and a world-writable
/// file — or any file in a world-writable, non-sticky directory, since directory write permission
/// is enough to unlink and replace what it holds — lets one workspace's user redirect the single
/// root agentd that serves every workspace on the box. Both writers here are already root (this
/// command runs under `sudo`, and agentd is root), so nothing needs write access but root.
pub fn push_command(url_file: &str, legacy_dir: &str, signed: Option<&SignedRelayUrl>) -> String {
    let dir = std::path::Path::new(url_file)
        .parent()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "/home/daytona/.arbos".to_owned());
    let url = signed.map(|s| s.url.clone()).unwrap_or_default();
    let sig_file = sig_path(url_file);
    // The signature is JSON and the command is single-quoted, so it travels base64-encoded rather
    // than fighting two levels of shell quoting.
    let sig_b64 = signed
        .and_then(|s| serde_json::to_string(s).ok())
        .map(|j| base64::engine::general_purpose::STANDARD.encode(j));
    let write_sig = match &sig_b64 {
        Some(b64) => format!(
            " && printf \"%s\" \"{b64}\" | base64 -d > {sig_file} && chmod 0644 {sig_file} && chown root:root {sig_file}"
        ),
        // No signature to write: remove a stale one rather than leave a box verifying an old
        // address against a key it no longer has.
        None => format!(" && rm -f {sig_file}"),
    };
    format!(
        "sudo -n sh -c 'mkdir -p {dir} && chown root:root {dir} && chmod 0755 {dir} \
         && printf \"%s\\n\" \"{url}\" > {url_file} && chown root:root {url_file} && chmod 0644 {url_file}{write_sig}; \
         if [ -d {legacy_dir} ]; then printf \"%s\\n\" \"{url}\" > {legacy_dir}/relay-url 2>/dev/null \
         && chown root:root {legacy_dir}/relay-url 2>/dev/null && chmod 0644 {legacy_dir}/relay-url 2>/dev/null; \
         chown root:root {legacy_dir} 2>/dev/null; chmod 0755 {legacy_dir} 2>/dev/null; fi; echo ok'"
    )
}

/// The local-box equivalent: agentd runs as this same user, so ownership is already right and
/// only the modes need stating.
pub fn write_local(state_dir: &std::path::Path, url: &str, signed: Option<&SignedRelayUrl>) {
    use std::os::unix::fs::PermissionsExt;
    if std::fs::create_dir_all(state_dir).is_err() {
        return;
    }
    let _ = std::fs::set_permissions(state_dir, std::fs::Permissions::from_mode(0o755));
    let file = state_dir.join("relay-url");
    if std::fs::write(&file, format!("{url}\n")).is_ok() {
        let _ = std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644));
    }
    let sig = state_dir.join(format!("relay-url{}", relay_url::SIG_FILE_SUFFIX));
    match signed.and_then(|s| serde_json::to_string(s).ok()) {
        Some(json) => {
            if std::fs::write(&sig, json).is_ok() {
                let _ = std::fs::set_permissions(&sig, std::fs::Permissions::from_mode(0o644));
            }
        }
        None => {
            let _ = std::fs::remove_file(&sig);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};

    /// The env-backed key is read once per process, so these tests drive the pure parts directly
    /// rather than racing each other over one global.
    fn key() -> SigningKey {
        SigningKey::from_bytes(&[7u8; 32])
    }

    fn sign_with(
        k: &SigningKey,
        url: &str,
        box_id: &str,
        counter: i64,
        exp: i64,
    ) -> SignedRelayUrl {
        let msg = relay_url::signing_message(url, box_id, counter, exp);
        SignedRelayUrl {
            url: url.into(),
            box_id: box_id.to_owned(),
            counter,
            expires_at_ms: exp,
            sig: base64::engine::general_purpose::STANDARD
                .encode(k.sign(msg.as_bytes()).to_bytes()),
        }
    }

    #[test]
    fn a_signature_this_relay_made_verifies_under_its_public_key() {
        let k = key();
        let s = sign_with(&k, "https://relay.example", "user:alice", 5, 9);
        let vk = VerifyingKey::from_bytes(&k.verifying_key().to_bytes()).unwrap();
        let raw = base64::engine::general_purpose::STANDARD
            .decode(&s.sig)
            .unwrap();
        let sig = Signature::from_slice(&raw).unwrap();
        assert!(vk.verify(s.message().as_bytes(), &sig).is_ok());
    }

    #[test]
    fn moving_the_signature_onto_another_address_breaks_it() {
        let k = key();
        let mut s = sign_with(&k, "https://relay.example", "user:alice", 5, 9);
        s.url = "https://evil.example".into();
        let vk = VerifyingKey::from_bytes(&k.verifying_key().to_bytes()).unwrap();
        let raw = base64::engine::general_purpose::STANDARD
            .decode(&s.sig)
            .unwrap();
        let sig = Signature::from_slice(&raw).unwrap();
        assert!(
            vk.verify(s.message().as_bytes(), &sig).is_err(),
            "an attacker who controls the address cannot carry a genuine signature onto it"
        );
    }

    #[test]
    fn a_malformed_key_is_an_error_and_not_a_quiet_no() {
        assert!(parse_signing_key("not base64!").is_err());
        assert!(
            parse_signing_key(&base64::engine::general_purpose::STANDARD.encode([1u8; 16]))
                .is_err(),
            "a short seed is a mistake, not a shorter key"
        );
        assert!(
            parse_signing_key(&base64::engine::general_purpose::STANDARD.encode([1u8; 32])).is_ok()
        );
    }

    #[test]
    fn the_push_command_leaves_nothing_writable_by_another_user() {
        let signed = sign_with(&key(), "https://relay.example", "user:alice", 5, 9);
        let cmd = push_command(
            "/home/daytona/.arbos/relay-url",
            "/work/.arbos",
            Some(&signed),
        );
        assert!(cmd.contains("chmod 0755 /home/daytona/.arbos"));
        assert!(cmd.contains("chmod 0644 /home/daytona/.arbos/relay-url"));
        assert!(cmd.contains("chmod 0644 /home/daytona/.arbos/relay-url.sig"));
        assert!(cmd.contains("chmod 0644 /work/.arbos/relay-url"));
        assert!(cmd.contains("chmod 0755 /work/.arbos"));
        assert!(cmd.contains("chown root:root /home/daytona/.arbos/relay-url"));
        for forbidden in ["0666", "0777", "1777", "chown daytona:daytona"] {
            assert!(
                !cmd.contains(forbidden),
                "{forbidden} would hand the address back to any workspace user: {cmd}"
            );
        }
    }

    #[test]
    fn the_signature_travels_base64_so_its_json_cannot_break_the_shell() {
        let signed = sign_with(&key(), "https://relay.example", "user:alice", 5, 9);
        let cmd = push_command(
            "/home/daytona/.arbos/relay-url",
            "/work/.arbos",
            Some(&signed),
        );
        assert!(cmd.contains("| base64 -d > /home/daytona/.arbos/relay-url.sig"));
        assert!(
            !cmd.contains("\\\"url\\\""),
            "raw JSON in a single-quoted shell string is how quoting bugs get in: {cmd}"
        );
    }

    #[test]
    fn an_unsigned_push_clears_any_signature_already_on_the_box() {
        let cmd = push_command("/home/daytona/.arbos/relay-url", "/work/.arbos", None);
        assert!(
            cmd.contains("rm -f /home/daytona/.arbos/relay-url.sig"),
            "a leftover signature would be checked against a key the box no longer has: {cmd}"
        );
    }

    #[test]
    fn a_local_box_write_is_not_world_writable() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("arbos-relay-url-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let signed = sign_with(&key(), "https://relay.example", "user:alice", 5, 9);
        write_local(&dir, "https://relay.example", Some(&signed));
        let url_mode = std::fs::metadata(dir.join("relay-url"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        let sig_mode = std::fs::metadata(dir.join("relay-url.sig"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        let dir_mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            url_mode, 0o644,
            "the address must not be writable by another user"
        );
        assert_eq!(sig_mode, 0o644);
        assert_eq!(
            dir_mode, 0o755,
            "a writable directory is enough to replace the file inside it"
        );
        // Dropping the key clears the signature rather than leaving a stale one.
        write_local(&dir, "https://relay.example", None);
        assert!(!dir.join("relay-url.sig").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
