//! Headless Arbos client (design §9 M0): pair, create project, open chat, send,
//! stream the reply, resume by cursor after a kill, stop/start the box.

mod client;
mod room;

use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use serde_json::{json, Value};

use crate::client::{Config, Relay};

#[derive(Parser, Debug)]
#[command(name = "arbosctl", version, about = "Headless Arbos client")]
struct Args {
    /// Relay base URL (overrides the saved one).
    #[arg(long, env = "ARBOS_RELAY_URL")]
    relay: Option<String>,
    /// Config file (device token, cursors).
    #[arg(long, env = "ARBOSCTL_CONFIG")]
    config: Option<std::path::PathBuf>,
    /// Print JSON instead of text where it applies.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Exchange a one-time pairing code for a device token (D35).
    Pair {
        code: String,
        #[arg(long, default_value = "arbosctl")]
        name: String,
    },
    /// Projects.
    Project {
        #[command(subcommand)]
        cmd: ProjectCmd,
    },
    /// Chats.
    Chat {
        #[command(subcommand)]
        cmd: ChatCmd,
    },
    /// Box lifecycle.
    Box {
        #[command(subcommand)]
        cmd: BoxCmd,
    },
    /// Today's and all-time metered usage.
    Usage { project: String },
    /// Recorded timings (boot, wake, TTFT, cold start).
    Timings { project: String },
    /// The project's audit trail as mirrored on the relay (approvals, tool calls).
    Audit {
        project: String,
        #[arg(long, default_value_t = 50)]
        limit: i64,
    },
    /// The user's machine (one box for all projects, D70): state, class, spend, workspaces.
    Machine {
        #[command(subcommand)]
        cmd: MachineCmd,
    },
    /// Copy every legacy per-project sandbox into the user box (idempotent; old sandboxes are
    /// stopped and marked `migrated_to`, never deleted).
    MigrateToUserBox {
        /// Only this project id.
        #[arg(long)]
        project: Option<String>,
        /// Copy again even if already marked migrated.
        #[arg(long)]
        force: bool,
    },
    /// Devices.
    Devices {
        #[command(subcommand)]
        cmd: DeviceCmd,
    },
    /// Operator commands (admin token from ARBOS_RELAY_ADMIN_TOKEN or --admin-token): users,
    /// credits, the global kill switch.
    Admin {
        #[arg(long, env = "ARBOS_RELAY_ADMIN_TOKEN")]
        admin_token: String,
        #[command(subcommand)]
        cmd: AdminCmd,
    },
    /// Routines: cron / webhook / event-triggered prompts run by the relay scheduler.
    Routines {
        #[command(subcommand)]
        cmd: RoutineCmd,
    },
    /// Skills library (F5): reusable how-tos shared across Projects.
    Skills {
        #[command(subcommand)]
        cmd: SkillsCmd,
    },
    /// Channels (Slack, Telegram, WhatsApp, Beeper): pairing codes, bindings, mock inbound/outbox.
    Channels {
        #[command(subcommand)]
        cmd: ChannelsCmd,
    },
    /// Blobs (attachments, offloaded tool outputs) from the relay mirror.
    Blob {
        #[command(subcommand)]
        cmd: BlobCmd,
    },
    /// Group chats: one chat shared by several Projects' agents (F6).
    Group {
        #[command(subcommand)]
        cmd: GroupCmd,
    },
}

#[derive(Subcommand, Debug)]
enum BlobCmd {
    /// Download `GET /blob/{project}/{key}` to a file (default: ./<key>, or the name after `--name`).
    Get {
        project: String,
        key: String,
        /// Output path.
        #[arg(long, short)]
        out: Option<std::path::PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
enum GroupCmd {
    /// Create a group hosted by --host with the other --with projects (ids or names).
    Create {
        #[arg(long)]
        host: String,
        /// Comma-separated project ids or names.
        #[arg(long, value_delimiter = ',')]
        with: Vec<String>,
        #[arg(long)]
        title: Option<String>,
    },
    /// The group entry with participant details.
    Show { host: String, chat: String },
    /// Add participants (comma-separated ids or names).
    Add {
        host: String,
        chat: String,
        #[arg(value_delimiter = ',')]
        projects: Vec<String>,
    },
    /// Remove participants (comma-separated ids or names).
    Remove {
        host: String,
        chat: String,
        #[arg(value_delimiter = ',')]
        projects: Vec<String>,
    },
    /// Send a user message into the group (host chat); with --stream, print the host's reply
    /// and the other Projects' answers as they land, until --wait seconds pass.
    Send {
        host: String,
        chat: String,
        text: String,
        #[arg(long)]
        stream: bool,
        /// Keep listening this long after the host's reply for the other Projects' answers.
        #[arg(long, default_value_t = 90)]
        wait: u64,
    },
}

#[derive(Subcommand, Debug)]
enum RoutineCmd {
    List {
        project: String,
    },
    /// Create a routine. Exactly one of --cron, --webhook, --event.
    Create {
        project: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        prompt: String,
        /// Chat id the prompt lands in (default: the pinned chat).
        #[arg(long)]
        chat: Option<String>,
        /// Cron expression, e.g. "0 9 * * 1-5".
        #[arg(long)]
        cron: Option<String>,
        /// IANA time zone for --cron (default UTC).
        #[arg(long, default_value = "UTC")]
        tz: String,
        /// Webhook trigger; prints the fire URL.
        #[arg(long)]
        webhook: bool,
        /// Event trigger: cross_project_message | connector_event | box_awake.
        #[arg(long)]
        event: Option<String>,
        /// Create it paused.
        #[arg(long)]
        paused: bool,
    },
    /// Run a routine now.
    Run {
        project: String,
        routine: String,
    },
    /// Last runs of a routine (newest first).
    Runs {
        project: String,
        routine: String,
        #[arg(long, default_value_t = 20)]
        limit: i64,
    },
    Pause {
        project: String,
        routine: String,
    },
    Resume {
        project: String,
        routine: String,
    },
    Delete {
        project: String,
        routine: String,
    },
}

#[derive(Subcommand, Debug)]
enum SkillsCmd {
    /// List skills; with --q, search (FTS over name/description/tags/when to use).
    List {
        #[arg(long)]
        q: Option<String>,
    },
    /// Print one skill as Markdown (frontmatter + body).
    Show { slug: String },
    /// Add a skill from flags or from a Markdown file with frontmatter (--file).
    Add {
        #[arg(long)]
        file: Option<std::path::PathBuf>,
        #[arg(long)]
        slug: Option<String>,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        description: Option<String>,
        /// Comma-separated.
        #[arg(long)]
        tags: Option<String>,
        #[arg(long)]
        when_to_use: Option<String>,
        /// Body Markdown (when no --file).
        #[arg(long)]
        body: Option<String>,
        /// Visible to every Project (default: only the origin Project, or all when none is set).
        #[arg(long)]
        shared: bool,
        /// Origin project id or name.
        #[arg(long)]
        project: Option<String>,
    },
    /// Delete a skill by id or slug.
    Delete { slug: String },
}

#[derive(Subcommand, Debug)]
enum ChannelsCmd {
    /// Platform states, bound chats, and the active pairing code.
    List,
    /// Mint a 6-digit pairing code for a project (its pinned chat unless --chat); send `/pair <code>` from the channel.
    Pair {
        project: String,
        #[arg(long)]
        chat: Option<String>,
    },
    /// Remove a binding by id.
    Unbind { id: String },
    /// Mock mode only: inject an inbound message as if the platform delivered it.
    Inbound {
        text: String,
        #[arg(long, default_value = "telegram")]
        platform: String,
        #[arg(long, default_value = "mock-chat")]
        chat: String,
        #[arg(long, default_value = "mock-user")]
        user: String,
        #[arg(long)]
        sender: Option<String>,
    },
    /// What was (or would have been) sent back to the platforms, newest first.
    Outbox {
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
}

#[derive(Subcommand, Debug)]
enum MachineCmd {
    Status,
    Start,
    Stop {
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand, Debug)]
enum ProjectCmd {
    Create {
        #[arg(long)]
        name: String,
        #[arg(long)]
        instructions: Option<String>,
        /// Box provider: daytona (default when the relay has a key), process, docker.
        #[arg(long)]
        provider: Option<String>,
        /// Shorthand for `--provider process` (runs agentd on the relay host).
        #[arg(long)]
        local_box: bool,
    },
    List,
    Status {
        project: String,
    },
    /// Soft-delete a project you own (workspace archived on the box; purged after 7 days).
    Delete {
        project: String,
    },
}

#[derive(Subcommand, Debug)]
enum ChatCmd {
    /// Create a chat; prints its id.
    Open {
        project: String,
        #[arg(long)]
        title: Option<String>,
    },
    List {
        project: String,
    },
    /// Send a message; with --stream, print the reply as it arrives.
    Send {
        project: String,
        chat: String,
        text: String,
        #[arg(long)]
        stream: bool,
        /// Give up waiting for the reply after N seconds.
        #[arg(long, default_value_t = 600)]
        timeout: u64,
        /// Reply to this message id (`replyTo` on the row, as the apps' reply arrow sets it).
        #[arg(long = "reply-to")]
        reply_to: Option<String>,
        /// Reply in DM: a cross-chat reply reference as JSON (`replyRef` on the row, crdt-sync
        /// §6.2): `{"chatId","projectId","messageId","chatTitle","sender","senderName","senderHandle","text"}`.
        #[arg(long = "reply-ref")]
        reply_ref: Option<String>,
        /// Attach a file (a picture, a PDF, …): uploaded to the chat's blob slot first, then sent
        /// as an attachment part beside the text (the apps' shape).
        #[arg(long = "attach")]
        attach: Option<std::path::PathBuf>,
    },
    /// Answer the agent's open question with a structured value (question widgets).
    Answer {
        project: String,
        chat: String,
        /// JSON value: "id" for choice, ["a","b"] for multi_select, "text", "2026-12-01" for
        /// date, {"field": value} for form. A bare word is taken as a string.
        #[arg(long = "answer-json", visible_alias = "value")]
        answer_json: String,
        /// Question id (default: the newest open question in the chat).
        #[arg(long)]
        question: Option<String>,
        #[arg(long)]
        stream: bool,
        #[arg(long, default_value_t = 600)]
        timeout: u64,
    },
    /// Join the room at the saved cursor and print rows as they arrive (resume after a kill).
    Stream {
        project: String,
        chat: String,
        /// Exit once the newest assistant message is done.
        #[arg(long)]
        until_done: bool,
        #[arg(long, default_value_t = 600)]
        timeout: u64,
    },
    /// Print the local replica of the chat (messages as JSON) after catching up.
    Dump {
        project: String,
        chat: String,
    },
    /// Stop the running turn in a chat now (aborts the main agent's pi turn / act loop).
    Stop {
        project: String,
        chat: String,
    },
    /// Approve a pending approval card (its id is printed with the card).
    Approve {
        project: String,
        chat: String,
        approval_id: String,
    },
    /// Reject a pending approval card.
    Reject {
        project: String,
        chat: String,
        approval_id: String,
        #[arg(long)]
        reason: Option<String>,
    },
    /// Forget the local replica and cursor for a chat.
    Reset {
        project: String,
        chat: String,
    },
}

#[derive(Subcommand, Debug)]
enum BoxCmd {
    Stop {
        project: String,
        #[arg(long)]
        force: bool,
    },
    Start {
        project: String,
    },
    Wake {
        project: String,
    },
    Status {
        project: String,
    },
}

#[derive(Subcommand, Debug)]
enum DeviceCmd {
    List,
    Revoke { id: String },
}

#[derive(Subcommand, Debug)]
enum MatrixCmd {
    /// Mode, homeserver reachability, outbox depth, rooms and events mirrored, flipped users.
    Status {
        /// Also list parked (failed) jobs.
        #[arg(long)]
        failed: bool,
    },
    /// Queue existing chats into the homeserver: rooms, members, history with original timestamps.
    Backfill {
        #[arg(long)]
        user: Option<String>,
        #[arg(long)]
        chat: Option<String>,
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        rooms_only: bool,
    },
    /// Shadow-compare the relay against the homeserver (the stage B gate).
    Shadow {
        #[arg(long)]
        chat: Option<String>,
        /// Also compare per-user unread counts.
        #[arg(long)]
        unread: bool,
        /// Print the newest nightly report instead of running one.
        #[arg(long)]
        last: bool,
    },
    /// Per-user read flip (`on` serves that user's reads from the homeserver; `off` rolls back).
    Reads { user: String, state: String },
    /// Put parked jobs back in the queue and retry everything that is waiting.
    Retry,
}

#[derive(Subcommand, Debug)]
enum AdminCmd {
    /// Every account with balance, caps, today's spend, and machine state.
    Users,
    /// Add credits to a user (USD).
    Grant {
        user: String,
        usd: f64,
        #[arg(long)]
        note: Option<String>,
    },
    /// Set a user's daily cap in USD, or `--unlimited`.
    Cap {
        user: String,
        #[arg(required_unless_present = "unlimited")]
        usd: Option<f64>,
        #[arg(long)]
        unlimited: bool,
    },
    /// A test account with no GitHub behind it (recordings, two-account tests): prints the
    /// user and its token pair.
    TestUser {
        handle: String,
        #[arg(long)]
        name: Option<String>,
    },
    /// Delete a test user (only `test_…` users; their projects and box must be gone first).
    DeleteTestUser { user: String },
    /// Attach another signed-in device to a user and print its token pair (test users; a real
    /// account needs --name).
    AddDevice {
        user: String,
        #[arg(long)]
        name: Option<String>,
    },
    /// Recreate a user's box from the current default snapshot, carrying /work (the old sandbox
    /// is kept 7 days). Progress: `GET /machine` → upgrade.phase.
    UpgradeBox {
        user: String,
        #[arg(long)]
        force: bool,
    },
    /// Re-adopt a retained sandbox for a user whose box row lost its sandbox (failed upgrade).
    RepairBox { user: String },
    /// Stop and delete a user's sandbox and clear the box row (a signed-in user gets a fresh box
    /// within a minute; a test user stays without one).
    DeleteBox { user: String },
    /// Calls: LiveKit configured?, live calls with participants, the last 20 ended calls.
    Calls,
    /// Chats on Matrix (D88): `status [--failed]`, `backfill [--user <id>] [--chat <id> [--project <id>]] [--rooms-only]`,
    /// `shadow [--chat <id>] [--unread]`, `reads <user> on|off`, `retry`.
    Matrix {
        #[command(subcommand)]
        cmd: MatrixCmd,
    },
    /// iOS push: `status` lists whether APNs is configured and every registered device token;
    /// `test --user <id> | --device <id>` sends one plain alert and prints APNs' answer.
    Push {
        #[arg(value_parser = ["status", "test"])]
        action: String,
        #[arg(long)]
        user: Option<String>,
        #[arg(long)]
        device: Option<String>,
        #[arg(long)]
        body: Option<String>,
    },
    /// Global kill switch: `on [--reason …]` makes the relay read-only, `off` clears it,
    /// `status` shows it with today's relay-wide spend.
    Killswitch {
        #[arg(value_parser = ["on", "off", "status"])]
        action: String,
        #[arg(long)]
        reason: Option<String>,
    },
    /// Host fleet (D89): `status` shows host pressure, capacity, every gvisor sandbox with live
    /// stats, the waitlist and recent evictions; `cap <n>` sets how many always-on sandboxes the
    /// host accepts (persisted).
    Fleet {
        #[arg(value_parser = ["status", "cap"])]
        action: String,
        /// New `max_boxes` for `cap`.
        n: Option<usize>,
    },
    /// Waitlist (D89): `list` shows who is in line; `admit <user>` lets one user through now
    /// (refused while the host is full).
    Waitlist {
        #[arg(value_parser = ["list", "admit"])]
        action: String,
        user: Option<String>,
    },
    /// Agent-hosted sites (#2060): `list`; `takedown <user> <app> [--reason …]` (410 at the edge,
    /// owner cannot republish); `restore <user> <app>`; `purge <user> <app>`; `events [--since <ms>]`;
    /// `quota <user> [--max-sites N] [--max-bytes N] [--block|--unblock]`; `move <user> <new-prefix>`
    /// (old address 301s for 30 days).
    Sites {
        #[arg(value_parser = ["list", "takedown", "restore", "purge", "events", "quota", "move"])]
        action: String,
        user: Option<String>,
        /// App name (takedown/restore/purge) or the new prefix (move).
        arg: Option<String>,
        #[arg(long)]
        reason: Option<String>,
        #[arg(long)]
        since: Option<i64>,
        #[arg(long)]
        max_sites: Option<i64>,
        #[arg(long)]
        max_bytes: Option<i64>,
        #[arg(long)]
        block: bool,
        #[arg(long)]
        unblock: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,loro_internal=error".into()),
        )
        .init();
    let args = Args::parse();
    let mut cfg = Config::load(args.config.clone())?;
    if let Some(r) = &args.relay {
        cfg.relay_url = r.trim_end_matches('/').to_owned();
    }
    let relay = Relay::new(&cfg);
    match args.cmd {
        Cmd::Pair { code, name } => {
            let v = relay
                .post_unauth("/pair", &json!({ "code": code, "name": name }))
                .await?;
            cfg.device_token = v
                .get("device_token")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            cfg.device_id = v
                .get("device_id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            cfg.save()?;
            println!(
                "paired as device {} (token saved to {})",
                cfg.device_id,
                cfg.path.display()
            );
        }
        Cmd::Project { cmd } => match cmd {
            ProjectCmd::Create {
                name,
                instructions,
                provider,
                local_box,
            } => {
                let provider = provider.or(if local_box {
                    Some("process".into())
                } else {
                    None
                });
                let t0 = std::time::Instant::now();
                let v = relay.post("/projects", &json!({ "name": name, "instructions": instructions, "provider": provider })).await?;
                if args.json {
                    println!("{}", serde_json::to_string_pretty(&v)?);
                } else {
                    println!(
                        "project {} ({}) created in {:.1}s",
                        v.pointer("/project/id")
                            .and_then(Value::as_str)
                            .unwrap_or("?"),
                        name,
                        t0.elapsed().as_secs_f64()
                    );
                    println!(
                        "pinned chat {}",
                        v.pointer("/pinned_chat/id")
                            .and_then(Value::as_str)
                            .unwrap_or("?")
                    );
                    println!(
                        "box: {}",
                        v.get("sandbox").map(|s| s.to_string()).unwrap_or_default()
                    );
                }
            }
            ProjectCmd::List => {
                let v = relay.get("/projects").await?;
                if args.json {
                    println!("{}", serde_json::to_string_pretty(&v)?);
                } else if let Some(a) = v.as_array() {
                    for p in a {
                        println!(
                            "{}  {:<20} {:<16} box={}",
                            p["id"].as_str().unwrap_or(""),
                            p["name"].as_str().unwrap_or(""),
                            p["state"].as_str().unwrap_or(""),
                            p["box"].as_str().unwrap_or("")
                        );
                    }
                }
            }
            ProjectCmd::Delete { project } => {
                relay.delete(&format!("/projects/{project}")).await?;
                println!("deleted {project} (archived on the box; purged in 7 days)");
            }
            ProjectCmd::Status { project } => {
                let v = relay.get(&format!("/projects/{project}/status")).await?;
                println!("{}", serde_json::to_string_pretty(&v)?);
            }
        },
        Cmd::Chat { cmd } => match cmd {
            ChatCmd::Open { project, title } => {
                let v = relay
                    .post(
                        &format!("/projects/{project}/chats"),
                        &json!({ "title": title }),
                    )
                    .await?;
                let id = v.pointer("/chat/id").and_then(Value::as_str).unwrap_or("?");
                if args.json {
                    println!("{}", serde_json::to_string_pretty(&v)?);
                } else {
                    println!("{id}");
                }
            }
            ChatCmd::List { project } => {
                let v = relay.get(&format!("/projects/{project}/chats")).await?;
                println!("{}", serde_json::to_string_pretty(&v)?);
            }
            ChatCmd::Send {
                project,
                chat,
                text,
                stream,
                timeout,
                reply_to,
                reply_ref,
                attach,
            } => {
                let project = resolve_project(&relay, &project).await?;
                let attachment = match attach {
                    Some(path) => Some(upload_attachment(&relay, &project, &chat, &path).await?),
                    None => None,
                };
                let reply_ref = match reply_ref {
                    Some(j) => Some(
                        serde_json::from_str::<arbos_core::doc::ReplyRef>(&j)
                            .map_err(|e| anyhow::anyhow!("--reply-ref: {e}"))?,
                    ),
                    None => None,
                };
                room::send(
                    &mut cfg,
                    &project,
                    &chat,
                    &text,
                    reply_to.as_deref(),
                    reply_ref.as_ref(),
                    attachment.as_ref(),
                    stream,
                    timeout,
                )
                .await?;
            }
            ChatCmd::Answer {
                project,
                chat,
                answer_json,
                question,
                stream,
                timeout,
            } => {
                let project = resolve_project(&relay, &project).await?;
                let value: Value = serde_json::from_str(&answer_json)
                    .unwrap_or_else(|_| Value::String(answer_json.clone()));
                room::answer(
                    &mut cfg,
                    &project,
                    &chat,
                    question.as_deref(),
                    value,
                    stream,
                    timeout,
                )
                .await?;
            }
            ChatCmd::Stream {
                project,
                chat,
                until_done,
                timeout,
            } => {
                let project = resolve_project(&relay, &project).await?;
                room::stream(&mut cfg, &project, &chat, until_done, timeout).await?;
            }
            ChatCmd::Dump { project, chat } => {
                let project = resolve_project(&relay, &project).await?;
                room::dump(&mut cfg, &project, &chat).await?;
            }
            ChatCmd::Stop { project, chat } => {
                let project = resolve_project(&relay, &project).await?;
                room::stop(&mut cfg, &project, &chat).await?;
            }
            ChatCmd::Approve {
                project,
                chat,
                approval_id,
            } => {
                let project = resolve_project(&relay, &project).await?;
                room::decide(&mut cfg, &project, &chat, true, &approval_id, None).await?;
            }
            ChatCmd::Reject {
                project,
                chat,
                approval_id,
                reason,
            } => {
                let project = resolve_project(&relay, &project).await?;
                room::decide(
                    &mut cfg,
                    &project,
                    &chat,
                    false,
                    &approval_id,
                    reason.as_deref(),
                )
                .await?;
            }
            ChatCmd::Reset { project, chat } => {
                let project = resolve_project(&relay, &project).await?;
                room::reset(&cfg, &project, &chat)?;
                println!("reset local replica for {project}/{chat}");
            }
        },
        Cmd::Box { cmd } => match cmd {
            BoxCmd::Stop { project, force } => {
                let t0 = std::time::Instant::now();
                let v = relay
                    .post(
                        &format!("/projects/{project}/stop?force={force}"),
                        &json!({}),
                    )
                    .await?;
                println!("stopped in {:.1}s: {v}", t0.elapsed().as_secs_f64());
            }
            BoxCmd::Start { project } => {
                let t0 = std::time::Instant::now();
                let v = relay
                    .post(&format!("/projects/{project}/start"), &json!({}))
                    .await?;
                println!("start accepted in {:.1}s: {v}", t0.elapsed().as_secs_f64());
            }
            BoxCmd::Wake { project } => {
                let v = relay
                    .post(&format!("/projects/{project}/wake"), &json!({}))
                    .await?;
                println!("{v}");
            }
            BoxCmd::Status { project } => {
                let v = relay.get(&format!("/projects/{project}/status")).await?;
                println!("{}", serde_json::to_string_pretty(&v)?);
            }
        },
        Cmd::Machine { cmd } => {
            let v = match cmd {
                MachineCmd::Status => relay.get("/machine").await?,
                MachineCmd::Start => relay.post("/machine/start", &json!({})).await?,
                MachineCmd::Stop { force } => {
                    relay
                        .post(&format!("/machine/stop?force={force}"), &json!({}))
                        .await?
                }
            };
            println!("{}", serde_json::to_string_pretty(&v)?);
        }
        Cmd::MigrateToUserBox { project, force } => {
            let mut path = format!("/machine/migrate?force={force}");
            if let Some(p) = project {
                path.push_str(&format!("&project={p}"));
            }
            let v = relay.post(&path, &json!({})).await?;
            if args.json {
                println!("{}", serde_json::to_string_pretty(&v)?);
            } else {
                println!(
                    "user {}",
                    v.get("user_id").and_then(|u| u.as_str()).unwrap_or("?")
                );
                for r in v
                    .get("reports")
                    .and_then(|r| r.as_array())
                    .cloned()
                    .unwrap_or_default()
                {
                    println!(
                        "  {} legacy={} {} {} {}",
                        r.get("project_id").and_then(|x| x.as_str()).unwrap_or("?"),
                        r.get("legacy_sandbox")
                            .and_then(|x| x.as_str())
                            .unwrap_or("-"),
                        r.get("outcome").and_then(|x| x.as_str()).unwrap_or("?"),
                        r.get("work_mib")
                            .and_then(|x| x.as_i64())
                            .map(|m| format!("{m} MiB"))
                            .unwrap_or_default(),
                        r.get("ms")
                            .and_then(|x| x.as_i64())
                            .map(|m| format!("{:.1}s", m as f64 / 1000.0))
                            .unwrap_or_default(),
                    );
                }
            }
        }
        Cmd::Usage { project } => {
            let v = relay.get(&format!("/projects/{project}/usage")).await?;
            println!("{}", serde_json::to_string_pretty(&v)?);
        }
        Cmd::Timings { project } => {
            let v = relay.get(&format!("/projects/{project}/status")).await?;
            let t = v.get("timings").cloned().unwrap_or(json!([]));
            if args.json {
                println!("{}", serde_json::to_string_pretty(&t)?);
            } else if let Some(a) = t.as_array() {
                for x in a {
                    println!(
                        "{:>8} ms  {:<34} {}",
                        x["ms"].as_i64().unwrap_or(0),
                        x["kind"].as_str().unwrap_or(""),
                        x["detail"].as_str().unwrap_or("")
                    );
                }
            }
        }
        Cmd::Audit { project, limit } => {
            let project = resolve_project(&relay, &project).await?;
            let v = relay
                .get(&format!("/projects/{project}/audit?limit={limit}"))
                .await?;
            let entries = v.get("entries").cloned().unwrap_or(json!([]));
            if args.json {
                println!("{}", serde_json::to_string_pretty(&entries)?);
            } else if let Some(a) = entries.as_array() {
                for x in a {
                    let mut rest = x.clone();
                    if let Some(o) = rest.as_object_mut() {
                        o.remove("ts");
                        o.remove("kind");
                        o.remove("source");
                    }
                    println!(
                        "{:>14}  {:<20} {}",
                        x["ts"].as_i64().unwrap_or(0),
                        x["kind"].as_str().unwrap_or(""),
                        rest
                    );
                }
            }
        }
        Cmd::Admin { admin_token, cmd } => {
            let admin = Relay::with_token(&cfg, &admin_token);
            match cmd {
                AdminCmd::Users => println!(
                    "{}",
                    serde_json::to_string_pretty(&admin.get("/admin/users").await?)?
                ),
                AdminCmd::Grant { user, usd, note } => {
                    let v = admin
                        .put(
                            &format!("/admin/users/{user}/credits"),
                            &serde_json::json!({ "grant_usd": usd, "note": note }),
                        )
                        .await?;
                    println!("{}", serde_json::to_string_pretty(&v)?);
                }
                AdminCmd::Cap {
                    user,
                    usd,
                    unlimited,
                } => {
                    let cap = if unlimited {
                        serde_json::Value::Null
                    } else {
                        serde_json::json!(usd)
                    };
                    let v = admin
                        .put(
                            &format!("/admin/users/{user}/credits"),
                            &serde_json::json!({ "daily_cap_usd": cap }),
                        )
                        .await?;
                    println!("{}", serde_json::to_string_pretty(&v)?);
                }
                AdminCmd::TestUser { handle, name } => {
                    let v = admin
                        .post(
                            "/admin/users/test",
                            &serde_json::json!({ "handle": handle, "name": name }),
                        )
                        .await?;
                    println!("{}", serde_json::to_string_pretty(&v)?);
                }
                AdminCmd::AddDevice { user, name } => {
                    let v = admin
                        .post(
                            &format!("/admin/users/{user}/devices"),
                            &serde_json::json!({ "name": name }),
                        )
                        .await?;
                    println!("{}", serde_json::to_string_pretty(&v)?);
                }
                AdminCmd::DeleteTestUser { user } => {
                    admin.delete(&format!("/admin/users/{user}")).await?;
                    println!("deleted {user}");
                }
                AdminCmd::UpgradeBox { user, force } => {
                    let v = admin
                        .post(
                            &format!("/admin/users/{user}/box/upgrade?force={force}"),
                            &serde_json::json!({}),
                        )
                        .await?;
                    println!("{}", serde_json::to_string_pretty(&v)?);
                }
                AdminCmd::RepairBox { user } => {
                    let v = admin
                        .post(
                            &format!("/admin/users/{user}/box/repair"),
                            &serde_json::json!({}),
                        )
                        .await?;
                    println!("{}", serde_json::to_string_pretty(&v)?);
                }
                AdminCmd::DeleteBox { user } => {
                    let v = admin.delete(&format!("/admin/users/{user}/box")).await?;
                    println!("{}", serde_json::to_string_pretty(&v)?);
                }
                AdminCmd::Calls => println!(
                    "{}",
                    serde_json::to_string_pretty(&admin.get("/admin/calls").await?)?
                ),
                AdminCmd::Matrix { cmd } => {
                    let v = match cmd {
                        MatrixCmd::Status { failed } => {
                            admin
                                .get(if failed {
                                    "/admin/matrix/status?failed=1"
                                } else {
                                    "/admin/matrix/status"
                                })
                                .await?
                        }
                        MatrixCmd::Backfill {
                            user,
                            chat,
                            project,
                            rooms_only,
                        } => {
                            admin
                                .post(
                                    "/admin/matrix/backfill",
                                    &serde_json::json!({
                                        "user_id": user, "chat_id": chat, "project_id": project, "rooms_only": rooms_only,
                                    }),
                                )
                                .await?
                        }
                        MatrixCmd::Shadow { chat, unread, last } => {
                            let mut q = Vec::new();
                            if last {
                                q.push("last=1".to_owned());
                            }
                            if let Some(c) = chat {
                                q.push(format!("chat={c}"));
                            }
                            if unread {
                                q.push("unread=1".to_owned());
                            }
                            let path = if q.is_empty() {
                                "/admin/matrix/shadow".to_owned()
                            } else {
                                format!("/admin/matrix/shadow?{}", q.join("&"))
                            };
                            admin.get(&path).await?
                        }
                        MatrixCmd::Reads { user, state } => {
                            let on = match state.as_str() {
                                "on" => true,
                                "off" => false,
                                other => anyhow::bail!("reads takes on or off, not {other:?}"),
                            };
                            admin
                                .put(
                                    &format!("/admin/matrix/reads/{user}"),
                                    &serde_json::json!({ "on": on }),
                                )
                                .await?
                        }
                        MatrixCmd::Retry => {
                            admin
                                .post("/admin/matrix/retry", &serde_json::json!({}))
                                .await?
                        }
                    };
                    println!("{}", serde_json::to_string_pretty(&v)?);
                }
                AdminCmd::Push {
                    action,
                    user,
                    device,
                    body,
                } => {
                    let v = match action.as_str() {
                        "status" => admin.get("/admin/push").await?,
                        _ => {
                            admin
                                .post(
                                    "/admin/push/test",
                                    &serde_json::json!({ "user_id": user, "device_id": device, "body": body }),
                                )
                                .await?
                        }
                    };
                    println!("{}", serde_json::to_string_pretty(&v)?);
                }
                AdminCmd::Killswitch { action, reason } => {
                    let v = match action.as_str() {
                        "status" => admin.get("/admin/killswitch").await?,
                        on => {
                            admin
                                .put(
                                    "/admin/killswitch",
                                    &serde_json::json!({ "on": on == "on", "reason": reason }),
                                )
                                .await?
                        }
                    };
                    println!("{}", serde_json::to_string_pretty(&v)?);
                }
                AdminCmd::Fleet { action, n } => {
                    let v = match action.as_str() {
                        "cap" => {
                            let n = n.ok_or_else(|| anyhow::anyhow!("cap needs a number"))?;
                            admin
                                .put("/admin/fleet", &serde_json::json!({ "max_boxes": n }))
                                .await?
                        }
                        _ => admin.get("/admin/fleet").await?,
                    };
                    println!("{}", serde_json::to_string_pretty(&v)?);
                }
                AdminCmd::Sites {
                    action,
                    user,
                    arg,
                    reason,
                    since,
                    max_sites,
                    max_bytes,
                    block,
                    unblock,
                } => {
                    let need_user = || {
                        user.clone()
                            .ok_or_else(|| anyhow::anyhow!("{action} needs a user id"))
                    };
                    let need_arg = |what: &str| {
                        arg.clone()
                            .ok_or_else(|| anyhow::anyhow!("{action} needs {what}"))
                    };
                    let v = match action.as_str() {
                        "takedown" => {
                            let (u, a) = (need_user()?, need_arg("an app name")?);
                            admin
                                .post(
                                    &format!("/admin/sites/{u}/{a}/takedown"),
                                    &serde_json::json!({ "reason": reason }),
                                )
                                .await?
                        }
                        "restore" => {
                            let (u, a) = (need_user()?, need_arg("an app name")?);
                            admin
                                .post(
                                    &format!("/admin/sites/{u}/{a}/restore"),
                                    &serde_json::json!({}),
                                )
                                .await?
                        }
                        "purge" => {
                            let (u, a) = (need_user()?, need_arg("an app name")?);
                            admin.delete(&format!("/admin/sites/{u}/{a}")).await?
                        }
                        "events" => {
                            admin
                                .get(&format!("/admin/sites/events?since={}", since.unwrap_or(0)))
                                .await?
                        }
                        "quota" => {
                            let u = need_user()?;
                            let blocked = if block {
                                Some(true)
                            } else if unblock {
                                Some(false)
                            } else {
                                None
                            };
                            admin
                                .put(
                                    &format!("/admin/sites/quota/{u}"),
                                    &serde_json::json!({ "max_sites": max_sites, "max_bytes": max_bytes, "publish_blocked": blocked }),
                                )
                                .await?
                        }
                        "move" => {
                            let (u, p) = (need_user()?, need_arg("the new prefix")?);
                            admin
                                .post(
                                    &format!("/admin/sites/move/{u}"),
                                    &serde_json::json!({ "prefix": p }),
                                )
                                .await?
                        }
                        _ => admin.get("/admin/sites").await?,
                    };
                    println!("{}", serde_json::to_string_pretty(&v)?);
                }
                AdminCmd::Waitlist { action, user } => {
                    let v = match action.as_str() {
                        "admit" => {
                            let u = user.ok_or_else(|| anyhow::anyhow!("admit needs a user id"))?;
                            admin
                                .post(
                                    &format!("/admin/waitlist/{u}/admit"),
                                    &serde_json::json!({}),
                                )
                                .await?
                        }
                        _ => {
                            let f = admin.get("/admin/fleet").await?;
                            serde_json::json!({ "capacity": f.get("capacity"), "waitlist": f.get("waitlist") })
                        }
                    };
                    println!("{}", serde_json::to_string_pretty(&v)?);
                }
            }
        }
        Cmd::Devices { cmd } => match cmd {
            DeviceCmd::List => println!(
                "{}",
                serde_json::to_string_pretty(&relay.get("/devices").await?)?
            ),
            DeviceCmd::Revoke { id } => {
                relay.delete(&format!("/devices/{id}")).await?;
                println!("revoked {id}");
            }
        },
        Cmd::Routines { cmd } => routines(&relay, cmd, args.json).await?,
        Cmd::Skills { cmd } => match cmd {
            SkillsCmd::List { q } => {
                let path = match q {
                    Some(q) => format!("/skills/search?q={}", urlencode(&q)),
                    None => "/skills".to_owned(),
                };
                let v = relay.get(&path).await?;
                if args.json {
                    println!("{}", serde_json::to_string_pretty(&v)?);
                } else if let Some(a) = v.as_array() {
                    for s in a {
                        println!(
                            "{:<28} v{:<3} {:<7} {:<28} {}",
                            s["slug"].as_str().unwrap_or(""),
                            s["version"].as_i64().unwrap_or(1),
                            if s["shared"].as_bool().unwrap_or(false) {
                                "shared"
                            } else {
                                "own"
                            },
                            s["name"].as_str().unwrap_or(""),
                            s["when_to_use"].as_str().unwrap_or("")
                        );
                    }
                }
            }
            SkillsCmd::Show { slug } => {
                let v = relay.get(&format!("/skills/{slug}")).await?;
                if args.json {
                    println!("{}", serde_json::to_string_pretty(&v)?);
                } else {
                    print!("{}", v["markdown"].as_str().unwrap_or(""));
                }
            }
            SkillsCmd::Add {
                file,
                slug,
                name,
                description,
                tags,
                when_to_use,
                body,
                shared,
                project,
            } => {
                let mut b = match file {
                    Some(f) => json!({ "markdown": std::fs::read_to_string(&f)? }),
                    None => json!({}),
                };
                let set = |b: &mut Value, k: &str, v: Option<String>| {
                    if let Some(v) = v {
                        b[k] = json!(v);
                    }
                };
                set(&mut b, "slug", slug);
                set(&mut b, "name", name);
                set(&mut b, "description", description);
                set(&mut b, "when_to_use", when_to_use);
                set(&mut b, "body", body);
                if let Some(t) = tags {
                    b["tags"] = json!(t
                        .split(',')
                        .map(|x| x.trim().to_owned())
                        .filter(|x| !x.is_empty())
                        .collect::<Vec<_>>());
                }
                if shared {
                    b["shared"] = json!(true);
                }
                if let Some(p) = project {
                    b["origin_project_id"] = json!(resolve_project(&relay, &p).await?);
                }
                let v = relay.post("/skills", &b).await?;
                if args.json {
                    println!("{}", serde_json::to_string_pretty(&v)?);
                } else {
                    println!(
                        "added skill {} ({}) v{}",
                        v["slug"].as_str().unwrap_or("?"),
                        v["id"].as_str().unwrap_or("?"),
                        v["version"].as_i64().unwrap_or(1)
                    );
                }
            }
            SkillsCmd::Delete { slug } => {
                relay.delete(&format!("/skills/{slug}")).await?;
                println!("deleted {slug}");
            }
        },
        Cmd::Channels { cmd } => match cmd {
            ChannelsCmd::List => {
                let v = relay.get("/channels").await?;
                if args.json {
                    println!("{}", serde_json::to_string_pretty(&v)?);
                } else {
                    if v["mock"].as_bool() == Some(true) {
                        println!("mock mode (ARBOS_CHANNELS_MOCK=1)");
                    }
                    for p in v["platforms"].as_array().into_iter().flatten() {
                        println!(
                            "{:<26} {:<11} {}",
                            p["label"].as_str().unwrap_or(""),
                            p["state"].as_str().unwrap_or(""),
                            p["detail"].as_str().unwrap_or("")
                        );
                    }
                    if let Some(code) = v["pairing"]["code"].as_str() {
                        let left = (v["pairing"]["expiresAt"].as_i64().unwrap_or(0)
                            - arbos_core::ids::now_ms())
                        .max(0)
                            / 1000;
                        println!("pairing code {code} ({left} s left) → send `/pair {code}` from the channel");
                    }
                    let bindings = v["bindings"].as_array().cloned().unwrap_or_default();
                    println!("{} bound chat(s)", bindings.len());
                    for b in bindings {
                        println!(
                            "  {} {:<9} {:<40} → project {} chat {}",
                            b["id"].as_str().unwrap_or(""),
                            b["platform"].as_str().unwrap_or(""),
                            b["label"].as_str().unwrap_or(""),
                            b["projectId"].as_str().unwrap_or(""),
                            b["chatId"].as_str().unwrap_or("")
                        );
                    }
                }
            }
            ChannelsCmd::Pair { project, chat } => {
                let pid = resolve_project(&relay, &project).await?;
                let v = relay
                    .post(
                        "/channels/pair/new",
                        &json!({ "projectId": pid, "chatId": chat }),
                    )
                    .await?;
                if args.json {
                    println!("{}", serde_json::to_string_pretty(&v)?);
                } else {
                    println!(
                        "{}  (valid 10 min) → send `/pair {}` from Slack, Telegram, WhatsApp, or a Beeper chat",
                        v["code"].as_str().unwrap_or(""),
                        v["code"].as_str().unwrap_or("")
                    );
                }
            }
            ChannelsCmd::Unbind { id } => {
                relay.delete(&format!("/channels/bindings/{id}")).await?;
                println!("unbound {id}");
            }
            ChannelsCmd::Inbound {
                text,
                platform,
                chat,
                user,
                sender,
            } => {
                let v = relay
                    .post(
                        "/channels/mock/inbound",
                        &json!({ "platform": platform, "externalChatId": chat, "externalUserId": user, "text": text, "senderName": sender }),
                    )
                    .await?;
                println!("{}", serde_json::to_string_pretty(&v)?);
            }
            ChannelsCmd::Outbox { limit } => {
                let v = relay.get("/channels/mock/outbox").await?;
                let items: Vec<Value> = v["items"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .take(limit)
                    .collect();
                if args.json {
                    println!("{}", serde_json::to_string_pretty(&items)?);
                } else {
                    for it in items {
                        println!(
                            "#{:<4} {:<9} {:<8} → {:<24} {}",
                            it["id"].as_i64().unwrap_or(0),
                            it["platform"].as_str().unwrap_or(""),
                            it["status"].as_str().unwrap_or(""),
                            it["externalChatId"].as_str().unwrap_or(""),
                            it["text"].as_str().unwrap_or("").replace('\n', " ⏎ ")
                        );
                    }
                }
            }
        },
        Cmd::Blob { cmd } => match cmd {
            BlobCmd::Get { project, key, out } => {
                let (mime, bytes) = relay.get_bytes(&format!("/blob/{project}/{key}")).await?;
                let out = out.unwrap_or_else(|| std::path::PathBuf::from(&key));
                std::fs::write(&out, &bytes)?;
                if args.json {
                    println!(
                        "{}",
                        json!({ "key": key, "mime": mime, "bytes": bytes.len(), "path": out })
                    );
                } else {
                    println!("{} ({mime}, {} bytes)", out.display(), bytes.len());
                }
            }
        },
        Cmd::Group { cmd } => groups(&mut cfg, &relay, cmd, args.json).await?,
    }
    Ok(())
}

fn print_group(v: &Value, as_json: bool) -> Result<()> {
    if as_json {
        println!("{}", serde_json::to_string_pretty(v)?);
        return Ok(());
    }
    println!(
        "{}  {}  kind={}",
        v["id"].as_str().unwrap_or(""),
        v["title"].as_str().unwrap_or(""),
        v["kind"].as_str().unwrap_or("")
    );
    for p in v["participants_detail"].as_array().into_iter().flatten() {
        println!(
            "  {} {:<20} {}  box={}",
            p["icon"].as_str().unwrap_or("·"),
            p["name"].as_str().unwrap_or(""),
            p["id"].as_str().unwrap_or(""),
            p["box"].as_str().unwrap_or("?")
        );
    }
    Ok(())
}

async fn groups(cfg: &mut Config, relay: &Relay, cmd: GroupCmd, as_json: bool) -> Result<()> {
    match cmd {
        GroupCmd::Create { host, with, title } => {
            let host = resolve_project(relay, &host).await?;
            let v = relay
                .post(
                    &format!("/projects/{host}/groups"),
                    &json!({ "title": title, "participants": with }),
                )
                .await?;
            print_group(&v, as_json)?;
        }
        GroupCmd::Show { host, chat } => {
            let host = resolve_project(relay, &host).await?;
            let v = relay
                .get(&format!("/projects/{host}/groups/{chat}"))
                .await?;
            print_group(&v, as_json)?;
        }
        GroupCmd::Add {
            host,
            chat,
            projects,
        } => {
            let host = resolve_project(relay, &host).await?;
            let v = relay
                .patch(
                    &format!("/projects/{host}/groups/{chat}"),
                    &json!({ "add": projects }),
                )
                .await?;
            print_group(&v, as_json)?;
        }
        GroupCmd::Remove {
            host,
            chat,
            projects,
        } => {
            let host = resolve_project(relay, &host).await?;
            let v = relay
                .patch(
                    &format!("/projects/{host}/groups/{chat}"),
                    &json!({ "remove": projects }),
                )
                .await?;
            print_group(&v, as_json)?;
        }
        GroupCmd::Send {
            host,
            chat,
            text,
            stream,
            wait,
        } => {
            let host = resolve_project(relay, &host).await?;
            room::send(cfg, &host, &chat, &text, None, None, None, stream, 600).await?;
            if stream && wait > 0 {
                // The other Projects answer after the host's own reply; keep the room open.
                room::stream(cfg, &host, &chat, false, wait).await?;
            }
        }
    }
    Ok(())
}

async fn routines(relay: &Relay, cmd: RoutineCmd, as_json: bool) -> Result<()> {
    match cmd {
        RoutineCmd::List { project } => {
            let v = relay.get(&format!("/projects/{project}/routines")).await?;
            if as_json {
                println!("{}", serde_json::to_string_pretty(&v)?);
            } else if let Some(a) = v.as_array() {
                if a.is_empty() {
                    println!("no routines");
                }
                for r in a {
                    println!(
                        "{}  {:<24} {:<10} {:<28} next={} last={}",
                        r["id"].as_str().unwrap_or(""),
                        r["name"].as_str().unwrap_or(""),
                        if r["enabled"].as_bool().unwrap_or(false) {
                            "enabled"
                        } else {
                            "paused"
                        },
                        trigger_label(&r["trigger"]),
                        fmt_ms(r["next_run_at"].as_i64()),
                        fmt_ms(r["last_run_at"].as_i64()),
                    );
                }
            }
        }
        RoutineCmd::Create {
            project,
            name,
            prompt,
            chat,
            cron,
            tz,
            webhook,
            event,
            paused,
        } => {
            let trigger = match (cron, webhook, event) {
                (Some(expr), false, None) => json!({ "kind": "cron", "expr": expr, "tz": tz }),
                (None, true, None) => json!({ "kind": "webhook" }),
                (None, false, Some(ev)) => json!({ "kind": "event", "event": ev }),
                _ => return Err(anyhow!("give exactly one of --cron, --webhook, --event")),
            };
            let v = relay
                .post(
                    &format!("/projects/{project}/routines"),
                    &json!({ "name": name, "prompt": prompt, "chatId": chat, "trigger": trigger, "enabled": !paused }),
                )
                .await?;
            if as_json {
                println!("{}", serde_json::to_string_pretty(&v)?);
            } else {
                println!(
                    "routine {} ({}) next run {}",
                    v["id"].as_str().unwrap_or("?"),
                    trigger_label(&v["trigger"]),
                    fmt_ms(v["next_run_at"].as_i64())
                );
                if let Some(u) = v["fire_url"].as_str() {
                    println!("fire with: curl -X POST '{u}'");
                }
            }
        }
        RoutineCmd::Run { project, routine } => {
            let v = relay
                .post(
                    &format!("/projects/{project}/routines/{routine}/run"),
                    &json!({}),
                )
                .await?;
            println!("run {} queued", v["id"].as_str().unwrap_or("?"));
        }
        RoutineCmd::Runs {
            project,
            routine,
            limit,
        } => {
            let v = relay
                .get(&format!(
                    "/projects/{project}/routines/{routine}/runs?limit={limit}"
                ))
                .await?;
            if as_json {
                println!("{}", serde_json::to_string_pretty(&v)?);
            } else if let Some(a) = v.as_array() {
                if a.is_empty() {
                    println!("no runs yet");
                }
                for r in a {
                    let dur = match (r["started_at"].as_i64(), r["finished_at"].as_i64()) {
                        (Some(s), Some(f)) => format!("{:>5.1}s", (f - s) as f64 / 1000.0),
                        _ => "     —".to_owned(),
                    };
                    println!(
                        "{}  {:<8} {} {}  msg={} {}",
                        fmt_ms(r["started_at"].as_i64()),
                        r["status"].as_str().unwrap_or(""),
                        dur,
                        r["id"].as_str().unwrap_or(""),
                        r["message_id"].as_str().unwrap_or("—"),
                        r["note"].as_str().unwrap_or("")
                    );
                }
            }
        }
        RoutineCmd::Pause { project, routine } => {
            set_enabled(relay, &project, &routine, false).await?;
            println!("paused {routine}");
        }
        RoutineCmd::Resume { project, routine } => {
            let v = set_enabled(relay, &project, &routine, true).await?;
            println!(
                "resumed {routine}; next run {}",
                fmt_ms(v["next_run_at"].as_i64())
            );
        }
        RoutineCmd::Delete { project, routine } => {
            relay
                .delete(&format!("/projects/{project}/routines/{routine}"))
                .await?;
            println!("deleted {routine}");
        }
    }
    Ok(())
}

async fn set_enabled(relay: &Relay, project: &str, routine: &str, enabled: bool) -> Result<Value> {
    relay
        .patch(
            &format!("/projects/{project}/routines/{routine}"),
            &json!({ "enabled": enabled }),
        )
        .await
}

fn trigger_label(t: &Value) -> String {
    match t["kind"].as_str() {
        Some("cron") => format!(
            "cron {} ({})",
            t["expr"].as_str().unwrap_or("?"),
            t["tz"].as_str().unwrap_or("UTC")
        ),
        Some("once") => format!(
            "once at {}",
            fmt_ms(t["at"].as_i64().or_else(|| t["at_ms"].as_i64()))
        ),
        Some("webhook") => "webhook".to_owned(),
        Some("event") => format!("event {}", t["event"].as_str().unwrap_or("?")),
        Some(other) => other.to_owned(),
        None => "—".to_owned(),
    }
}

fn fmt_ms(ms: Option<i64>) -> String {
    match ms.and_then(chrono::DateTime::from_timestamp_millis) {
        Some(t) => t.format("%Y-%m-%d %H:%M:%SZ").to_string(),
        None => "—".to_owned(),
    }
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

async fn resolve_project(relay: &Relay, id_or_name: &str) -> Result<String> {
    // People chats (DMs and groups) live under the `people` pseudo-project: `chat send people
    // <chat-id> …` writes into `people/<chat-id>` like a device does.
    if id_or_name == "people" {
        return Ok(id_or_name.to_owned());
    }
    let v = relay.get(&format!("/projects/{id_or_name}")).await?;
    v.pointer("/project/id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("no such project {id_or_name}"))
}

/// Upload `path` to the chat's blob slot (`people:<chat>` for people chats, the project id
/// otherwise) and return the attachment object the row carries (`{name, mime, kind, ref, bytes,
/// width, height}`), as the apps write it.
async fn upload_attachment(
    relay: &client::Relay,
    project: &str,
    chat: &str,
    path: &std::path::Path,
) -> Result<serde_json::Value> {
    use sha2::Digest;
    let bytes =
        std::fs::read(path).map_err(|e| anyhow::anyhow!("--attach {}: {e}", path.display()))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_owned());
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let (mime, kind) = match ext.as_str() {
        "png" => ("image/png", "image"),
        "jpg" | "jpeg" => ("image/jpeg", "image"),
        "gif" => ("image/gif", "gif"),
        "webp" => ("image/webp", "image"),
        "heic" => ("image/heic", "image"),
        "pdf" => ("application/pdf", "file"),
        "txt" | "md" => ("text/plain", "file"),
        "mp4" | "mov" => ("video/mp4", "video"),
        _ => ("application/octet-stream", "file"),
    };
    let key: String = sha2::Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let slot = if project == "people" {
        format!("people:{chat}")
    } else {
        project.to_owned()
    };
    let size = bytes.len();
    let (w, h) = png_size(&bytes).unzip();
    relay
        .put_bytes(&format!("/blob/{slot}/{key}"), mime, bytes)
        .await?;
    let mut a =
        serde_json::json!({ "name": name, "mime": mime, "kind": kind, "ref": key, "bytes": size });
    if let (Some(w), Some(h)) = (w, h) {
        a["width"] = serde_json::json!(w);
        a["height"] = serde_json::json!(h);
    }
    Ok(a)
}

/// Width and height of a PNG from its IHDR (other formats: unknown).
fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 24 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let w = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    Some((w, h))
}
