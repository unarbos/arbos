//! `arbos-kernel prompt <place> [--agent ID]`: what every model call of
//! that agent carries before the conversation — the contract, the project
//! context, the instance prompt by block, the plan segment, the tool
//! schemas by tool — with token estimates (chars/4), and the total. The
//! same numbers the kernel logs per turn as `prompt_size`.

use std::sync::Arc;

use anyhow::{Context, Result, bail};
use arbos_core::{AgentId, Place, load_agent};

pub struct Args {
    pub place: Place,
    pub agent: String,
    pub json: bool,
    pub dump: bool,
}

impl Args {
    pub fn parse(mut args: impl Iterator<Item = String>) -> Result<Self> {
        let mut place = None;
        let mut agent = "root".to_string();
        let mut json = false;
        let mut dump = false;
        while let Some(a) = args.next() {
            match a.as_str() {
                "--agent" => agent = args.next().context("--agent needs an id")?,
                "--json" => json = true,
                "--dump" => dump = true,
                p if !p.starts_with('-') => place = Some(Place::new(p)),
                other => bail!("prompt: unknown flag {other}"),
            }
        }
        let place =
            place.unwrap_or_else(|| Place::new(std::env::current_dir().unwrap_or_default()));
        Ok(Self {
            place,
            agent,
            json,
            dump,
        })
    }
}

pub fn run(args: Args) -> Result<i32> {
    let mut agent = load_agent(&args.place, &AgentId::new(&args.agent))
        .with_context(|| format!("no agent {} in {}", args.agent, args.place.path.display()))?;
    arbos_core::project::apply_role(&args.place, &mut agent);
    let (wake_tx, _wake_rx) = tokio::sync::mpsc::unbounded_channel();
    let (kick_tx, _kick_rx) = tokio::sync::mpsc::unbounded_channel();
    let hooks = crate::hooks::KernelHooks::new(args.place.clone(), wake_tx, kick_tx);
    let ptys = Arc::new(crate::pty::PtyHub::new());
    let registry = crate::serve::kernel_registry(&hooks, &ptys);
    let view = registry.view(&agent);
    let skills: Vec<String> = arbos_core::load_skills(&args.place)
        .iter()
        .map(arbos_core::Skill::roster_line)
        .collect();
    if args.dump {
        // Every section as JSON lines {section, text}: for a real tokenizer.
        for (k, text) in arbos_engine::project::sections(&args.place, &agent, &skills, &view) {
            println!(
                "{}",
                serde_json::json!({"section": k.trim(), "nested": k.starts_with("  "), "text": text})
            );
        }
        return Ok(0);
    }
    let rows = arbos_engine::project::measure(&args.place, &agent, &skills, &view);
    let total: u64 = rows
        .iter()
        .filter(|(k, _)| !k.starts_with("  "))
        .map(|(_, n)| n)
        .sum();
    if args.json {
        let obj: serde_json::Map<String, serde_json::Value> = rows
            .iter()
            .map(|(k, n)| (k.trim().to_string(), serde_json::Value::from(*n)))
            .chain(std::iter::once((
                "total".to_string(),
                serde_json::Value::from(total),
            )))
            .collect();
        println!("{}", serde_json::Value::Object(obj));
    } else {
        for (k, n) in &rows {
            println!("{n:>6}  {k}");
        }
        println!(
            "{total:>6}  total ({} as {}{})",
            agent.id,
            agent.role.as_deref().unwrap_or("worker"),
            if agent.kind.is_empty() {
                String::new()
            } else {
                format!(", kind {}", agent.kind)
            }
        );
    }
    Ok(0)
}
