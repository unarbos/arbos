//! `~/.config/arbos/config.toml` and the places list.

use anyhow::{Context, Result};
use serde::{Deserialize, Deserializer, Serialize};
use std::path::PathBuf;

/// Every field has a default, so a partial file is fine and an unknown key
/// is reported rather than ignored.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HostConfig {
    pub model: String,
    pub api_base: String,
    pub api_key: Option<String>,
    pub api_key_env: Option<String>,
    /// Context the loop plans against. 0 = the model's own context length
    /// from the provider's model list (capped by `window_tokens_max`).
    pub window_tokens: u64,
    /// Cap for the auto window. Big prompts are slow and dear even when the
    /// model accepts them.
    pub window_tokens_max: u64,
    pub search_url: Option<String>,
    pub search_key: Option<String>,
    /// Inception Mercury: instant | low | medium | high. Empty = omit.
    pub reasoning_effort: Option<String>,
    /// `max_tokens` on every call: the model's own completion limit from the
    /// provider's model list, but never more than this. 0 = do not send.
    /// A runaway model otherwise streams until the provider stops it.
    pub max_output_tokens: u64,
    /// Tool calls from one model step that may run at once. 1 = sequential.
    pub max_parallel_tools: usize,
    /// Start read-only tool calls while the model is still streaming.
    pub speculate: bool,
    /// How long `bash` stays attached before the command continues as a job.
    pub bash_wait_ms: u64,
    /// Models to try, in order, when the primary keeps failing or its
    /// provider errors out. A switch lasts one turn; the next turn tries
    /// the primary again. Accepts an array or a comma-separated string.
    /// Empty on OpenRouter = a built-in list; `["none"]` = no fallback.
    #[serde(deserialize_with = "list_or_csv")]
    pub fallback_models: Vec<String>,
    /// Provider calls per model before moving to the next, including the first.
    pub max_attempts: u32,
    pub backoff_base_ms: u64,
    pub backoff_max_ms: u64,
    /// Longest server `Retry-After` honoured. Beyond it: fall back or fail fast.
    pub max_server_delay_ms: u64,
    /// Silence mid-stream that counts as a lost connection.
    pub stream_idle_ms: u64,
    /// Model that writes compaction summaries. Empty = the turn's model.
    pub compact_model: String,
    /// The summariser model's window when smaller than `window_tokens`.
    /// 0 = same as the turn's window.
    pub compact_window_tokens: u64,
    /// Fraction of the window at which old turns are summarised.
    pub compact_at: f64,
    /// Fraction of the window at which old tool bodies fold to a cite.
    pub fold_at: f64,
    /// Tokens of recent turns kept verbatim through a compaction.
    pub keep_recent_tokens: u64,
    /// Headroom the summariser may use for its answer.
    pub reserve_tokens: u64,
    /// Newest tool results a fold never touches.
    pub protect_tool_results: usize,
    /// Write every provider call — request body, response headers, each raw
    /// chunk with its arrival time, and the parsed result — under the
    /// agent's `trace/` folder. Off by default; large.
    pub trace: bool,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            model: "gpt-4.1".into(),
            api_base: "https://api.openai.com/v1".into(),
            api_key: None,
            api_key_env: Some("OPENAI_API_KEY".into()),
            window_tokens: 0,
            window_tokens_max: 400_000,
            search_url: None,
            search_key: None,
            reasoning_effort: None,
            max_output_tokens: 32_000,
            max_parallel_tools: 8,
            speculate: true,
            // A test suite that takes four minutes should come back as one
            // result, not as a job the model polls in two-minute slices —
            // each slice is a model call. Servers still use background:true.
            bash_wait_ms: 600_000,
            fallback_models: Vec::new(),
            max_attempts: 5,
            backoff_base_ms: 1_000,
            backoff_max_ms: 30_000,
            max_server_delay_ms: 60_000,
            stream_idle_ms: 120_000,
            compact_model: String::new(),
            compact_window_tokens: 0,
            // Folding at half the window made models re-read what had
            // just been hidden; with the prefix cached, a fuller window is
            // cheaper than the extra calls.
            compact_at: 0.85,
            fold_at: 0.65,
            keep_recent_tokens: 20_000,
            reserve_tokens: 16_384,
            protect_tool_results: 8,
            trace: false,
        }
    }
}

/// `["a", "b"]` or `"a, b"`.
fn list_or_csv<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        List(Vec<String>),
        Csv(String),
    }
    Ok(match Raw::deserialize(d)? {
        Raw::List(v) => v,
        Raw::Csv(s) => s
            .split(',')
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect(),
    })
}

impl HostConfig {
    pub fn retry_policy(&self) -> crate::retry::RetryPolicy {
        crate::retry::RetryPolicy {
            max_attempts: self.max_attempts.max(1),
            base: std::time::Duration::from_millis(self.backoff_base_ms),
            max_backoff: std::time::Duration::from_millis(self.backoff_max_ms),
            max_server_delay: std::time::Duration::from_millis(self.max_server_delay_ms),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Host {
    pub config: HostConfig,
    pub dir: PathBuf,
}

impl Host {
    /// Read the config, or write the defaults on first run. A malformed
    /// file is an error the user sees, not a silent fall back to defaults.
    pub fn load() -> Result<Self> {
        let dir = dirs_config();
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("config.toml");
        let config = if path.exists() {
            let text = std::fs::read_to_string(&path)?;
            toml::from_str(&text).with_context(|| format!("parse {}", path.display()))?
        } else {
            let cfg = HostConfig::default();
            std::fs::write(&path, toml::to_string_pretty(&cfg)?)?;
            cfg
        };
        let places = dir.join("places");
        if !places.exists() {
            std::fs::write(places, "")?;
        }
        Ok(Self { config, dir })
    }

    pub fn api_key(&self) -> Option<String> {
        if let Some(k) = &self.config.api_key {
            if !k.is_empty() {
                return Some(k.clone());
            }
        }
        let env = self
            .config
            .api_key_env
            .as_deref()
            .unwrap_or("OPENAI_API_KEY");
        std::env::var(env).ok().filter(|s| !s.is_empty())
    }

    pub fn remember_place(&self, path: &std::path::Path) {
        let file = self.dir.join("places");
        let line = format!("{}\n", path.display());
        let body = std::fs::read_to_string(&file).unwrap_or_default();
        if body.lines().any(|l| l == path.display().to_string()) {
            return;
        }
        let mut out = body;
        out.push_str(&line);
        let _ = std::fs::write(file, out);
    }

    pub fn places(&self) -> Vec<PathBuf> {
        std::fs::read_to_string(self.dir.join("places"))
            .unwrap_or_default()
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(PathBuf::from)
            .collect()
    }
}

fn dirs_config() -> PathBuf {
    if let Some(base) = std::env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(base).join("arbos");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".config").join("arbos");
    }
    PathBuf::from(".arbos-host")
}
