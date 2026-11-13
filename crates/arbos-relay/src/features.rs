//! Remote feature flags the apps read from `GET /me` → `features` (one object, one place).
//! Each flag is an env switch on the relay, flipped with a redeploy and no client release.
//!
//! | flag | env | default |
//! | --- | --- | --- |
//! | `vault_connect` (#1265: the Connect 1Password flow) | `ARBOS_FEATURE_VAULT_CONNECT` (`vaults.rs`) | off |
//! | `leaf_avatars`, `leaf_lottie` (retired: the leaf artwork is gone) | — | always off, see [`leaf_avatars`] |
//! | `voice_mode` (spoken replies, voice picker, reply playback; 2026-10-22 Jacob: hidden until it works end to end) | `ARBOS_FEATURE_VOICE_MODE` | off; the relay also refuses `/proxy/v1/audio/speech` while off, so no client can spend on TTS |
//!
//! `GET /secrets` serves the same object from [`dto`].

use arbos_core::api::MeFeaturesDto;

/// `1|true|on|yes` → on, `0|false|off|no` → off, anything else → `default`.
pub fn env_flag(name: &str, default: bool) -> bool {
    match std::env::var(name) {
        Ok(v) => match v.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "on" | "yes" => true,
            "0" | "false" | "off" | "no" => false,
            _ => default,
        },
        Err(_) => default,
    }
}

/// Retired 2026-11-05: agents without a photo draw a plain solid-colour disc, and the leaf
/// artwork is no longer in the repository. Current builds ignore both keys; the relay keeps
/// sending them as `false` so a build that still carries the art stops drawing it and falls
/// back to its D79 initials. Not an env switch any more — there is nothing to switch back on.
pub fn leaf_avatars() -> bool {
    false
}

/// Voice mode (spoken replies, voice picker, playback). Default off; `ARBOS_FEATURE_VOICE_MODE=1`
/// brings it back without a client release.
pub fn voice_mode() -> bool {
    env_flag("ARBOS_FEATURE_VOICE_MODE", false)
}

/// The `features` object on `GET /me` and `GET /secrets`. Key names are a contract with the
/// shipped apps (Mac v0.6.104 / TestFlight read `vault_connect`); only add, never rename.
pub fn dto() -> MeFeaturesDto {
    MeFeaturesDto {
        vault_connect: Some(crate::vaults::vault_connect()),
        leaf_avatars: Some(leaf_avatars()),
        leaf_lottie: Some(leaf_avatars()),
        voice_mode: Some(voice_mode()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flag_parsing() {
        std::env::remove_var("ARBOS_FEATURE_TEST_FLAG");
        assert!(env_flag("ARBOS_FEATURE_TEST_FLAG", true));
        assert!(!env_flag("ARBOS_FEATURE_TEST_FLAG", false));
        for (v, want) in [
            ("0", false),
            ("off", false),
            ("NO", false),
            ("1", true),
            ("on", true),
            ("maybe", true),
        ] {
            std::env::set_var("ARBOS_FEATURE_TEST_FLAG", v);
            assert_eq!(env_flag("ARBOS_FEATURE_TEST_FLAG", true), want, "{v}");
        }
        std::env::remove_var("ARBOS_FEATURE_TEST_FLAG");
    }

    /// Voice mode is off unless the relay is started with the flag; the object always carries
    /// the key, so a client never has to guess. (The flag itself is not flipped here: other
    /// tests snapshot the feature DTO concurrently.)
    #[test]
    fn voice_mode_is_off_by_default_and_always_sent() {
        let features = serde_json::to_value(dto()).expect("feature DTO JSON");
        if std::env::var_os("ARBOS_FEATURE_VOICE_MODE").is_none() {
            assert!(!voice_mode());
            assert_eq!(features["voice_mode"], serde_json::json!(false));
        }
        assert!(features.get("voice_mode").is_some());
    }
}
