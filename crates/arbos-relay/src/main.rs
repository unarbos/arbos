mod admin;
mod agent_messages;
mod api;
mod api_error_layer;
#[cfg(test)]
mod api_fixture_capture;
mod apns;
mod auth;
mod avatar;
mod avatar_image;
mod blobs;
mod boxes;
mod browser;
mod calls;
mod channels;
mod connectors;
mod credits;
mod daytona;
mod db;
mod db_accounts;
mod db_agent_messages;
mod db_calls;
mod db_fleet;
mod db_inspect;
mod db_people;
mod db_push;
mod db_secrets;
mod db_voice;
mod debug_api;
mod diag;
mod export;
mod features;
mod feedback;
mod fleet;
mod goals;
mod groups;
mod image_sniff;
mod images;
mod inspect;
mod link_icons;
mod livekit;
mod local;
mod matrix;
mod migrate;
mod moderation;
mod notices;
mod notify;
mod outside;
mod people;
mod proxy;
mod push;
mod releases;
mod restart;
mod room_health;
mod rooms;
mod routines;
mod secrets;
mod settings_audit;
mod shell_grants;
mod signin;
mod sites;
mod skills;
mod smoke;
mod state;
mod unfurl;
mod upgrade;
mod upgrade_carry;
mod vault;
mod vaults;
mod voice;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use axum::routing::{delete, get, post, put};
use axum::Router;
use clap::{Parser, Subcommand};
use tokio::sync::{Mutex, RwLock};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::state::{AppState, AuthMode, Config};

#[derive(Parser, Debug)]
#[command(
    name = "arbos-relay",
    version,
    about = "Arbos relay: registry, pairing, sync rooms, blobs, model proxy, Daytona"
)]
struct Args {
    /// Listen address.
    #[arg(long, env = "ARBOS_RELAY_LISTEN", default_value = "127.0.0.1:7700")]
    listen: String,
    /// URL boxes and clients use to reach this relay.
    #[arg(long, env = "ARBOS_RELAY_PUBLIC_URL")]
    public_url: Option<String>,
    /// Data directory (SQLite + blobs + local boxes).
    #[arg(long, env = "ARBOS_RELAY_DATA", default_value = "./.arbos-local/relay")]
    data: PathBuf,
    /// Admin token for `POST /pair/new`; generated and printed when absent.
    #[arg(long, env = "ARBOS_RELAY_ADMIN_TOKEN")]
    admin_token: Option<String>,
    /// Daytona snapshot name for new boxes.
    #[arg(
        long,
        env = "ARBOS_DAYTONA_SNAPSHOT",
        default_value = "arbos-agent-0.1.11"
    )]
    daytona_snapshot: String,
    /// Inline Dockerfile (e.g. image/Dockerfile.dev-inline) built by Daytona when the snapshot does not exist.
    #[arg(long, env = "ARBOS_DAYTONA_DOCKERFILE")]
    daytona_dockerfile: Option<PathBuf>,
    /// Directory served to boxes at /bootstrap/{file} (arbos-agentd, arbos-pi-ext.tar).
    #[arg(long, env = "ARBOS_BOOTSTRAP_DIR")]
    bootstrap_dir: Option<PathBuf>,
    /// Path to the arbos-agentd binary for `process` boxes.
    #[arg(long, env = "ARBOS_AGENTD_BIN", default_value = "arbos-agentd")]
    agentd_bin: PathBuf,
    /// Docker image for `docker` boxes.
    #[arg(long, env = "ARBOS_LOCAL_BOX_IMAGE", default_value = "arbos-agent:dev")]
    local_box_image: String,
    /// OpenAI-compatible upstream (OpenRouter by default).
    #[arg(
        long,
        env = "ARBOS_OPENAI_BASE",
        default_value = "https://openrouter.ai/api/v1"
    )]
    openai_base: String,
    /// Upstream for image generation (needs `OPENAI_API_KEY`; OpenRouter has no images endpoint).
    #[arg(
        long,
        env = "ARBOS_OPENAI_IMAGES_BASE",
        default_value = "https://api.openai.com/v1"
    )]
    openai_images_base: String,
    /// Anthropic-compatible upstream; OpenRouter's `/api/v1` when only an OpenRouter key exists.
    #[arg(long, env = "ARBOS_ANTHROPIC_BASE")]
    anthropic_base: Option<String>,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Run the server (default).
    Serve,
    /// Print a one-time pairing code (writes to the same SQLite file).
    PairCode,
    /// D25 lifecycle check against the live Daytona API with timings (create → stop → start → archive → start → delete).
    DaytonaSmoke {
        /// Snapshot name; when absent, an inline Dockerfile (node:24 + pi) is built by Daytona.
        #[arg(long)]
        snapshot: Option<String>,
        /// Keep the sandbox instead of deleting it.
        #[arg(long)]
        keep: bool,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,loro_internal=warn,arbos_relay=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .with(diag::WarnRing)
        .init();
    let args = Args::parse();
    let db = db::Db::open(&args.data.join("relay.db"))?;

    if let Some(Cmd::PairCode) = args.cmd {
        let code = arbos_core::auth::pairing_code();
        db.new_pairing_code(&code, 10 * 60 * 1000)?;
        println!("{code}");
        return Ok(());
    }

    let openrouter_key = std::env::var("OPENROUTER_API_KEY")
        .ok()
        .filter(|s| !s.is_empty());
    let anthropic_key = std::env::var("ANTHROPIC_API_KEY")
        .ok()
        .filter(|s| !s.is_empty());
    let openai_key = std::env::var("OPENAI_API_KEY")
        .ok()
        .filter(|s| !s.is_empty());
    let daytona_key = std::env::var("DAYTONA_API_KEY")
        .ok()
        .filter(|s| !s.is_empty());
    let anthropic_base = args.anthropic_base.clone().unwrap_or_else(|| {
        if anthropic_key.is_some() {
            "https://api.anthropic.com".to_owned()
        } else {
            "https://openrouter.ai/api".to_owned()
        }
    });
    let admin_token = args.admin_token.clone().or_else(|| {
        let t = arbos_core::auth::mint(arbos_core::auth::TokenKind::Device).replace("adv_", "adm_");
        tracing::info!("no ARBOS_RELAY_ADMIN_TOKEN set; generated admin token: {t}");
        Some(t)
    });
    let public_url = env_nonempty("ARBOS_PUBLIC_URL")
        .or_else(|| args.public_url.clone())
        .unwrap_or_else(|| format!("http://{}", args.listen));
    // Both spellings work: the vault item uses ARBOS_GITHUB_CLIENT_*, the design ARBOS_GITHUB_OAUTH_CLIENT_*.
    let github_client_id = env_nonempty("ARBOS_GITHUB_OAUTH_CLIENT_ID")
        .or_else(|| env_nonempty("ARBOS_GITHUB_CLIENT_ID"));
    let github_client_secret = env_nonempty("ARBOS_GITHUB_OAUTH_CLIENT_SECRET")
        .or_else(|| env_nonempty("ARBOS_GITHUB_CLIENT_SECRET"));
    let auth_mode = match env_nonempty("ARBOS_AUTH_MODE").as_deref() {
        Some("github") => AuthMode::Github,
        Some("paircode") => AuthMode::Paircode,
        Some(other) => anyhow::bail!("ARBOS_AUTH_MODE must be github or paircode, not {other:?}"),
        None if github_client_id.is_some() => AuthMode::Github,
        None => AuthMode::Paircode,
    };
    if auth_mode == AuthMode::Github
        && (github_client_id.is_none() || github_client_secret.is_none())
    {
        anyhow::bail!("ARBOS_AUTH_MODE=github needs ARBOS_GITHUB_CLIENT_ID and ARBOS_GITHUB_CLIENT_SECRET (or the ARBOS_GITHUB_OAUTH_CLIENT_* names)");
    }

    let cfg = Config {
        public_url,
        data_dir: args.data.clone(),
        admin_token,
        openrouter_key,
        anthropic_key,
        openai_key,
        openai_base: args.openai_base.clone(),
        openai_direct_base: env_nonempty("ARBOS_OPENAI_DIRECT_BASE")
            .unwrap_or_else(|| "https://api.openai.com/v1".to_owned()),
        openai_route: state::OpenAiRoute::parse(
            &std::env::var("ARBOS_OPENAI_ROUTE").unwrap_or_default(),
        ),
        openai_images_base: args.openai_images_base.clone(),
        anthropic_base,
        daytona_snapshot: args.daytona_snapshot.clone(),
        daytona_dockerfile: args.daytona_dockerfile.clone(),
        bootstrap_dir: args.bootstrap_dir.clone(),
        agentd_bin: args.agentd_bin.clone(),
        local_box_image: args.local_box_image.clone(),
        box_idle_grace_ms: 30_000,
        default_provider: env_nonempty("ARBOS_DEFAULT_PROVIDER"),
        fleet: fleet::FleetCfg::from_env(),
        connectors_mock: std::env::var("ARBOS_CONNECTORS_MOCK").is_ok_and(|v| v == "1"),
        channels_mock: std::env::var("ARBOS_CHANNELS_MOCK").is_ok_and(|v| v == "1"),
        image_mock: std::env::var("ARBOS_IMAGE_MOCK").is_ok_and(|v| v == "1"),
        avatar_retry_backoff_ms: std::env::var("ARBOS_AVATAR_RETRY_MS")
            .ok()
            .map(|v| v.split(',').filter_map(|s| s.trim().parse().ok()).collect())
            .unwrap_or_else(|| vec![2_000, 5_000]),
        avatar_attempt_timeout: std::time::Duration::from_secs(
            std::env::var("ARBOS_AVATAR_ATTEMPT_TIMEOUT_S")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(40),
        ),
        avatar_total_timeout: std::time::Duration::from_secs(
            std::env::var("ARBOS_AVATAR_TOTAL_TIMEOUT_S")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(75),
        ),
        voice_mock: std::env::var("ARBOS_VOICE_MOCK").is_ok_and(|v| v == "1"),
        openai_audio_base: std::env::var("ARBOS_OPENAI_AUDIO_BASE")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "https://api.openai.com/v1".to_owned()),
        vault_mock: std::env::var("ARBOS_VAULT_MOCK").is_ok_and(|v| v == "1"),
        auth_mode,
        github_client_id,
        github_client_secret,
        auth_redirects: env_nonempty("ARBOS_AUTH_REDIRECTS")
            .map(|s| {
                s.split(',')
                    .map(|p| p.trim().to_owned())
                    .filter(|p| !p.is_empty())
                    .collect()
            })
            .unwrap_or_default(),
        default_grant_usd: env_f64("ARBOS_DEFAULT_GRANT_USD", 100.0),
        default_daily_cap_usd: env_f64("ARBOS_DEFAULT_DAILY_CAP_USD", 20.0),
        global_daily_cap_usd: env_f64("ARBOS_GLOBAL_DAILY_CAP_USD", 500.0),
        global_machine_daily_cap_usd: env_f64("ARBOS_GLOBAL_MACHINE_DAILY_CAP_USD", 400.0),
        always_on: match env_nonempty("ARBOS_ALWAYS_ON").as_deref() {
            Some("0") | Some("false") | Some("off") => false,
            Some(_) => true,
            None => auth_mode == AuthMode::Github,
        },
        machine_usd_per_hour: env_f64("ARBOS_MACHINE_USD_PER_HOUR", 0.332),
        machine_stopped_usd_per_day: env_f64("ARBOS_MACHINE_STOPPED_USD_PER_DAY", 0.013),
        avatar_slugs: avatar::slugs_from_env(),
        avatar_colors: avatar::colors_from_env(),
        site_url: env_nonempty("ARBOS_SITE_URL").unwrap_or_else(|| "https://arbos.life".to_owned()),
        multi_chat: matches!(
            std::env::var("ARBOS_MULTI_CHAT").as_deref(),
            Ok("1") | Ok("true") | Ok("yes") | Ok("on")
        ),
        releases_repo: env_nonempty("ARBOS_RELEASES_REPO")
            .unwrap_or_else(|| "unarbos/arbos".to_owned()),
    };
    let relay_key = Arc::new(connectors::crypto::RelayKey::load(&db)?);
    let jwt_key: [u8; 32] = match env_nonempty("ARBOS_JWT_KEY") {
        Some(k) => connectors::crypto::RelayKey::derive(&k).subkey("jwt"),
        None => relay_key.subkey("jwt"),
    };
    let daytona = daytona_key.map(|k| {
        daytona::Daytona::new(
            k,
            std::env::var("DAYTONA_API_URL").ok(),
            std::env::var("DAYTONA_ORG_ID").ok(),
        )
    });
    if let Some(Cmd::DaytonaSmoke { snapshot, keep }) = args.cmd {
        let dt = daytona.ok_or_else(|| anyhow::anyhow!("DAYTONA_API_KEY is required"))?;
        return smoke::run(&dt, snapshot, keep).await;
    }
    let data_dir_for_voice = args.data.clone();
    let state = AppState {
        db: db.clone(),
        cfg: Arc::new(cfg),
        http: reqwest::Client::builder()
            .pool_idle_timeout(std::time::Duration::from_secs(300))
            .build()?,
        daytona,
        rooms: Arc::new(RwLock::new(HashMap::new())),
        box_states: Arc::new(RwLock::new(HashMap::new())),
        local_children: Arc::new(Mutex::new(HashMap::new())),
        box_conns: Arc::new(Mutex::new(HashMap::new())),
        wake_started: Arc::new(Mutex::new(HashMap::new())),
        last_client_push: Arc::new(Mutex::new(HashMap::new())),
        decisions_cache: Arc::new(std::sync::Mutex::new(Default::default())),
        stale_bounces: Arc::new(Mutex::new(HashMap::new())),
        conn_seq: Arc::new(AtomicU64::new(0)),
        relay_key,
        local_conns: Arc::new(Mutex::new(HashMap::new())),
        jwt_key: Arc::new(jwt_key),
        download_cache: Arc::new(Mutex::new(None)),
        upgrading: Arc::new(Mutex::new(HashMap::new())),
        upgrade_refused: Arc::new(Mutex::new(HashMap::new())),
        refresh_grace: Arc::new(Mutex::new(HashMap::new())),
        push: Arc::new(push::Push::from_env(db.clone())),
        focus: Arc::new(Mutex::new(HashMap::new())),
        fallback_sent: Arc::new(Mutex::new(HashMap::new())),
        livekit: livekit::from_env(),
        presence_seen: Arc::new(Mutex::new(HashMap::new())),
        activity: Arc::new(Mutex::new(HashMap::new())),
        box_http_seen: Arc::new(Mutex::new(HashMap::new())),
        turn_running: Arc::new(Mutex::new(HashMap::new())),
        evicted: Arc::new(Mutex::new(HashMap::new())),
        fleet_pressure: Arc::new(Mutex::new(None)),
        voice: Arc::new(voice::cfg_from_env(&data_dir_for_voice)),
        voice_conns: Arc::new(Mutex::new(HashMap::new())),
        default_snapshot: Arc::new(std::sync::RwLock::new(String::new())),
        started_at: std::time::Instant::now(),
        restart: Arc::new(crate::state::RestartSignal::default()),
        voice_dispatched: Arc::new(Mutex::new(HashMap::new())),
        voice_worker: Arc::new(Mutex::new(None)),
        voice_inbox: crate::voice::VoiceInbox::default(),
        image_health: Arc::new(Mutex::new(None)),
        client_seen: Arc::new(std::sync::Mutex::new(HashMap::new())),
        matrix: matrix::Matrix::new(matrix::Config::from_env()?),
    };
    tracing::info!(auth_mode = auth_mode.as_str(), public_url = %state.cfg.public_url, "sign-in");
    people::rename_agents_that_shadow_people(&state);
    if state.cfg.connectors_mock {
        tracing::info!("ARBOS_CONNECTORS_MOCK=1: connectors return canned data; sends land in connector_outbox");
    }
    if state.cfg.channels_mock {
        tracing::info!("ARBOS_CHANNELS_MOCK=1: channel sends land in channel_outbox; POST /channels/mock/inbound injects messages");
    }
    if state.cfg.voice_mock {
        tracing::info!(
            "ARBOS_VOICE_MOCK=1: /proxy/v1/audio/* return a mock transcript and a 1 s sine WAV"
        );
    }

    if state.cfg.image_mock {
        tracing::info!("ARBOS_IMAGE_MOCK=1: /proxy/v1/images/generations draws placeholder PNGs");
    } else if state.cfg.openai_key.is_none() {
        tracing::info!("no OPENAI_API_KEY: image generation is unavailable (set ARBOS_IMAGE_MOCK=1 for placeholders)");
    }
    if !connectors::crypto::RelayKey::from_env() && auth_mode == AuthMode::Github {
        tracing::warn!("ARBOS_RELAY_KEY is not set: the key that protects secrets and connector tokens lives in the database next to the ciphertext. Set it in the environment (op run) for production.");
    }
    secrets::migrate_f8(&state)?;
    vault::seed_mock(&state)?;

    if auth_mode == AuthMode::Paircode && db.device_count()? == 0 {
        let code = arbos_core::auth::pairing_code();
        db.new_pairing_code(&code, 60 * 60 * 1000)?;
        tracing::info!("no devices paired yet; one-time pairing code (1 h): {code}");
    }

    db.local_devices_reset()?;
    state.push.spawn_worker();
    rooms::spawn_sweeper(state.clone()).await;
    boxes::spawn_wake_runner(state.clone()).await;
    routines::spawn_scheduler(state.clone()).await;
    channels::spawn_pollers(state.clone()).await;
    credits::spawn_meter(state.clone()).await;
    fleet::spawn_fleet_tasks(state.clone()).await;
    boxes::spawn_always_on(state.clone()).await;
    upgrade::spawn_history_sweeper(state.clone()).await;
    secrets::spawn_sweeper(state.clone()).await;
    outside::spawn_sweeper(state.clone()).await;
    calls::spawn_sweeper(state.clone()).await;
    images::spawn_health_probe(state.clone());
    matrix::worker::spawn(state.clone());
    matrix::shadow::spawn_nightly(state.clone());
    boxes::spawn_project_purge(state.clone()).await;

    let app = Router::new()
        .route("/healthz", get(api::healthz))
        .route("/pair/new", post(api::pair_new))
        .route("/pair", post(api::pair))
        .route("/auth/github/start", get(signin::github_start))
        .route("/auth/github/callback", get(signin::github_callback))
        .route("/auth/token", post(signin::token))
        .route("/auth/refresh", post(signin::refresh))
        .route("/auth/logout", post(signin::logout))
        .route(
            "/auth/devices",
            get(signin::devices_list).post(export::mint_reader),
        )
        .route("/chats/{id}/export", get(export::export_chat))
        .route("/chats/{id}/messages", post(export::send_message))
        .route("/export/blob/{chat}/{key}", get(export::signed_blob))
        .route("/auth/devices/{id}", delete(signin::device_revoke))
        .route("/me", get(signin::me).patch(signin::patch_me))
        .route(
            "/me/avatar",
            put(signin::put_avatar).delete(signin::delete_avatar),
        )
        .route("/avatars/{file}", get(signin::get_avatar))
        .route("/projects/{file}/avatar.jpg", get(images::project_avatar))
        .route("/me/pushes", get(people::me_pushes))
        .route("/me/unread", get(notify::me_unread))
        .route(
            "/me/notifications",
            get(notify::me_notifications).put(notify::put_me_notifications),
        )
        .route("/chats/{id}/calls", post(calls::start_people_call))
        .route("/chats/{id}/call", get(calls::people_chat_call))
        .route(
            "/projects/{project_id}/chats/{chat_id}/calls",
            post(calls::start_project_call),
        )
        .route(
            "/projects/{project_id}/chats/{chat_id}/call",
            get(calls::project_chat_call),
        )
        .route("/calls/{id}", get(calls::get_call))
        .route("/calls/{id}/join", post(calls::join_call))
        .route("/calls/{id}/leave", post(calls::leave_call))
        .route("/calls/{id}/speakers", post(calls::invite_speaker))
        .route(
            "/calls/{id}/speakers/{user_id}",
            axum::routing::delete(calls::revoke_speaker),
        )
        .route(
            "/devices/{id}/voip",
            post(calls::register_voip).delete(calls::unregister_voip),
        )
        .route("/livekit/webhook", post(calls::livekit_webhook))
        .route("/admin/calls", get(calls::admin_calls))
        .route("/admin/usage/{purpose}", get(admin::usage_by_purpose))
        .route("/admin/release", post(releases::admin_release))
        .route("/admin/snapshot/default", post(admin::set_default_snapshot))
        .route("/admin/restart", post(restart::admin_restart))
        // Chats on Matrix (D88): the relay is Synapse's appservice, push gateway and operator.
        .route(
            "/_matrix/app/v1/transactions/{txn}",
            put(matrix::appservice::transaction),
        )
        .route(
            "/_matrix/app/v1/users/{user}",
            get(matrix::appservice::user_query),
        )
        .route(
            "/_matrix/app/v1/rooms/{alias}",
            get(matrix::appservice::room_query),
        )
        .route("/_matrix/app/v1/ping", post(matrix::appservice::ping))
        .route(
            "/_matrix/push/v1/notify",
            post(matrix::appservice::push_notify),
        )
        .route("/me/matrix", get(matrix::servers::me_matrix))
        .route(
            "/_matrix/client/{*rest}",
            axum::routing::any(matrix::servers::proxy),
        )
        .route(
            "/servers",
            get(matrix::servers::list).post(matrix::servers::create),
        )
        .route("/servers/join", post(matrix::servers::join))
        .route("/servers/{id}/archive", put(matrix::servers::archive))
        .route("/servers/search", get(matrix::servers::search))
        .route(
            "/servers/{id}",
            get(matrix::servers::get)
                .patch(matrix::servers::patch)
                .delete(matrix::servers::delete),
        )
        .route(
            "/servers/{id}/channels",
            post(matrix::servers::channel_create),
        )
        .route(
            "/servers/{id}/channels/{cid}",
            axum::routing::patch(matrix::servers::channel_patch)
                .delete(matrix::servers::channel_delete),
        )
        .route(
            "/servers/{id}/channels/{cid}/archive",
            post(matrix::servers::channel_archive),
        )
        .route(
            "/servers/{id}/invite/reset",
            post(matrix::servers::invite_reset),
        )
        .route("/servers/{id}/members", post(matrix::servers::member_add))
        .route(
            "/servers/{id}/members/{uid}",
            put(matrix::servers::member_role).delete(matrix::servers::member_remove),
        )
        .route(
            "/servers/{id}/invites",
            get(matrix::servers::invites_list).post(matrix::servers::invites_create),
        )
        .route(
            "/servers/{id}/invites/{iid}",
            axum::routing::delete(matrix::servers::invites_revoke),
        )
        .route("/servers/{id}/leave", post(matrix::servers::leave))
        .route("/s/{code}", get(matrix::servers::landing))
        .route("/admin/matrix/status", get(matrix::appservice::status))
        .route("/admin/matrix/backfill", post(matrix::appservice::backfill))
        .route(
            "/admin/matrix/shadow",
            get(matrix::appservice::shadow_compare),
        )
        .route(
            "/admin/matrix/retry",
            post(matrix::appservice::retry_failed),
        )
        .route(
            "/admin/matrix/reads/{user}",
            put(matrix::appservice::set_reads),
        )
        .route(
            "/admin/matrix/pushers",
            post(matrix::appservice::backfill_pushers),
        )
        .route("/admin/installs", get(releases::admin_installs))
        .route("/releases/current", get(releases::current))
        .route("/box/voice", get(voice::ws_box_voice))
        .route("/voice/calls/{id}", get(voice::call_info))
        .route("/voice/calls/{id}/tool", post(voice::call_tool))
        .route("/voice/calls/{id}/events", post(voice::call_events))
        .route("/voice/calls/{id}/usage", post(voice::call_usage))
        .route(
            "/projects/{id}/voice",
            get(voice::get_agent_settings).put(voice::put_agent_settings),
        )
        .route(
            "/admin/users/{id}/voice",
            axum::routing::put(voice::put_user_cap),
        )
        .route("/voice/worker/hello", post(voice::worker_hello))
        .route("/voice/calls/{id}/inbox", get(voice::call_inbox))
        .route("/voices", get(voice::list_voices))
        .route("/voices/{id}/sample.mp3", get(voice::voice_sample))
        .route(
            "/devices/{id}/push",
            post(notify::register_push).delete(notify::unregister_push),
        )
        .route("/chats/{id}/notify", post(notify::notify_people_chat))
        // Outside agents (#2140): owner side …
        .route("/outside-agents", get(outside::list).post(outside::create))
        .route(
            "/outside-agents/{id}",
            get(outside::get).delete(outside::revoke),
        )
        .route("/outside-agents/{id}/rotate", post(outside::rotate))
        // … and the agent side (bearer arbk_ key).
        .route("/asp/me", get(outside::me))
        .route("/asp/inbox", get(outside::inbox))
        .route("/asp/events", post(outside::events))
        .route("/asp/socket", get(outside::socket))
        .route("/asp/connect/{id}", get(outside::connect_page))
        .route(
            "/projects/{project_id}/chats/{chat_id}/notify",
            post(notify::notify_project_chat),
        )
        .route("/users/search", get(people::users_search))
        .route("/users", get(people::users_directory))
        .route("/users/{id}", get(people::user_profile))
        .route("/avatar-colors", get(people::avatar_colors))
        .route("/avatar-leaves", get(people::avatar_leaves))
        .route(
            "/chats",
            get(people::chats_list).post(people::chats_create_group),
        )
        .route("/chats/dm", post(people::dm_create))
        .route("/chats/join", post(people::group_join))
        .route(
            "/chats/{id}",
            get(people::chat_get)
                .patch(people::chat_patch)
                .delete(people::chat_delete),
        )
        .route(
            "/chats/{id}/invites",
            get(people::group_invites_list).post(people::group_invites_create),
        )
        .route(
            "/chats/{id}/invites/{iid}",
            delete(people::group_invites_revoke),
        )
        .route("/g/{code}", get(people::group_invite_landing))
        .route("/chats/{id}/read", axum::routing::put(people::chat_read))
        .route(
            "/chats/{id}/settings",
            axum::routing::put(people::chat_settings),
        )
        .route("/chats/{id}/hide", post(people::chat_hide))
        .route("/chats/{id}/leave", post(people::chat_leave))
        .route("/chats/{id}/agents", post(people::agent_add))
        .route(
            "/chats/{id}/agents/{project_id}",
            delete(people::agent_remove),
        )
        .route(
            "/chats/{id}/agents/{project_id}/settings",
            axum::routing::put(people::agent_settings),
        )
        .route(
            "/chats/{id}/members",
            get(debug_api::chat_members).post(people::member_add),
        )
        .route(
            "/chats/{id}/members/{user_id}",
            put(people::member_patch).delete(people::member_remove),
        )
        .route(
            "/chats/{id}/members/{user_id}/unban",
            post(people::member_unban),
        )
        .route("/box/people/{id}", get(people::box_people_chat))
        .route(
            "/box/people/{id}/gate",
            post(room_health::post_gate_decision),
        )
        .route(
            "/admin/room-health/{id}",
            get(room_health::admin_room_health),
        )
        .route(
            "/projects/{id}/members",
            get(people::project_members_list).post(people::project_member_add),
        )
        .route(
            "/projects/{id}/members/{user_id}",
            delete(people::project_member_remove),
        )
        // Agent-to-agent messaging (docs/agent-to-agent-messaging.md): the box sends for its
        // agent, the target box answers; the owner edits "Who may message X" from the agent pane.
        .route("/projects/{id}/agent-messages", post(agent_messages::send))
        .route(
            "/projects/{id}/agent-messages/{mid}/reply",
            post(agent_messages::reply),
        )
        .route(
            "/projects/{id}/message-access",
            get(agent_messages::list_grants).put(agent_messages::put_grant),
        )
        .route(
            "/projects/{id}/message-access/{grantee}",
            delete(agent_messages::delete_grant),
        )
        .route(
            "/blocks",
            get(people::blocks_list).post(people::block_create),
        )
        .route("/blocks/{user_id}", delete(people::block_delete))
        .route(
            "/invites",
            get(people::invites_list).post(people::invite_create),
        )
        .route("/invites/{code}", delete(people::invite_revoke))
        .route("/i/{code}", get(people::invite_landing))
        .route("/download/mac", get(signin::download_mac))
        .route("/devices", get(api::list_devices))
        .route("/devices/{id}", delete(api::revoke_device))
        .route(
            "/projects",
            get(api::list_projects).post(api::create_project),
        )
        .route(
            "/projects/{id}",
            get(api::get_project)
                .patch(api::patch_project)
                .delete(api::delete_project),
        )
        .route(
            "/projects/{id}/chats/{chat}",
            axum::routing::patch(api::patch_chat).delete(api::delete_chat),
        )
        .route(
            "/projects/{id}/browser/stream",
            get(browser::browser_stream),
        )
        .route("/projects/{id}/history/{chat}", get(api::chat_history))
        .route("/projects/{id}/archive", put(api::archive_project))
        .route("/projects/{id}/status", get(api::project_status))
        .route("/projects/{id}/inspect", get(inspect::project_inspect))
        .route("/projects/{id}/jobs/{job_id}", get(inspect::project_job))
        // Read-only debugging for an account's own devices (debug_api.rs).
        .route("/me/devices", get(debug_api::me_devices))
        .route("/me/feedback", get(debug_api::me_feedback))
        .route("/me/relay-errors", get(debug_api::me_relay_errors))
        .route("/machine/debug", get(debug_api::machine_debug))
        .route("/chats/{id}/reads", get(debug_api::chat_reads))
        .route("/chats/{id}/health", get(room_health::chat_health))
        .route(
            "/projects/{id}/chats",
            get(api::list_chats).post(api::create_chat),
        )
        .route("/projects/{id}/groups", post(groups::create_group))
        .route(
            "/projects/{id}/groups/{chat}",
            get(groups::get_group).patch(groups::patch_group),
        )
        .route(
            "/projects/{id}/groups/{chat}/send",
            post(groups::group_send),
        )
        .route(
            "/projects/{id}/groups/{chat}/reply",
            post(groups::group_reply),
        )
        .route("/projects/{id}/wake", post(api::wake))
        .route("/projects/{id}/start", post(api::start))
        .route("/projects/{id}/stop", post(api::stop))
        .route("/projects/{id}/keepalive", post(api::keepalive))
        .route("/projects/{id}/workspace", post(api::set_workspace_state))
        // One box per user: the box speaks for every project of its user.
        .route("/machine", get(api::machine))
        .route("/machine/start", post(api::machine_start))
        .route("/machine/stop", post(api::machine_stop))
        .route("/machine/migrate", post(api::machine_migrate))
        .route("/machine/restart", post(api::machine_restart))
        .route("/machine/upgrade", post(upgrade::machine_upgrade))
        .route(
            "/admin/users/{id}/box/upgrade",
            post(upgrade::admin_upgrade),
        )
        .route("/admin/users/{id}/box/repair", post(upgrade::admin_repair))
        .route(
            "/admin/users/{id}/box",
            axum::routing::put(api::admin_set_user_box).delete(api::admin_delete_user_box),
        )
        .route("/admin/push", get(admin::push_status))
        .route("/admin/push/test", post(admin::push_test))
        .route("/admin/push/drain", post(admin::push_drain))
        .route(
            "/admin/fleet",
            get(fleet::admin_fleet).put(fleet::admin_fleet_update),
        )
        .route(
            "/admin/waitlist/{id}/admit",
            post(fleet::admin_waitlist_admit),
        )
        .route("/admin/users", get(admin::list_users))
        .route("/admin/users/test", post(admin::create_test_user))
        .route("/admin/users/{id}", delete(admin::delete_test_user))
        .route("/admin/users/{id}/devices", post(admin::add_device))
        .route(
            "/admin/users/{id}/credits",
            axum::routing::put(admin::set_credits),
        )
        .route(
            "/admin/projects/{id}/spend-cap",
            axum::routing::put(admin::set_project_cap),
        )
        .route(
            "/admin/killswitch",
            get(admin::get_kill_switch).put(admin::set_kill_switch),
        )
        .route("/box/hello", post(api::box_hello))
        .route("/box/projects", get(api::box_projects))
        .route("/box/keepalive", post(api::box_keepalive))
        .route("/projects/{id}/push", post(api::push))
        .route("/projects/{id}/pushes", get(api::take_pushes))
        .route("/projects/{id}/usage", get(api::usage))
        .route("/projects/{id}/timings", post(api::record_timing))
        .route("/projects/{id}/activity", post(api::set_activity))
        .route(
            "/projects/{id}/routines",
            get(routines::list).post(routines::create),
        )
        .route(
            "/projects/{id}/routines/{rid}",
            axum::routing::patch(routines::patch).delete(routines::delete),
        )
        .route("/projects/{id}/routines/{rid}/run", post(routines::run_now))
        .route("/projects/{id}/routines/{rid}/runs", get(routines::runs))
        .route(
            "/projects/{id}/routines/{rid}/runs/{run}",
            post(routines::report_run),
        )
        .route("/routines/{rid}/fire", post(routines::webhook_fire))
        .route("/goals", get(goals::list_all))
        .route("/projects/{id}/goals", get(goals::list).post(goals::create))
        .route(
            "/projects/{id}/goals/{gid}",
            get(goals::get).patch(goals::patch).delete(goals::delete),
        )
        .route("/projects/{id}/goals/{gid}/log", post(goals::append_log))
        .route("/skills", get(skills::list).post(skills::create))
        .route("/skills/search", get(skills::list))
        .route("/skills/index", get(skills::index))
        .route(
            "/skills/{id}",
            get(skills::get).put(skills::update).delete(skills::delete),
        )
        .route(
            "/projects/{id}/audit",
            post(api::record_audit).get(api::list_audit),
        )
        .route("/local/{device}", get(local::ws_local))
        .route("/local/devices", get(local::list))
        .route(
            "/local/devices/{device}",
            axum::routing::patch(local::patch).delete(local::delete),
        )
        .route("/projects/{id}/local/{device}/exec", post(local::exec))
        .route("/room/{project}/{chat}", get(rooms::ws_room))
        .route(
            "/room/{project}/{chat}/checkpoint",
            get(rooms::get_checkpoint).post(rooms::post_checkpoint),
        )
        .route("/room/{project}/{chat}/stats", get(rooms::room_stats))
        .route(
            "/room/{project}/{chat}/messages/{message_id}",
            delete(moderation::delete_message),
        )
        .route(
            "/room/{project}/{chat}/messages/{message_id}/preview",
            delete(moderation::remove_preview),
        )
        .route("/blob/{project}/{key}", get(blobs::get).put(blobs::put))
        .route("/link-icon", get(link_icons::get))
        .route("/feedback", post(feedback::submit))
        .route("/feedback/capture/{id}", post(feedback::device_capture))
        .route("/box/feedback/capture", post(feedback::box_capture_request))
        .route(
            "/box/feedback/capture/{id}",
            get(feedback::box_capture_status),
        )
        .route("/admin/feedback", get(feedback::admin_list))
        .route("/admin/feedback/{id}/image", get(feedback::admin_image))
        .route(
            "/admin/feedback/{id}/issue",
            post(feedback::admin_set_issue),
        )
        // Agent-hosted sites (#2060): box publish API, owner list/unpublish, admin takedown.
        .route("/sites", get(sites::list).delete(sites::unpublish_all))
        .route("/sites/{app}", delete(sites::unpublish))
        .route("/sites/{app}/begin", post(sites::begin))
        .route("/sites/{app}/blob/{sha}", put(sites::put_blob))
        .route("/sites/{app}/commit", post(sites::commit))
        .route("/admin/sites", get(sites::admin_list))
        .route("/admin/sites/events", get(sites::admin_events))
        .route("/admin/sites/move/{user}", post(sites::admin_move))
        .route("/admin/sites/quota/{user}", put(sites::admin_set_quota))
        .route("/admin/sites/{user}/{app}", delete(sites::admin_purge))
        .route(
            "/admin/sites/{user}/{app}/takedown",
            post(sites::admin_takedown),
        )
        .route(
            "/admin/sites/{user}/{app}/restore",
            post(sites::admin_restore),
        )
        .route("/bootstrap/{file}", get(blobs::bootstrap))
        .route("/proxy/v1/models", get(proxy::openai_models))
        .route("/proxy/v1/images/generations", post(images::generations))
        .route(
            "/projects/{id}/avatar/generate",
            post(images::avatar_generate),
        )
        .route(
            "/chats/{id}/avatar/generate",
            post(images::chat_avatar_generate),
        )
        .route(
            "/servers/{id}/avatar/generate",
            post(images::server_avatar_generate),
        )
        .route("/me/avatar/generate", post(images::me_avatar_generate))
        .route("/admin/images/health", get(images::admin_image_health))
        .route(
            "/proxy/v1/audio/transcriptions",
            post(proxy::audio_transcriptions),
        )
        .route("/proxy/v1/audio/speech", post(proxy::audio_speech))
        .route("/proxy/v1/{*tail}", post(proxy::openai))
        .route("/proxy/anthropic/{*tail}", post(proxy::anthropic))
        .route("/proxy/decisions", post(proxy::decisions))
        .route("/connectors", get(connectors::list))
        .route("/connectors/catalog", get(connectors::catalog))
        .route("/connectors/outbox", get(connectors::outbox))
        .route("/connectors/call", post(connectors::call))
        .route(
            "/connectors/oauth/callback",
            get(connectors::oauth_callback),
        )
        .route("/connectors/{provider}/connect", post(connectors::connect))
        .route("/connectors/{id}", delete(connectors::disconnect))
        .route("/channels", get(channels::overview))
        .route("/channels/pair/new", post(channels::pair_new))
        .route("/channels/bindings", get(channels::list_bindings))
        .route("/channels/bindings/{id}", delete(channels::unbind))
        .route("/channels/mock/inbound", post(channels::mock_inbound))
        .route("/channels/mock/outbox", get(channels::mock_outbox))
        .route("/channels/slack/events", post(channels::slack::events))
        .route(
            "/channels/telegram/webhook/{secret}",
            post(channels::telegram::webhook),
        )
        .route(
            "/channels/whatsapp/webhook",
            get(channels::whatsapp::verify).post(channels::whatsapp::webhook),
        )
        .route("/projects/{id}/channels/reply", post(channels::reply))
        .route(
            "/vault/items",
            get(vault::list_items).post(vault::create_item),
        )
        .route(
            "/vault/items/{id}",
            axum::routing::put(vault::update_item).delete(vault::delete_item),
        )
        .route("/vault/requests", get(vault::list_requests))
        .route("/vault/requests/{id}", get(vault::request_status))
        .route("/vault/requests/{id}/decide", post(vault::decide_request))
        .route("/vault/requests/{id}/values", post(vault::request_values))
        .route("/vault/request", post(vault::create_request))
        .route("/vault/mock/echo", axum::routing::any(vault::mock_echo))
        .route("/egress", post(vault::egress))
        // Secrets (D83): devices manage secrets, access and the kill switch; boxes read names
        // and ask; values move only through /egress and the browser-fill window.
        .route("/secrets", get(secrets::list).post(secrets::create))
        .route("/secrets/activity", get(secrets::list_activity))
        .route("/secrets/requests", get(secrets::list_requests))
        .route(
            "/secrets/requests/{id}/decide",
            post(secrets::decide_request),
        )
        .route(
            "/secrets/{id}",
            get(secrets::get)
                .patch(secrets::patch)
                .delete(secrets::delete),
        )
        .route("/secrets/{id}/value", put(secrets::replace_value))
        .route("/secrets/{id}/revoke-all", post(secrets::revoke_all))
        .route("/secrets/{id}/scrub", post(secrets::scrub))
        .route(
            "/secrets/{id}/access/{project_id}",
            put(secrets::put_access).delete(secrets::delete_access),
        )
        .route("/secrets/{id}/people", post(secrets::post_director))
        .route(
            "/secrets/{id}/people/{director_user_id}",
            put(secrets::put_director).delete(secrets::delete_director),
        )
        // #1041 names, kept one release for clients built before #1442.
        .route("/secrets/{id}/directors", post(secrets::post_director))
        .route(
            "/secrets/{id}/directors/{director_user_id}",
            put(secrets::put_director).delete(secrets::delete_director),
        )
        .route("/projects/{id}/secrets", get(secrets::project_secrets))
        .route("/me/secrets/settings", put(secrets::put_settings))
        .route("/box/secrets", get(secrets::box_list))
        .route("/box/secrets/env", get(secrets::box_env))
        .route(
            "/box/shell-grants",
            get(shell_grants::box_list).post(shell_grants::box_create),
        )
        .route(
            "/projects/{id}/shell-grants",
            get(shell_grants::list).post(shell_grants::create),
        )
        .route(
            "/projects/{id}/shell-grants/{grant}",
            delete(shell_grants::revoke),
        )
        .route("/box/secrets/{id}/scrub", get(secrets::box_scrub))
        .route("/box/secrets/request", post(secrets::box_request))
        .route("/box/secrets/from-message", post(secrets::box_from_message))
        .route(
            "/box/secrets/requests/{id}",
            get(secrets::box_request_status),
        )
        .route(
            "/box/secrets/requests/{id}/values",
            post(secrets::box_request_values),
        )
        .route("/vaults", post(vaults::connect))
        .route("/vaults/{id}", get(vaults::get).delete(vaults::disconnect))
        .route("/vaults/{id}/token", put(vaults::replace_token))
        .route("/vaults/{id}/refresh", post(vaults::refresh))
        .route("/vaults/{id}/access/{project_id}", put(vaults::put_access))
        .route(
            "/vaults/{id}/access/{project_id}/hosts",
            put(vaults::put_item_hosts),
        )
        .route("/vaults/{id}/items/env", put(vaults::put_item_env))
        .route("/admin/secrets", get(secrets::admin_status))
        .route("/admin/secrets/pause", post(secrets::admin_pause))
        .layer(axum::extract::DefaultBodyLimit::max(64 * 1024 * 1024))
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            api_error_layer::rewrite_plain_api_errors,
        ))
        .with_state(state.clone());
    let state_for_restart = state;

    let listener = tokio::net::TcpListener::bind(&args.listen).await?;
    tracing::info!(listen = %args.listen, data = %args.data.display(), "arbos-relay listening");
    restart::install_sigterm_handler(state_for_restart);
    axum::serve(listener, app).await?;
    Ok(())
}

fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

fn env_f64(name: &str, default: f64) -> f64 {
    env_nonempty(name)
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v >= 0.0)
        .unwrap_or(default)
}
