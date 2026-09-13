//! `arbos-kernel setup`: provider, key, model, one test call, saved.
//!
//! Interactive when stdin is a terminal; every step also takes a flag so a
//! script or the desktop can drive it. The key is read with echo off and
//! is never printed; the summary names where it came from, not what it is.

use anyhow::{Context, Result, bail};
use arbos_core::host::{Host, KeySource, ProviderKind};
use arbos_engine::{ChatMessage, Provider, check_key, list_model_ids};
use std::io::{BufRead, IsTerminal, Write};
use std::time::Duration;

/// How many listed ids to show when none of the picks are on the list.
const FALLBACK_ROWS: usize = 12;
/// Attempts at a key before setup gives up.
const KEY_TRIES: u32 = 3;

#[derive(Debug, Default)]
pub struct Args {
    pub provider: Option<ProviderKind>,
    pub base: Option<String>,
    pub model: Option<String>,
    pub key_env: Option<String>,
    /// Read the key from the first line of stdin instead of the terminal.
    pub key_stdin: bool,
    /// Ask nothing: take flags, env, and defaults; fail where a choice is
    /// missing.
    pub yes: bool,
}

impl Args {
    pub fn parse(mut argv: impl Iterator<Item = String>) -> Result<Self> {
        let mut args = Self::default();
        while let Some(a) = argv.next() {
            match a.as_str() {
                "--provider" => {
                    let v = argv.next().context("--provider needs a value")?;
                    args.provider = Some(ProviderKind::parse(&v).ok_or_else(|| {
                        anyhow::anyhow!("--provider: openrouter, openai, or custom (not {v:?})")
                    })?);
                }
                "--base" => args.base = Some(argv.next().context("--base needs a URL")?),
                "--model" => args.model = Some(argv.next().context("--model needs an id")?),
                "--key-env" => args.key_env = Some(argv.next().context("--key-env needs a name")?),
                "--key-stdin" => args.key_stdin = true,
                "--yes" | "-y" => args.yes = true,
                "-h" | "--help" => {
                    println!("{USAGE}");
                    std::process::exit(0);
                }
                other => bail!("setup: unknown flag {other}\n{USAGE}"),
            }
        }
        Ok(args)
    }
}

pub const USAGE: &str = "arbos-kernel setup [--provider openrouter|openai|custom] [--base URL] [--model ID] [--key-env VAR] [--key-stdin] [--yes]";

/// Reads and writes the terminal, or does neither under `--yes`.
struct Prompt {
    interactive: bool,
}

impl Prompt {
    fn ask(&self, question: &str, default: &str) -> Result<String> {
        if !self.interactive {
            return Ok(default.to_string());
        }
        let mut out = std::io::stdout();
        if default.is_empty() {
            write!(out, "{question}: ")?;
        } else {
            write!(out, "{question} [{default}]: ")?;
        }
        out.flush()?;
        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line)?;
        let line = line.trim();
        Ok(if line.is_empty() {
            default.to_string()
        } else {
            line.to_string()
        })
    }

    fn yes_no(&self, question: &str, default: bool) -> Result<bool> {
        let d = if default { "Y/n" } else { "y/N" };
        let a = self.ask(question, d)?;
        Ok(match a.trim().to_ascii_lowercase().as_str() {
            "y" | "yes" => true,
            "n" | "no" => false,
            _ => default,
        })
    }

    fn secret(&self, question: &str) -> Result<String> {
        if !self.interactive {
            return Ok(String::new());
        }
        let key = rpassword::prompt_password(format!("{question}: "))?;
        Ok(key.trim().to_string())
    }
}

pub fn run(args: Args) -> Result<()> {
    let interactive = !args.yes && std::io::stdin().is_terminal();
    let prompt = Prompt { interactive };
    let mut host = Host::peek()?;
    let path = host.config_path();
    println!("Arbos setup — {}", path.display());

    // 1. Provider.
    let current = host.config.provider();
    let kind = match args.provider {
        Some(k) => k,
        None => pick_provider(&prompt, current)?,
    };
    if kind != current {
        host.config.set_provider(kind);
    }
    // An old file names only a URL; the saved one says which provider.
    host.config.provider = Some(kind);
    if let Some(base) = &args.base {
        host.config.api_base = base.trim().to_string();
    } else if kind == ProviderKind::Custom {
        let base = prompt.ask(
            "Base URL of the OpenAI-compatible endpoint (ends in /v1)",
            &host.config.api_base,
        )?;
        host.config.api_base = base;
    }
    if let Some(env) = &args.key_env {
        host.config.api_key_env = Some(env.trim().to_string());
    }
    let base = host.config.api_base()?;

    // 2. Key, checked against the provider's model list.
    let mut key_stdin = args.key_stdin;
    let mut ids: Vec<String> = Vec::new();
    for attempt in 1..=KEY_TRIES {
        take_key(&prompt, &mut host, kind, key_stdin)?;
        key_stdin = false;
        let Some(key) = host.api_key() else {
            if !interactive {
                bail!("{}", host.missing_key_hint());
            }
            println!("No key given.");
            continue;
        };
        print!("Checking the key against {base} … ");
        std::io::stdout().flush()?;
        match block_on(check_key(&base, &key)) {
            Ok(()) => println!("accepted."),
            Err(e) if kind == ProviderKind::Custom => {
                // Not every endpoint serves /models; a Custom user may know
                // their model id. The test call checks the key instead.
                println!("could not check ({e:#}); the test call will tell.");
            }
            Err(e) => {
                println!("rejected: {e:#}");
                if !interactive || attempt == KEY_TRIES {
                    bail!("the key was not accepted by {base}");
                }
                host.config.api_key = None;
                continue;
            }
        }
        ids = block_on(list_model_ids(&base, &key)).unwrap_or_default();
        break;
    }
    if host.api_key().is_none() {
        bail!("{}", host.missing_key_hint());
    }

    // 3. Model.
    let model = match &args.model {
        Some(m) => m.trim().to_string(),
        None => pick_model(&prompt, kind, &host.config.model(), &ids)?,
    };
    if !ids.is_empty() && !ids.iter().any(|id| id == &model) {
        if !interactive {
            bail!("model {model:?} is not in the provider's list");
        }
        println!("Note: {model} is not in the provider's list; the test call will tell.");
    }
    host.config.model = model.clone();

    // 4. One real call.
    print!("Asking {model} to say OK … ");
    std::io::stdout().flush()?;
    let key = host.api_key().context("key vanished")?;
    let test = Provider {
        base: base.clone(),
        key,
        model: model.clone(),
        reasoning_effort: host.config.reasoning_effort.clone(),
        stream_idle: Duration::from_secs(60),
        // Reasoning models spend tokens thinking before the word.
        max_tokens: Some(512),
        trace: None,
        trace_agent: "setup".into(),
        trace_purpose: "setup".into(),
        trace_line: 0,
    };
    let messages = [ChatMessage::plain(
        "user",
        Some("Reply with the single word OK.".into()),
    )];
    match block_on(test.complete(&messages, &[])) {
        Ok(c) if c.content.trim().is_empty() => println!("it answered (no text)."),
        Ok(c) => println!(
            "it said {:?}.",
            c.content.trim().chars().take(40).collect::<String>()
        ),
        Err(e) => {
            println!("failed: {e:#}");
            if !interactive || !prompt.yes_no("Save this configuration anyway?", false)? {
                bail!("setup did not save: the test call failed");
            }
        }
    }

    // 5. Save.
    host.save()?;
    println!();
    println!("Saved {}", path.display());
    println!("  provider  {}", kind.label());
    println!("  base      {base}");
    println!("  model     {model}");
    println!("  key       {}", describe(&host.key_source()));
    println!();
    println!("Next: open the Arbos app, or run `arbos-kernel serve <folder>`.");
    Ok(())
}

fn pick_provider(prompt: &Prompt, current: ProviderKind) -> Result<ProviderKind> {
    if !prompt.interactive {
        return Ok(current);
    }
    println!("Provider:");
    for (i, k) in ProviderKind::ALL.iter().enumerate() {
        let note = match k {
            ProviderKind::OpenRouter => "one key, every model",
            ProviderKind::OpenAi => "api.openai.com",
            ProviderKind::Custom => "any OpenAI-compatible URL",
        };
        println!("  {}. {:<16} {note}", i + 1, k.label());
    }
    let default = ProviderKind::ALL
        .iter()
        .position(|k| *k == current)
        .map(|i| (i + 1).to_string())
        .unwrap_or_else(|| "1".into());
    loop {
        let a = prompt.ask("Pick 1-3", &default)?;
        if let Some(k) = a
            .parse::<usize>()
            .ok()
            .and_then(|n| ProviderKind::ALL.get(n.wrapping_sub(1)))
            .copied()
            .or_else(|| ProviderKind::parse(&a))
        {
            return Ok(k);
        }
        println!("Type 1, 2, or 3.");
    }
}

/// Put a key on `host`: from stdin, from the terminal, or leave the one the
/// environment already has.
fn take_key(prompt: &Prompt, host: &mut Host, kind: ProviderKind, from_stdin: bool) -> Result<()> {
    if from_stdin {
        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line)?;
        let key = line.trim().to_string();
        if key.is_empty() {
            bail!("--key-stdin: no key on stdin");
        }
        host.config.api_key = Some(key);
        return Ok(());
    }
    let source = host.key_source();
    let label = kind.label();
    let where_ = kind
        .keys_url()
        .map(|u| format!(" (get one at {u})"))
        .unwrap_or_default();
    let question = match &source {
        KeySource::Env(env) => {
            format!(
                "Found {env} in the environment. Paste a {label} key to replace it, or press Enter to keep it"
            )
        }
        KeySource::Config => {
            format!(
                "A key is saved in config.toml. Paste a {label} key to replace it, or press Enter to keep it"
            )
        }
        KeySource::Missing(_) => format!("Paste your {label} API key{where_}"),
    };
    let typed = prompt.secret(&question)?;
    if !typed.is_empty() {
        host.config.api_key = Some(typed);
    }
    Ok(())
}

fn pick_model(
    prompt: &Prompt,
    kind: ProviderKind,
    current: &str,
    ids: &[String],
) -> Result<String> {
    if !prompt.interactive {
        return Ok(current.to_string());
    }
    let mut rows: Vec<String> = kind
        .suggested_models()
        .iter()
        .filter(|p| ids.is_empty() || ids.iter().any(|id| id == *p))
        .map(|p| p.to_string())
        .collect();
    if rows.is_empty() {
        rows = ids.iter().take(FALLBACK_ROWS).cloned().collect();
    }
    if !current.is_empty() && !rows.iter().any(|r| r == current) {
        rows.insert(0, current.to_string());
    }
    if rows.is_empty() {
        return prompt.ask("Model id", current);
    }
    println!("Model:");
    for (i, id) in rows.iter().enumerate() {
        println!("  {:>2}. {id}", i + 1);
    }
    let default = rows
        .iter()
        .position(|r| r == current)
        .map(|i| (i + 1).to_string())
        .unwrap_or_else(|| "1".into());
    let a = prompt.ask("Pick a number, or type any model id", &default)?;
    Ok(match a.parse::<usize>() {
        Ok(n) if n >= 1 && n <= rows.len() => rows[n - 1].clone(),
        _ => a,
    })
}

fn describe(source: &KeySource) -> String {
    match source {
        KeySource::Config => "saved in config.toml (mode 600)".into(),
        KeySource::Env(env) => format!("read from ${env}"),
        KeySource::Missing(env) => format!("missing (set ${env})"),
    }
}

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
        .block_on(f)
}
