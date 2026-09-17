//! Contract tests for `turn`: every exit leaves the transcript ended, so a
//! failed turn is never replayed as unfinished on the next kernel start.
//!
//! Found by the QA loop's `prompt-no-key` scenario: a kernel with no API key
//! wrote the wake, returned an error, and left the transcript open. Every
//! restart then refired the wake, and the plan closed the user's node as
//! done with "(no reply)".

use arbos_core::{
    AgentId, EventKind, Layout, Place, Wake, bootstrap, load_transcript, needs_serve,
};
use arbos_engine::{
    BoxFuture, Grep, GrepHit, Hooks, Host, HostConfig, Registry, TurnControl, TurnOpts, turn,
};
use std::{path::PathBuf, sync::Arc};

struct NoGrep;

impl Grep for NoGrep {
    fn search(&self, _pattern: &str, _glob: Option<&str>) -> anyhow::Result<Vec<GrepHit>> {
        Ok(Vec::new())
    }
}

struct NoHooks;

impl Hooks for NoHooks {
    fn approve(
        &self,
        _agent: &AgentId,
        _tool: &str,
        _command: &str,
    ) -> BoxFuture<'static, anyhow::Result<bool>> {
        Box::pin(async { Ok(false) })
    }
}

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "arbos-engine-{name}-{}-{}",
        std::process::id(),
        arbos_core::now_ms()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[tokio::test]
async fn a_turn_without_an_api_key_still_ends_the_transcript() {
    let dir = tmp("nokey");
    let place = Place::new(&dir);
    let agent = bootstrap(&place).unwrap();
    let host = Host {
        config: HostConfig {
            api_key: None,
            api_key_env: Some("ARBOS_TEST_KEY_THAT_IS_NOT_SET".into()),
            ..HostConfig::default()
        },
        dir: dir.join("host"),
    };
    let id = agent.id.as_str().to_string();
    turn(TurnOpts {
        place: place.clone(),
        agent,
        wake: Wake::user(id.as_str(), "hello"),
        host,
        registry: Arc::new(Registry::builtin()),
        grep: Arc::new(NoGrep),
        hooks: Arc::new(NoHooks),
        control: TurnControl::new(),
    })
    .await
    .expect("a missing key is a failed turn, not an error out of turn()");

    let events = load_transcript(&Layout::new(&place, &id).transcript()).unwrap();
    let kinds: Vec<&str> = events
        .iter()
        .map(|e| match &e.kind {
            EventKind::Wake { .. } => "wake",
            EventKind::User { .. } => "user",
            EventKind::Notice { failed: true, .. } => "failed_notice",
            EventKind::TurnComplete { .. } => "turn_complete",
            _ => "other",
        })
        .collect();
    assert_eq!(
        kinds,
        ["wake", "user", "failed_notice", "turn_complete"],
        "transcript: {events:#?}"
    );
    assert!(
        !needs_serve(&place, &id),
        "an ended turn must not be replayed on the next kernel start"
    );
    let notice = events
        .iter()
        .find_map(|e| match &e.kind {
            EventKind::Notice { text, failed: true } => Some(text.clone()),
            _ => None,
        })
        .unwrap();
    assert!(
        notice.contains("ARBOS_TEST_KEY_THAT_IS_NOT_SET"),
        "the notice names the env var the config points at: {notice}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
