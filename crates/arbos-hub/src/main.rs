//! `arbos-hub`: the meeting point of the mesh. Kernels and workers connect
//! outbound and register a machine name; clients attach, claim, and
//! message by that name. Listens on loopback; a tunnel or reverse proxy
//! in front does TLS.

/// One line on stderr, stamped with the moment it happened
/// (`2026-09-17T04:46:44Z hub: …`). Without the stamp the log could list a
/// kernel leaving and coming back but never say when, which is what made the
/// 2026-09-17 outage windows unmeasurable. Defined before the modules so they
/// can use it.
macro_rules! log {
    ($($arg:tt)*) => {
        eprintln!(
            "{} hub: {}",
            arbos_core::inbox::rfc3339(arbos_core::now_ms()),
            format_args!($($arg)*)
        )
    };
}

mod auth;
mod http;
mod hub;
mod push;

use anyhow::{Context, Result, bail};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::TcpListener;

const USAGE: &str = "arbos-hub [--config FILE] [--bind HOST:PORT]\n  FILE (or ARBOS_HUB_CONFIG): TOML with bind, [[machine]] name/token|token_env, [[client]] name/token|token_env/role. Default ~/.config/arbos/hub-server.toml.\n  Routes: GET /healthz; GET /list (token); GET /push, GET /push/test[/<token-tail>] (token); WS /register (machine token); WS /attach/<machine>[/<project>]; WS /claim/<machine>.";

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let mut config: Option<PathBuf> = std::env::var_os("ARBOS_HUB_CONFIG").map(PathBuf::from);
    let mut bind: Option<String> = std::env::var("ARBOS_HUB_BIND").ok();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--config" | "-c" => {
                config = Some(PathBuf::from(args.next().context("--config needs a file")?))
            }
            "--bind" => bind = Some(args.next().context("--bind needs host:port")?),
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            "--version" => {
                println!("arbos-hub {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            other => bail!("unknown argument {other}\n{USAGE}"),
        }
    }
    let config = config.unwrap_or_else(|| arbos_core::host_dir().join("hub-server.toml"));
    let auth = Arc::new(auth::Auth::load(&config)?);
    let config_dir = config
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| arbos_core::host_dir());
    let bind = bind
        .filter(|b| !b.trim().is_empty())
        .or_else(|| Some(auth.bind.clone()).filter(|b| !b.trim().is_empty()))
        .unwrap_or_else(|| "127.0.0.1:7010".to_string());
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(serve(auth, bind, config_dir))
}

async fn serve(auth: Arc<auth::Auth>, bind: String, config_dir: PathBuf) -> Result<()> {
    let listener = TcpListener::bind(&bind)
        .await
        .with_context(|| format!("bind {bind}"))?;
    let push =
        push::Push::new(auth.push.clone(), &config_dir).context("[push] in the hub config")?;
    if push.enabled() {
        println!(
            "arbos-hub: push to Apple enabled for topic {} ({} device(s) registered)",
            auth.push.topic,
            push.device_count()
        );
    } else {
        // Loud, and the hub serves anyway: a wrong key path must not take
        // the hub down for everyone. GET /push says the same.
        log!(
            "push disabled — {} ({} device(s) registered and waiting)",
            push.reason().unwrap_or("no key"),
            push.device_count()
        );
    }
    let hub = Arc::new(hub::Hub::new(auth.default_share(), push));
    println!(
        "arbos-hub {} listening on {} with {} identities of {} user(s); an unset project's store is {} by default",
        env!("CARGO_PKG_VERSION"),
        listener.local_addr()?,
        auth.len(),
        auth.user_count(),
        auth.default_share()
    );
    loop {
        let Ok((stream, peer)) = listener.accept().await else {
            continue;
        };
        let auth = Arc::clone(&auth);
        let hub = Arc::clone(&hub);
        tokio::spawn(async move {
            if let Err(e) = handle(auth, hub, stream, peer.to_string()).await {
                log!("{peer}: {e:#}");
            }
        });
    }
}

async fn handle(
    auth: Arc<auth::Auth>,
    hub: Arc<hub::Hub>,
    mut stream: tokio::net::TcpStream,
    peer: String,
) -> Result<()> {
    let req = http::read_request(&mut stream).await?;
    // cloudflared puts the visitor's address here; loopback is the tunnel.
    let peer = req
        .header("cf-connecting-ip")
        .map(str::to_string)
        .unwrap_or(peer);
    if req.method != "GET" {
        return http::respond(&mut stream, 405, "text/plain", "GET only\n").await;
    }
    if req.path == "/healthz" || req.path == "/" {
        return http::respond(&mut stream, 200, "text/plain", "ok\n").await;
    }
    let who = auth::token_from_request(&req.query, req.header("authorization"))
        .and_then(|t| auth.authenticate(&t));
    let Some(who) = who else {
        log!("{peer}: refused {} (no or unknown token)", req.path);
        if req.wants_websocket() {
            // Finish the upgrade so the peer reads a reason, then close.
            let mut ws = http::upgrade(stream, &req).await?;
            let _ = futures_util::SinkExt::send(
                &mut ws,
                tokio_tungstenite::tungstenite::Message::Text(
                    serde_json::to_string(&arbos_core::wire::Frame::Error {
                        agent: None,
                        detail: "auth failed: unknown token".into(),
                    })?
                    .into(),
                ),
            )
            .await;
            // The same wait every other refusal gets (#344, #417): a close in
            // the same instant reaches the peer through cloudflared as a bare
            // close with no reason — observed on 2026-09-17 for exactly this
            // path while every authenticated refusal arrived with its text.
            hub::refuse_close(&mut ws).await;
            return Ok(());
        }
        return http::respond(&mut stream, 401, "text/plain", "token required\n").await;
    };
    let parts: Vec<&str> = req.path.trim_matches('/').split('/').collect();
    match (parts.as_slice(), req.wants_websocket()) {
        (["list"], false) => {
            let body = serde_json::to_string_pretty(&serde_json::json!({
                "machines": hub.roster_for(Some((who.user(), who.role()))),
            }))?;
            http::respond(&mut stream, 200, "application/json", &body).await
        }
        // Push state, for the person adding the key and the phone loop:
        // enabled and why not, the devices (token tails), the last attempts.
        (["push"], false) => {
            let all = matches!(who.role(), "owner" | "admin");
            let body = serde_json::to_string_pretty(&hub.push.status(who.user(), all))?;
            http::respond(&mut stream, 200, "application/json", &body).await
        }
        // A test alert to the caller's devices (or the one whose token ends
        // in the tail): see a push arrive the day the key is set.
        (["push", "test"], false) | (["push", "test", _], false) => {
            let tail = parts.get(2).copied().unwrap_or("");
            let deliveries = hub.push.test(who.user(), tail).await;
            let body = serde_json::to_string_pretty(&serde_json::json!({
                "enabled": hub.push.enabled(),
                "reason": hub.push.reason(),
                "sent": deliveries.len(),
                "deliveries": deliveries.iter().map(|d| serde_json::json!({
                    "token": &d.token[d.token.len().saturating_sub(8)..],
                    "status": d.status,
                    "detail": d.detail.trim(),
                })).collect::<Vec<_>>(),
            }))?;
            let code = if hub.push.enabled()
                && deliveries.iter().all(|d| d.status == 200)
                && !deliveries.is_empty()
            {
                200
            } else if !hub.push.enabled() {
                503
            } else {
                502
            };
            http::respond(&mut stream, code, "application/json", &body).await
        }
        (["register"], true) => {
            let ws = http::upgrade(stream, &req).await?;
            hub::register(hub, ws, who, peer).await;
            Ok(())
        }
        (["attach", machine], true) => {
            let ws = http::upgrade(stream, &req).await?;
            hub::attach(hub, ws, who, machine, None).await;
            Ok(())
        }
        (["attach", machine, project], true) => {
            let ws = http::upgrade(stream, &req).await?;
            hub::attach(hub, ws, who, machine, Some(project)).await;
            Ok(())
        }
        (["claim", machine], true) => {
            let ws = http::upgrade(stream, &req).await?;
            hub::claim(hub, ws, who, machine).await;
            Ok(())
        }
        (_, true) => {
            http::respond(&mut stream, 404, "text/plain", "no such websocket route\n").await
        }
        (_, false) => http::respond(&mut stream, 404, "text/plain", "not found\n").await,
    }
}
