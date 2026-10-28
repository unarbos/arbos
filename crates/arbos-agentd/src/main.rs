mod activity;
mod agent;
mod agent_messages;
mod approval;
mod attachments;
mod budget;
mod channels;
mod chart;
mod config;
mod connectors;
mod disk;
mod eval;
mod feedback;
mod goals;
mod groups;
mod handoff;
mod harness;
mod hooks;
mod http;
mod identity;
mod inspect;
mod isolation;
mod jobs_view;
mod llm;
mod local;
mod markdown_tables;
mod mcp;
mod media;
mod memory;
mod people;
mod relay_error;
mod reply;
mod resources;
mod responders;
mod routines;
mod rpc_child;
mod selfref;
mod settings;
mod silence;
mod sites;
mod skills;
mod status;
mod store;
mod sync;
mod tree;
mod truncate;
mod vault;
mod voice;
mod web;
mod workspace;

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use serde_json::{json, Value};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::config::BoxConfig;
use crate::workspace::Supervisor;

#[derive(Parser, Debug)]
#[command(name = "arbos-agentd", version, about = "Arbos box entrypoint")]
struct Args {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Boot the box: doc host, main agent, MCP, history store (image ENTRYPOINT).
    Boot,
    /// Print the effective configuration (secrets masked).
    Config,
    /// Run the labelled group-chat fixture through the responder decision (Jev decision model
    /// and the LLM router) and report accuracy, latency, tokens and cost. Needs
    /// OPENROUTER_API_KEY, or ARBOS_RELAY_URL + ARBOS_BOX_TOKEN for the relay proxy.
    EvalResponders {
        /// Fixture path (default: crates/arbos-agentd/tests/fixtures/group-responders.json).
        #[arg(
            long,
            default_value = "crates/arbos-agentd/tests/fixtures/group-responders.json"
        )]
        fixture: String,
        /// `jev`, `llm`, or `both`.
        #[arg(long, default_value = "both")]
        provider: String,
        /// Decision-model id (default typesafe/jev-1.14).
        #[arg(long)]
        scorer: Option<String>,
        /// Chat model for the `llm` router.
        #[arg(long, default_value = "anthropic/claude-haiku-5.5")]
        llm_model: String,
        /// Yes-probability threshold for `jev` (default 0.5).
        #[arg(long)]
        threshold: Option<f64>,
        /// Run every case this many times (latency spread).
        #[arg(long, default_value_t = 1)]
        repeat: usize,
        /// Write the JSON report here instead of stdout.
        #[arg(long)]
        out: Option<String>,
    },
    /// Memory tree (D77) offline check: build the tree over a copy of a workspace's `agentd.db`
    /// (cards through OpenRouter or the relay proxy, same env rules as eval-responders), walk one
    /// query as if asked in a new chat, and print the pack with every decision.
    EvalTree {
        /// Path to a copy of `<workspace>/.arbos/agentd/agentd.db` (the copy gets the tree tables).
        #[arg(long)]
        db: String,
        /// The question, as the user would type it in a new chat.
        #[arg(long)]
        query: String,
        /// The chat the question is asked in (its own recent window is excluded).
        #[arg(long, default_value = "new-chat")]
        chat: String,
        #[arg(long, default_value = "typesafe/jev-1.14")]
        scorer: String,
        #[arg(long, default_value = "anthropic/claude-haiku-5.5")]
        card_model: String,
        #[arg(long)]
        out: Option<String>,
        /// Pack with the compact policy (`ARBOS_MEMORY_PACK_COMPACT`) instead of the full one.
        #[arg(long)]
        compact: bool,
        /// Walk the D77 way (every thread scored, three Jev rounds, no cache) instead of the
        /// fast walk (`ARBOS_MEMORY_WALK_FAST`).
        #[arg(long)]
        slow: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,loro_internal=warn,arbos_agentd=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .with(inspect::WarnRing)
        .init();
    let args = Args::parse();
    match args.cmd {
        Cmd::Boot => {
            // Daytona starts us as `daytona`; per-workspace users need root (sudo is passwordless).
            let local_box = std::env::var("ARBOS_LOCAL_BOX").is_ok_and(|v| v == "1");
            isolation::reexec_as_root_if_needed(local_box);
            boot().await
        }
        Cmd::Config => {
            let cfg = BoxConfig::from_env()?;
            println!(
                "{}",
                json!({ "user_id": cfg.user_id, "relay_url": cfg.relay_url, "work_root": cfg.work_root, "port": cfg.port, "local_box": cfg.local_box, "box_token": mask(&cfg.box_token), "mcp_token": mask(&cfg.mcp_token), "default_talking_model": cfg.default_talking_model, "default_subagent_model": cfg.default_subagent_model, "legacy_project_id": cfg.legacy_project_id })
            );
            Ok(())
        }
        Cmd::EvalResponders {
            fixture,
            provider,
            scorer,
            llm_model,
            threshold,
            repeat,
            out,
        } => {
            eval::run(eval::EvalArgs {
                fixture,
                provider,
                scorer,
                llm_model,
                threshold,
                repeat,
                out,
            })
            .await
        }
        Cmd::EvalTree {
            db,
            query,
            chat,
            scorer,
            card_model,
            out,
            compact,
            slow,
        } => {
            let opts = tree::WalkOptions {
                compact,
                fast: !slow,
            };
            eval_tree(db, query, chat, scorer, card_model, out, opts).await
        }
    }
}

async fn eval_tree(
    db: String,
    query: String,
    chat: String,
    scorer: String,
    card_model: String,
    out: Option<String>,
    opts: tree::WalkOptions,
) -> Result<()> {
    let llm = match std::env::var("OPENROUTER_API_KEY")
        .ok()
        .filter(|s| !s.is_empty())
    {
        Some(key) => llm::Llm::with_endpoints(
            "https://openrouter.ai/api/v1",
            "https://openrouter.ai/api/alpha/decisions",
            &key,
        ),
        None => {
            let relay = std::env::var("ARBOS_RELAY_URL")
                .ok()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    anyhow::anyhow!("set OPENROUTER_API_KEY, or ARBOS_RELAY_URL + ARBOS_BOX_TOKEN")
                })?;
            let token = std::env::var("ARBOS_BOX_TOKEN").unwrap_or_default();
            let relay = relay.trim_end_matches('/');
            llm::Llm::with_endpoints(
                &format!("{relay}/proxy/v1"),
                &format!("{relay}/proxy/decisions"),
                &token,
            )
        }
    };
    let store = store::Store::open(std::path::Path::new(&db))?;
    let tree = tree::Tree::with_ids("eval", "eval", &card_model, store, std::sync::Arc::new(llm));
    let report = tree.eval(&scorer, &chat, &query, opts).await?;
    let text = serde_json::to_string_pretty(&report)?;
    match out {
        Some(p) => std::fs::write(p, text)?,
        None => println!("{text}"),
    }
    Ok(())
}

fn mask(s: &str) -> String {
    if s.len() > 8 {
        format!("{}…({} chars)", &s[..6], s.len())
    } else {
        "(unset)".into()
    }
}

/// The relay URL baked into the sandbox env goes stale when the relay's public URL changes (quick
/// tunnels change on every restart). The relay writes the current URL to `<state>/relay-url` at
/// every wake; agentd prefers a reachable file URL over the env, and keeps re-reading the file
/// until one of the candidates answers `/healthz`. Returns the URL that answered.
async fn resolve_relay_url() -> String {
    // The relay uploads the file as the sandbox user, which cannot write under the root-owned
    // `/work` any more (D70 isolation): the file lives in that user's home. The old location is
    // still read for boxes created before the move.
    let file = std::path::PathBuf::from(
        std::env::var("ARBOS_RELAY_URL_FILE")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "/home/daytona/.arbos/relay-url".to_owned()),
    );
    let legacy_file = std::path::PathBuf::from(format!(
        "{}/.arbos/relay-url",
        std::env::var("ARBOS_WORK_DIR").unwrap_or_else(|_| "/work".into())
    ));
    let env_url = std::env::var("ARBOS_RELAY_URL")
        .ok()
        .map(|u| u.trim().trim_end_matches('/').to_owned())
        .filter(|u| !u.is_empty());
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("client");
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        let read = |p: &std::path::Path| {
            std::fs::read_to_string(p)
                .ok()
                .map(|u| u.trim().trim_end_matches('/').to_owned())
                .filter(|u| !u.is_empty())
        };
        let file_url = read(&file);
        let legacy_url = read(&legacy_file);
        let mut candidates: Vec<String> = Vec::new();
        for u in [file_url.clone(), legacy_url, env_url.clone()]
            .into_iter()
            .flatten()
        {
            if !candidates.contains(&u) {
                candidates.push(u);
            }
        }
        for u in &candidates {
            let ok = http
                .get(format!("{u}/healthz"))
                .send()
                .await
                .map(|r| r.status().is_success())
                .unwrap_or(false);
            if ok {
                if file_url.as_deref() != Some(u.as_str()) {
                    // agentd runs as root; the relay's next push arrives as the sandbox user, so
                    // the directory and the file must stay writable for it — and the directory
                    // must not be sticky, or that user could not replace a root-written file.
                    use std::os::unix::fs::PermissionsExt;
                    if let Some(dir) = file.parent() {
                        if std::fs::create_dir_all(dir).is_ok() {
                            let _ = std::fs::set_permissions(
                                dir,
                                std::fs::Permissions::from_mode(0o777),
                            );
                        }
                    }
                    if std::fs::write(&file, format!("{u}\n")).is_ok() {
                        let _ =
                            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o666));
                    }
                }
                if env_url.as_deref() != Some(u.as_str()) {
                    tracing::warn!(url = %u, env = ?env_url, "relay url from the wake file (env is stale)");
                }
                return u.clone();
            }
        }
        if attempt == 1 || attempt % 15 == 0 {
            tracing::warn!(
                ?candidates,
                attempt,
                "no relay url answers /healthz; waiting for the relay to push one"
            );
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

async fn boot() -> Result<()> {
    let t_boot = Instant::now();
    let relay_url = resolve_relay_url().await;
    // Config reads the env; the resolved URL wins over whatever the sandbox was created with.
    std::env::set_var("ARBOS_RELAY_URL", &relay_url);
    let cfg = BoxConfig::from_env()?;
    std::fs::create_dir_all(cfg.box_state_dir().join("logs"))?;
    // Disk headroom (#1641): arm the ballast and keep it armed while there is room.
    disk::boot(&cfg.work_root)?;
    disk::spawn_ballast_keeper(cfg.work_root.clone());
    // Settings → Machine: CPU / memory / disk / network of the box and of every workspace,
    // sampled every 5 s for `GET /box/state`.
    resources::start(cfg.work_root.clone());
    let sup = Supervisor::new(cfg.clone());

    // Box HTTP (health, per-workspace history/metrics, browser stream) first: the relay's preview
    // URL and the stale-box detector look here.
    let main_l = tokio::net::TcpListener::bind(format!("{}:{}", cfg.bind, cfg.port))
        .await
        .context("bind main port")?;
    let main_app = http::box_router(sup.clone());
    tokio::spawn(async move { axum::serve(main_l, main_app).await.ok() });

    // Hello: one box per user; the answer lists every project as a workspace.
    let http_client = reqwest::Client::new();
    let listing = loop {
        let hello = http_client
            .post(format!("{}/box/hello", cfg.relay_url))
            .bearer_auth(&cfg.box_token)
            .json(&json!({
                "url": format!("http://127.0.0.1:{}", cfg.port),
                "version": env!("CARGO_PKG_VERSION"),
                "proto": arbos_core::PROTO_VERSION,
                // Baked by image.yml (ARBOS_AGENTD_SHA / ARBOS_IMAGE_VERSION): which commit this
                // box runs, so the relay admin can tell what is shipped.
                "sha": std::env::var("ARBOS_AGENTD_SHA").ok().filter(|v| !v.is_empty() && v != "unknown"),
                "image": std::env::var("ARBOS_IMAGE_VERSION").ok().filter(|v| !v.is_empty() && v != "unknown"),
                // The pi-ext seed this box actually copies into its projects (#1708): on a host
                // sandbox /etc is persisted, so it can lag behind the image.
                "seed_sha": std::fs::read_to_string("/etc/arbos/pi-seed/extensions/arbos-pi-ext/.source-sha")
                    .ok()
                    .map(|s| s.trim().to_owned())
                    .filter(|s| !s.is_empty()),
            }))
            .send()
            .await;
        match hello {
            Ok(r) if r.status().is_success() => break r.json::<Value>().await.unwrap_or_default(),
            Ok(r) if r.status() == reqwest::StatusCode::CONFLICT => {
                let text = r.text().await.unwrap_or_default();
                anyhow::bail!(
                    "relay refused this agentd: {}",
                    crate::relay_error::message(&text)
                )
            }
            Ok(r) => tracing::warn!(status = %r.status(), "box hello refused; retrying"),
            Err(e) => tracing::warn!(error = %e, "box hello failed; retrying"),
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    };
    let user_id = listing
        .get("user_id")
        .and_then(Value::as_str)
        .unwrap_or("u_default")
        .to_owned();
    sup.apply_listing(&listing).await;
    // Calls Phase 2: the relay's voice link (tool calls and transcript rows from the voice worker).
    voice::spawn_voice_link(sup.clone());
    let n = sup.list().await.len();
    tracing::info!(
        user = %user_id,
        workspaces = n,
        ms = t_boot.elapsed().as_millis() as i64,
        "arbos-agentd booted"
    );
    if let Some(w) = sup.list().await.first() {
        let _ = w
            .agent
            .post_timing("agentd_boot_ms", t_boot.elapsed().as_millis() as i64, None)
            .await;
    }
    // The read-only copy of Arbos's own source (/opt/arbos/src) follows main: refreshed now, in
    // the background, and every few hours after.
    selfref::spawn_refresh();

    // Registry poll (3 s): new projects become workspaces, closed ones stop, settings changes
    // (SetModel, rename, new chats) reach each workspace's pi and doc.
    {
        let sup2 = sup.clone();
        let cfg2 = cfg.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(3)).await;
                let Ok(r) = reqwest::Client::new()
                    .get(format!("{}/box/projects", cfg2.relay_url))
                    .bearer_auth(&cfg2.box_token)
                    .send()
                    .await
                else {
                    continue;
                };
                if !r.status().is_success() {
                    continue;
                }
                let Ok(v) = r.json::<Value>().await else {
                    continue;
                };
                sup2.apply_listing(&v).await;
            }
        });
    }
    // Idle keep-alive heartbeat (turn_running=false lets the relay restore auto-stop).
    {
        let sup2 = sup.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                let running = sup2.any_turn_running().await;
                let _ = sup2.keepalive(running).await;
            }
        });
    }
    // Shutdown: SIGTERM → every workspace persists docs + checkpoints and stops its children.
    let mut term =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("sigterm");
    term.recv().await;
    tracing::info!("SIGTERM: stopping workspaces and persisting docs");
    let _ = tokio::time::timeout(Duration::from_secs(45), sup.shutdown()).await;
    std::process::exit(0);
}
