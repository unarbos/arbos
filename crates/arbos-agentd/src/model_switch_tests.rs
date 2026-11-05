//! A model change made while a session is live must reach the main session, not only the lanes
//! that resolve a model on every call.
//!
//! 2026-11-04, Forest on agentd 0.1.193: the project's talking model was switched to Opus 5.5,
//! the quick replies went out on Opus, and the main-session call fifteen seconds later still
//! went out on Haiku. `LiveModels::resolve` answers every per-call lane from the live slots, so
//! those lanes cannot miss a change. The main session is the exception: pi owns its model
//! between spawns, and `apply_models` gives it a change with one `set_model`, once. These tests
//! hold that one push back the way production loses it — the registry pass that carries it is
//! bounded and dropped when it overruns — and then assert the thing the user sees, which is the
//! model the next main-session call goes out on.

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use arbos_core::ids::ulid;

use crate::agent::Agent;
use crate::config::{BoxConfig, Config};
use crate::store::Store;
use crate::sync::SyncHub;

const PROJECT: &str = "01MODELSWITCHPROJECT000000";
const CHAT: &str = "01MODELSWITCHCHAT000000000";
/// What the box starts this agent on, and what pi is spawned with.
const BOX_DEFAULT_TALKING: &str = "vendor/haiku-5.5";
const BOX_DEFAULT_SUBAGENT: &str = "vendor/sonnet-5.5";
/// What the user picks mid-session.
const CHOSEN_TALKING: &str = "vendor/opus-5.5";

/// A pi that answers the RPC protocol and records, for every prompt, the model it would make
/// that call on. With `lose_one` the first `set_model` is swallowed unanswered, which is what
/// agentd leaves behind when the bounded registry pass carrying it is dropped mid-push. The log
/// path and the flag are written into the script rather than passed in the environment, so two
/// tests in one process cannot read each other's.
const STUB_PI: &str = r#"#!/usr/bin/env python3
import json, sys

argv = sys.argv[1:]
model = "unset"
for i, a in enumerate(argv):
    if a == "--model" and i + 1 < len(argv):
        model = argv[i + 1]

log = "@LOG@"
lose_one = "@LOSE_ONE@" == "1"

def note(what, m):
    with open(log, "a") as f:
        f.write("%s %s\n" % (what, m))

def reply(cmd, cid, data=None):
    sys.stdout.write(json.dumps({"type": "response", "id": cid, "command": cmd, "success": True, "data": data}) + "\n")
    sys.stdout.flush()

def emit(ev):
    sys.stdout.write(json.dumps(ev) + "\n")
    sys.stdout.flush()

note("spawn", model)
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    c = json.loads(line)
    kind, cid = c.get("type"), c.get("id")
    if kind == "set_model":
        if lose_one:
            lose_one = False
            note("set_model_lost", c["modelId"])
            continue
        model = c["modelId"]
        note("set_model", model)
        reply(kind, cid)
    elif kind == "prompt":
        note("prompt", model)
        reply(kind, cid)
        emit({"type": "agent_settled", "aborted": False})
    elif kind == "get_state":
        reply(kind, cid, {"isStreaming": False})
    else:
        reply(kind, cid)
"#;

struct Fixture {
    agent: Arc<Agent>,
    log: std::path::PathBuf,
    root: std::path::PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

impl Fixture {
    /// Every line the stub pi recorded, in order.
    fn pi_log(&self) -> Vec<String> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// The model each main-session call went out on.
    fn prompt_models(&self) -> Vec<String> {
        self.pi_log()
            .iter()
            .filter_map(|l| l.strip_prefix("prompt ").map(str::to_owned))
            .collect()
    }
}

fn boot(lose_first_set_model: bool) -> Fixture {
    use std::os::unix::fs::PermissionsExt;

    let root = std::env::temp_dir().join(format!("arbos-model-switch-{}", ulid()));
    std::fs::create_dir_all(&root).unwrap();
    let log = root.join("pi-calls.log");
    let pi_bin = root.join("stub-pi.py");
    std::fs::write(
        &pi_bin,
        STUB_PI
            .replace("@LOG@", &log.display().to_string())
            .replace("@LOSE_ONE@", if lose_first_set_model { "1" } else { "0" }),
    )
    .unwrap();
    std::fs::set_permissions(&pi_bin, std::fs::Permissions::from_mode(0o755)).unwrap();

    let bx = BoxConfig {
        user_id: Some("u-model-switch".into()),
        // Nothing answers here: the model catalog cannot be fetched, so `sanitize_models` keeps
        // any well-shaped id, as it does on a box that cannot reach the relay.
        relay_url: "http://127.0.0.1:1".into(),
        box_token: "box-token".into(),
        mcp_token: "mcp-token".into(),
        work_root: root.clone(),
        bind: "127.0.0.1".into(),
        port: 0,
        local_box: true,
        pi_bin: pi_bin.display().to_string(),
        prime_bin: "/nonexistent/prime-agent".into(),
        claude_bin: "/nonexistent/claude".into(),
        pi_ext_override: None,
        device_id: "box:model-switch".into(),
        default_talking_model: BOX_DEFAULT_TALKING.into(),
        default_subagent_model: BOX_DEFAULT_SUBAGENT.into(),
        legacy_project_id: None,
    };
    let cfg: Config = bx.workspace(&json!({ "id": PROJECT, "name": "Forest" }), 0);
    let cfg = Arc::new(cfg);
    std::fs::create_dir_all(cfg.state_dir.join("agentd")).unwrap();
    let store = Store::open(&cfg.state_dir.join("agentd").join("agentd.db")).unwrap();
    let (tx, _rx) = tokio::sync::mpsc::channel(64);
    let sync = SyncHub::new(cfg.clone(), store.clone(), tx);
    let agent = Agent::new(cfg, store, sync, Arc::new(tokio::sync::Semaphore::new(4)));
    Fixture { agent, log, root }
}

/// The registry view the 3 s poll returns once the user has picked a model.
fn registry_with_talking(model: &str) -> serde_json::Value {
    json!({
        "id": PROJECT,
        "name": "Forest",
        "models": {
            "talking_model": { "id": model, "effort": "off" },
            "subagent_model": { "id": BOX_DEFAULT_SUBAGENT, "effort": "medium" },
        },
    })
}

/// #2750 follow-up: a model switch while the session is live reaches the main session's call.
///
/// Red before the fix: the prompt goes out on `vendor/haiku-5.5`, the model the box started pi
/// on, because the single `set_model` that would have moved pi was dropped with the registry
/// pass and nothing retries it. The live slots hold the new model all the while, which is why
/// the quick-reply lane in the same window is right and this one is wrong.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_main_turn_runs_on_the_model_the_agent_was_switched_to() {
    let f = boot(true);
    let agent = f.agent.clone();
    agent.start_pi(CHAT).await.expect("pi starts");
    let host = agent.sync.open_room(CHAT).await.expect("room");

    // The poll brings the user's pick. Bounded exactly as `ensure_workspace` bounds it, and the
    // stub holds the `set_model` back, so the pass is dropped mid-push.
    let settled = crate::workspace::bounded_by(
        Duration::from_millis(2_000),
        "settings",
        PROJECT,
        agent.settings_from_relay(&registry_with_talking(CHOSEN_TALKING), &[]),
    )
    .await;
    assert!(
        settled.is_none(),
        "the registry pass must be the one that overran and was dropped"
    );
    assert!(
        f.pi_log()
            .iter()
            .any(|l| l == "set_model_lost vendor/opus-5.5"),
        "pi must have been sent the push and never answered it: {:?}",
        f.pi_log()
    );

    // Every per-call lane already has the new model: this is the state the probe saw.
    assert_eq!(
        agent.live_models().await.0,
        CHOSEN_TALKING,
        "the live slots hold the user's pick"
    );

    agent
        .run_main_turn(CHAT, &host, "how long will this take?")
        .await
        .expect("the turn runs");

    assert_eq!(
        f.prompt_models(),
        vec![CHOSEN_TALKING.to_owned()],
        "the main session's call must go out on the model the agent is set to, not on the one \
         pi was spawned with; full pi log: {:?}",
        f.pi_log()
    );
}

/// The turn boundary must not undo the retry fallback. A transient model failure moves the
/// session onto the subagent model on purpose and retries there; the reconcile has to leave
/// that alone, and pick the agent's model up again once the retry has handed it back.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_retry_fallback_keeps_the_model_it_chose() {
    let f = boot(false);
    let agent = f.agent.clone();
    agent.start_pi(CHAT).await.expect("pi starts");
    let host = agent.sync.open_room(CHAT).await.expect("room");

    agent
        .settings_from_relay(&registry_with_talking(CHOSEN_TALKING), &[])
        .await
        .expect("settings applied");

    assert!(
        agent.switch_talking_model(true).await,
        "the retry moves the session onto the fallback model"
    );
    agent
        .run_main_turn(CHAT, &host, "retry")
        .await
        .expect("the retry turn runs");
    assert_eq!(
        f.prompt_models(),
        vec![BOX_DEFAULT_SUBAGENT.to_owned()],
        "the retry runs on the fallback it chose: {:?}",
        f.pi_log()
    );

    assert!(
        agent.switch_talking_model(false).await,
        "and hands the session back afterwards"
    );
    agent
        .run_main_turn(CHAT, &host, "next")
        .await
        .expect("the next turn runs");
    assert_eq!(
        f.prompt_models(),
        vec![BOX_DEFAULT_SUBAGENT.to_owned(), CHOSEN_TALKING.to_owned()],
        "the turn after it is back on the agent's model: {:?}",
        f.pi_log()
    );
}
