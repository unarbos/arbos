//! `arbos-hub`: the meeting point of the mesh. Kernels and workers connect
//! outbound and register a machine name; clients attach, claim, and
//! message by that name. Listens on loopback; a tunnel or reverse proxy
//! in front does TLS.

mod auth;
mod http;
mod hub;

use anyhow::{Context, Result, bail};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::TcpListener;

const USAGE: &str = "arbos-hub [--config FILE] [--bind HOST:PORT]\n  FILE (or ARBOS_HUB_CONFIG): TOML with bind, [[machine]] name/token|token_env, [[client]] name/token|token_env/role. Default ~/.config/arbos/hub-server.toml.\n  Routes: GET /healthz; GET /list (token); WS /register (machine token); WS /attach/<machine>[/<project>]; WS /claim/<machine>.";

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
    let bind = bind
        .filter(|b| !b.trim().is_empty())
        .or_else(|| Some(auth.bind.clone()).filter(|b| !b.trim().is_empty()))
        .unwrap_or_else(|| "127.0.0.1:7010".to_string());
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(serve(auth, bind))
}

async fn serve(auth: Arc<auth::Auth>, bind: String) -> Result<()> {
    let listener = TcpListener::bind(&bind)
        .await
        .with_context(|| format!("bind {bind}"))?;
    let hub = Arc::new(hub::Hub::new(auth.default_share()));
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
                eprintln!("hub: {peer}: {e:#}");
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
        eprintln!("hub: {peer}: refused {} (no or unknown token)", req.path);
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
            let _ = futures_util::SinkExt::close(&mut ws).await;
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
