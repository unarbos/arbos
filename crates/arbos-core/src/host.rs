//! `~/.config/arbos/config.toml` and the places list.
//!
//! Lives in core so the kernel, the engine, and the desktop all read one
//! config the same way: which provider, which base URL, where the key is.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Deserializer, Serialize};
use std::path::{Path, PathBuf};

/// Where model calls go. Every kind speaks the OpenAI chat-completions
/// wire; they differ in base URL, key variable, and extra headers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    /// openrouter.ai: one key, every model. The default.
    #[default]
    OpenRouter,
    /// api.openai.com with an OpenAI key.
    OpenAi,
    /// Any other OpenAI-compatible endpoint (vLLM, Ollama, a gateway).
    /// `api_base` is required.
    Custom,
}

impl ProviderKind {
    pub const ALL: [Self; 3] = [Self::OpenRouter, Self::OpenAi, Self::Custom];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenRouter => "openrouter",
            Self::OpenAi => "openai",
            Self::Custom => "custom",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "openrouter" => Some(Self::OpenRouter),
            "openai" => Some(Self::OpenAi),
            "custom" => Some(Self::Custom),
            _ => None,
        }
    }

    /// Human label for pickers.
    pub fn label(self) -> &'static str {
        match self {
            Self::OpenRouter => "OpenRouter",
            Self::OpenAi => "OpenAI",
            Self::Custom => "Custom endpoint",
        }
    }

    /// The base URL this kind uses when `api_base` is empty. Custom has
    /// none: the user must say.
    pub fn default_base(self) -> Option<&'static str> {
        match self {
            Self::OpenRouter => Some("https://openrouter.ai/api/v1"),
            Self::OpenAi => Some("https://api.openai.com/v1"),
            Self::Custom => None,
        }
    }

    /// The environment variable holding the key when `api_key_env` is empty.
    pub fn default_key_env(self) -> &'static str {
        match self {
            Self::OpenRouter => "OPENROUTER_API_KEY",
            Self::OpenAi => "OPENAI_API_KEY",
            Self::Custom => "ARBOS_API_KEY",
        }
    }

    /// The model used when `model` is empty.
    pub fn default_model(self) -> &'static str {
        match self {
            Self::OpenRouter => "anthropic/claude-opus-5",
            Self::OpenAi => "gpt-5.6-terra",
            Self::Custom => "",
        }
    }

    /// Models offered first in a picker, in this order. Any other id the
    /// provider lists is one keystroke away; these are the good defaults.
    pub fn suggested_models(self) -> &'static [&'static str] {
        match self {
            Self::OpenRouter => &[
                "anthropic/claude-opus-5",
                "anthropic/claude-fable-5.1",
                "openai/gpt-5.6-terra",
                "google/gemini-3.8-flash",
                "x-ai/grok-4.6",
                "deepseek/deepseek-v4.1-flash",
                "z-ai/glm-5.3",
                "moonshotai/kimi-k3",
            ],
            Self::OpenAi => &["gpt-5.6-terra", "gpt-5.6-sol", "gpt-5.6-luna", "gpt-4.1"],
            Self::Custom => &[],
        }
    }

    /// Where to get a key, for setup text.
    pub fn keys_url(self) -> Option<&'static str> {
        match self {
            Self::OpenRouter => Some("https://openrouter.ai/settings/keys"),
            Self::OpenAi => Some("https://platform.openai.com/api-keys"),
            Self::Custom => None,
        }
    }

    /// The kind a bare base URL implies. Old config files name no
    /// provider; the URL they point at says which one they meant.
    pub fn infer(api_base: &str) -> Self {
        let base = api_base.trim().to_ascii_lowercase();
        if base.is_empty() || base.contains("openrouter.ai") {
            Self::OpenRouter
        } else if base.contains("api.openai.com") {
            Self::OpenAi
        } else {
            Self::Custom
        }
    }
}

/// Every field has a default, so a partial file is fine and an unknown key
/// is reported rather than ignored.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HostConfig {
    /// openrouter (default), openai, or custom. Absent in old files: then
    /// inferred from `api_base`.
    pub provider: Option<ProviderKind>,
    /// Empty = the provider's default model.
    pub model: String,
    /// Empty = the provider's default base. Required for custom.
    pub api_base: String,
    pub api_key: Option<String>,
    /// Empty = the provider's default variable.
    pub api_key_env: Option<String>,
    /// Context the loop plans against. 0 = the model's own context length
    /// from the provider's model list (capped by `window_tokens_max`).
    pub window_tokens: u64,
    /// Cap for the auto window. Big prompts are slow and dear even when the
    /// model accepts them.
    pub window_tokens_max: u64,
    pub search_url: Option<String>,
    pub search_key: Option<String>,
    /// Model for OpenRouter's web-plugin searches (a small, fast one; the
    /// answer is discarded, only its sources are kept). Empty = default.
    pub search_model: String,
    /// Self-hosted speech server for the desktop's voice client
    /// (`ws://` / `wss://`); the kernel only carries the setting.
    pub voice_url: Option<String>,
    pub voice_token: Option<String>,
    pub voice_token_env: Option<String>,
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
    /// Levels of agents below the root: 1 = root → child and no further
    /// (Cursor's shape), 2 = children may spawn grandchildren, 3 = one more
    /// (a coordinator that delegates delegation). Default 3.
    pub max_depth: usize,
    /// Live children one agent may have at once. Default 8.
    pub max_children: usize,
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
    /// Vision-capable model that describes an attached image in words when
    /// the turn's model cannot see it. Empty = the first vision-capable
    /// fallback, else a cheap OpenRouter vision model.
    pub vision_model: String,
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
            // None here, not Some: `#[serde(default)]` fills a missing key
            // from this value, and an old file that names only `api_base`
            // must be read by that URL, not overruled by the default.
            provider: None,
            model: ProviderKind::OpenRouter.default_model().into(),
            api_base: ProviderKind::OpenRouter
                .default_base()
                .unwrap_or_default()
                .into(),
            api_key: None,
            api_key_env: None,
            window_tokens: 0,
            window_tokens_max: 400_000,
            search_url: None,
            search_key: None,
            search_model: String::new(),
            voice_url: None,
            voice_token: None,
            voice_token_env: None,
            reasoning_effort: None,
            max_output_tokens: 32_000,
            max_parallel_tools: 8,
            speculate: true,
            // A test suite that takes four minutes should come back as one
            // result, not as a job the model polls in two-minute slices —
            // each slice is a model call. Servers still use background:true.
            bash_wait_ms: 600_000,
            max_depth: 3,
            max_children: 8,
            fallback_models: Vec::new(),
            max_attempts: 5,
            backoff_base_ms: 1_000,
            backoff_max_ms: 30_000,
            max_server_delay_ms: 60_000,
            stream_idle_ms: 120_000,
            compact_model: String::new(),
            vision_model: String::new(),
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
    /// The provider in force: named, or inferred from the base URL.
    pub fn provider(&self) -> ProviderKind {
        self.provider
            .unwrap_or_else(|| ProviderKind::infer(&self.api_base))
    }

    /// The base URL requests go to, with the trailing slash removed.
    pub fn api_base(&self) -> Result<String> {
        let base = self.api_base.trim();
        if !base.is_empty() {
            return Ok(base.trim_end_matches('/').to_string());
        }
        match self.provider().default_base() {
            Some(b) => Ok(b.to_string()),
            None => bail!("provider = \"custom\" needs api_base in config.toml"),
        }
    }

    /// The model a turn uses when the agent says `inherit`.
    pub fn model(&self) -> String {
        let m = self.model.trim();
        if m.is_empty() {
            self.provider().default_model().to_string()
        } else {
            m.to_string()
        }
    }

    /// The environment variable consulted for the key.
    pub fn key_env(&self) -> String {
        match self.api_key_env.as_deref().map(str::trim) {
            Some(env) if !env.is_empty() => env.to_string(),
            _ => self.provider().default_key_env().to_string(),
        }
    }

    /// This config with `key` written in as the key (and no env lookup):
    /// what goes onto a remote machine, or into a kernel from a window.
    /// One shape for both, so the keys match wherever it lands.
    pub fn with_key(&self, key: &str) -> Self {
        let mut cfg = self.clone();
        cfg.api_key = Some(key.trim().to_string());
        cfg.api_key_env = None;
        cfg
    }

    /// Reset the provider-shaped fields for `kind` and leave the rest.
    pub fn set_provider(&mut self, kind: ProviderKind) {
        self.provider = Some(kind);
        self.api_base = kind.default_base().unwrap_or_default().to_string();
        self.api_key_env = None;
        self.model = kind.default_model().to_string();
    }
}

/// Where the key in use came from. Setup screens show this; nothing prints
/// the key itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeySource {
    /// `api_key` in config.toml.
    Config,
    /// The named environment variable.
    Env(String),
    /// Nowhere: the named variable is what setup would read.
    Missing(String),
}

#[derive(Debug, Clone)]
pub struct Host {
    pub config: HostConfig,
    pub dir: PathBuf,
}

/// A config handed to this process at run time and not written down
/// (`configure` with `remember = false`). `Host::load`/`peek` return it
/// instead of the file while it stands; it dies with the process.
static OVERRIDE: std::sync::RwLock<Option<HostConfig>> = std::sync::RwLock::new(None);

/// Use `config` for the rest of this process without touching the file.
pub fn set_override(config: Option<HostConfig>) {
    *OVERRIDE.write().unwrap_or_else(|p| p.into_inner()) = config;
}

/// Whether a run-time config stands in for the file.
pub fn is_overridden() -> bool {
    override_config().is_some()
}

fn override_config() -> Option<HostConfig> {
    OVERRIDE.read().unwrap_or_else(|p| p.into_inner()).clone()
}

impl Host {
    /// Read the config, or write the defaults on first run. A malformed
    /// file is an error the user sees, not a silent fall back to defaults.
    pub fn load() -> Result<Self> {
        let dir = dirs_config();
        if let Some(config) = override_config() {
            return Ok(Self { config, dir });
        }
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("config.toml");
        let config = if path.exists() {
            Self::read(&path)?
        } else {
            let cfg = HostConfig {
                provider: Some(ProviderKind::OpenRouter),
                ..HostConfig::default()
            };
            write_private(&path, &toml::to_string_pretty(&cfg)?)?;
            cfg
        };
        let places = dir.join("places");
        if !places.exists() {
            std::fs::write(places, "")?;
        }
        Ok(Self { config, dir })
    }

    /// The config as it stands, without writing anything. Absent file =
    /// defaults. For screens that only look.
    pub fn peek() -> Result<Self> {
        let dir = dirs_config();
        if let Some(config) = override_config() {
            return Ok(Self { config, dir });
        }
        let path = dir.join("config.toml");
        let config = if path.exists() {
            Self::read(&path)?
        } else {
            HostConfig::default()
        };
        Ok(Self { config, dir })
    }

    fn read(path: &Path) -> Result<HostConfig> {
        let text = std::fs::read_to_string(path)?;
        toml::from_str(&text).with_context(|| format!("parse {}", path.display()))
    }

    pub fn config_path(&self) -> PathBuf {
        self.dir.join("config.toml")
    }

    /// Write the config back. The file may hold a key, so it is
    /// owner-readable only and never left half-written.
    pub fn save(&self) -> Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        write_private(&self.config_path(), &toml::to_string_pretty(&self.config)?)
    }

    pub fn api_key(&self) -> Option<String> {
        if let Some(k) = &self.config.api_key {
            if !k.trim().is_empty() {
                return Some(k.trim().to_string());
            }
        }
        std::env::var(self.config.key_env())
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    pub fn key_source(&self) -> KeySource {
        if self
            .config
            .api_key
            .as_deref()
            .is_some_and(|k| !k.trim().is_empty())
        {
            return KeySource::Config;
        }
        let env = self.config.key_env();
        match std::env::var(&env) {
            Ok(v) if !v.trim().is_empty() => KeySource::Env(env),
            _ => KeySource::Missing(env),
        }
    }

    /// One line telling the user how to get unblocked when there is no key.
    pub fn missing_key_hint(&self) -> String {
        let env = self.config.key_env();
        let kind = self.config.provider();
        let where_ = match kind.keys_url() {
            Some(url) => format!(" Keys: {url}"),
            None => String::new(),
        };
        format!(
            "No API key for {}. Run `arbos-kernel setup`, or set {env}, or put api_key in {}.{where_}",
            kind.label(),
            self.config_path().display()
        )
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

/// Write via a sibling temp file and rename, mode 0600 on Unix.
fn write_private(path: &Path, text: &str) -> Result<()> {
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    std::fs::write(&tmp, text).with_context(|| format!("write {}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&tmp, path).with_context(|| format!("replace {}", path.display()))?;
    Ok(())
}

pub fn dirs_config() -> PathBuf {
    if let Some(base) = std::env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(base).join("arbos");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".config").join("arbos");
    }
    PathBuf::from(".arbos-host")
}

/// Request headers a provider wants beyond `Authorization`. OpenRouter
/// asks apps to name themselves so usage shows up under the app.
pub fn attribution_headers(kind: ProviderKind) -> &'static [(&'static str, &'static str)] {
    match kind {
        ProviderKind::OpenRouter => &[
            ("HTTP-Referer", "https://github.com/unarbos/arbos"),
            ("X-Title", "Arbos"),
        ],
        ProviderKind::OpenAi | ProviderKind::Custom => &[],
    }
}
