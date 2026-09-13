//! `~/.config/arbos/config.toml`: the shared type lives in `arbos_core::host`
//! so the desktop reads the same file the same way. This module adds what
//! only the engine needs from it.

pub use arbos_core::host::{Host, HostConfig, KeySource, ProviderKind};

impl crate::retry::RetryPolicy {
    pub fn from_config(cfg: &HostConfig) -> Self {
        Self {
            max_attempts: cfg.max_attempts.max(1),
            base: std::time::Duration::from_millis(cfg.backoff_base_ms),
            max_backoff: std::time::Duration::from_millis(cfg.backoff_max_ms),
            max_server_delay: std::time::Duration::from_millis(cfg.max_server_delay_ms),
        }
    }
}
