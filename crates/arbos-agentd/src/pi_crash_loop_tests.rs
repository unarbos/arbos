//! Bug Y: the main pi child crash-loops, says nothing about why, and the chat swallows every
//! message sent into the outage.
//!
//! Jacob on Test5, round 8 (2026-11-05, agentd 0.1.193 `3595b9fe0`, project
//! `01M6KTFT1X6WVXB39DX50RGF8Z`, side chat `01M6ME9A7EGV5PJ7DZW43AHTN5`). The main child died in
//! bug V's turn-end race at 01:16:11 CR and the box never had a working agent again:
//!
//! | 01:17:29.919 → 03:55:45 | an `extension_loaded` audit line every 2.4 s, about 4,000 of them |
//! | — | no exit status, signal, OOM counter or line of stderr for any of them |
//! | 01:31 → 03:31 | 25 user messages, each acked and then given a row with no words in it |
//! | 03:55:45 | the 10-minute idle park took a dying child, and the box read as healthy and idle |
//! | 04:09:35 | round 9's first probe woke it; the 2.4 s loop resumed, `pi_ready` flickering true |
//!
//! **Where the 2.4 s comes from, and what it proves.** It is the supervisor's, not the child's:
//! a 2 s poll plus about 0.4 s to spawn a child and get its first `get_state` back. The loop
//! only logged a failure and slept its 5 s penalty when `respawn_pi` returned `Err`, which would
//! have made the period at least seven seconds — so `respawn_pi` returned `Ok` every time. The
//! box believed it had a working agent about four thousand times, which is why not one failure
//! was ever written down. The child's own lifetime was never measured at all: a death is dated
//! to the poll that noticed it, so "about two seconds" is an upper bound and nothing more.
//!
//! **What these tests stand in for.** A test box normally points `pi_bin` at nothing, so there
//! is no main session and no supervisor; `test_agent_with_pi` gives it a scripted child instead,
//! and `start_pi`'s own supervise loop is what runs here. The stub answers `get_state`, writes
//! the `extension_loaded` audit line the extension writes, and then exits with a code — the
//! shape round 9 measured, where the child gets through ready and dies after.
//!
//! **Why the backoff is scaled.** Six deaths cost just over two minutes of waiting at the
//! shipped 2 s → 60 s, and a test that cannot reach the sixth death cannot measure what the
//! fifth triggers. `pi-backoff-ms` moves the whole curve; the doubling, the ceiling and the
//! five-deaths rule are the same code at either scale, and
//! `the_shipped_backoff_is_two_seconds_doubling_to_a_minute` pins the shipped numbers.

use std::sync::Arc;
use std::time::{Duration, Instant};

use arbos_core::doc::{CommandView, UserMessageOpts};
use arbos_core::ids::{now_ms, ulid};
use serde_json::{json, Value};

use crate::agent::{Agent, PI_RESTARTING_LINE};
use crate::status::tests::{test_agent, test_agent_with_pi};
use crate::sync::SharedHost;

/// The first respawn wait the tests run on. The shipped value is 2,000.
const FAST_BACKOFF_MS: u64 = 40;

/// How long a test waits for something the fix is supposed to do. Generous against a loaded CI
/// box: every assertion below is about what happened, never about how fast.
const SETTLE: Duration = Duration::from_secs(60);

/// A scripted pi child, written to its own directory so the counter beside it is per test.
///
/// `die_for` starts answer `get_state`, write the extension's audit line and then exit with
/// code 9 after a short pause — long enough that the parent has its boot response, which is how
/// round 9's children behaved. Later starts serve prompts normally. `die_for = 0` dies forever.
struct Stub {
    dir: std::path::PathBuf,
    bin: String,
}

/// How long a dying child holds ready by default: long enough that the parent has read its boot
/// response, short enough that the tests are quick.
const STUB_LIFE_MS: u64 = 150;

/// What the scripted child does. Written beside the script as JSON, so every start reads the
/// same brief and a test does not have to count positional arguments.
#[derive(Clone, Copy)]
struct StubCfg {
    /// This many starts answer `get_state`, write the audit line and then exit 9.
    die_for: i64,
    /// Every start dies that way.
    forever: bool,
    /// The first start dies inside its prompt instead, after streaming its words: bug V's race.
    race: bool,
    /// How long a dying start holds ready before it goes.
    life_ms: u64,
    /// After this many prompts, the start refuses every `switch_session` with "already
    /// processing" — a child left believing a run is still streaming (bug Y, phase two). 0 never.
    wedge_after_prompts: i64,
    /// The wedge applies to every start, not only the first.
    wedge_every_start: bool,
    /// `get_state` answers `success: false`: a child that is up and cannot serve.
    bad_state: bool,
}

impl Default for StubCfg {
    fn default() -> Self {
        Self {
            die_for: 0,
            forever: false,
            race: false,
            life_ms: STUB_LIFE_MS,
            wedge_after_prompts: 0,
            wedge_every_start: false,
            bad_state: false,
        }
    }
}

fn stub(die_for: i64, forever: bool) -> Stub {
    stub_cfg(StubCfg {
        die_for,
        forever,
        ..StubCfg::default()
    })
}

/// `race` makes the first start die in the middle of its prompt, after it has streamed its
/// words and before it settles — bug V's turn-end race, which is what started bug Y.
/// `life_ms` is how long a dying child stays ready: past one supervisor tick, and the loop finds
/// it alive and idle, which is the window Test5's park caught a doomed child in.
fn stub_cfg(cfg: StubCfg) -> Stub {
    let dir = std::env::temp_dir().join(format!("arbos-pi-stub-{}", ulid()));
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("stub-pi.py");
    std::fs::write(
        &script,
        r#"import json, os, sys, time

here = os.path.dirname(os.path.abspath(__file__))
cfg = json.load(open(os.path.join(here, "cfg.json")))
die_for = cfg["die_for"]
forever = cfg["forever"]
race = cfg["race"]
life_s = cfg["life_ms"] / 1000.0

# Which start is this? The counter lives beside the script, so a respawn sees the last one.
counter = os.path.join(here, "starts")
try:
    n = int(open(counter).read().strip())
except Exception:
    n = 0
n += 1
open(counter, "w").write(str(n))

# Every start records the session file it was given, so a test can see the fresh one.
session = ""
for i, a in enumerate(sys.argv):
    if a == "--session" and i + 1 < len(sys.argv):
        session = sys.argv[i + 1]
with open(os.path.join(here, "sessions"), "a") as f:
    f.write(session + "\n")

def out(o):
    sys.stdout.write(json.dumps(o) + "\n")
    sys.stdout.flush()

def audit(kind):
    path = os.environ.get("ARBOS_AUDIT_PATH")
    if not path:
        return
    try:
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "a") as f:
            f.write(json.dumps({"kind": kind, "ts": int(time.time() * 1000)}) + "\n")
    except Exception:
        pass

# The first start of a race run serves its prompt and dies inside it instead of at boot.
mid_turn = race and n == 1
dying = (forever or n <= die_for) and not mid_turn
# A child that can wedge: after `wedge_after_prompts` prompts it believes a run is still
# streaming and refuses every session switch, exactly as a stale run makes pi refuse one.
can_wedge = cfg["wedge_after_prompts"] > 0 and (cfg["wedge_every_start"] or n == 1)
prompts_served = 0
wedged = False

for line in sys.stdin:
    try:
        c = json.loads(line)
    except ValueError:
        continue
    t = c.get("type")
    r = {"type": "response", "id": c.get("id"), "command": t, "success": True}
    if t == "get_state":
        if cfg["bad_state"]:
            r["success"] = False
            r["error"] = "session state unavailable"
        else:
            r["data"] = {"isStreaming": wedged}
    if t == "switch_session" and wedged:
        r["success"] = False
        r["error"] = "already processing a request"
    out(r)
    if t == "prompt" and can_wedge:
        prompts_served += 1
        if prompts_served >= cfg["wedge_after_prompts"]:
            wedged = True
    if t == "get_state":
        # The extension loads after the process can serve RPC; Test5's audit line is the only
        # trace four thousand of these children left.
        audit("extension_loaded")
        if dying:
            # Ready, then gone. The pause lets the parent read the boot response first, which
            # is the case round 9 pinned down: pi_ready flickers true between starts.
            time.sleep(life_s)
            sys.stderr.write("stub pi: exiting after extension load\n")
            sys.stderr.flush()
            sys.exit(9)
        continue
    if t != "prompt":
        continue
    out({"type": "agent_start"})
    text = c.get("message") or ""
    reply = "cubed: 1,8,27" if "cube" in text else "the agent is back and this is the answer"
    out({"type": "message_update",
         "assistantMessageEvent": {"type": "text_delta", "contentIndex": 0, "delta": reply}})
    if mid_turn:
        # Words in the chat, then gone before settling: the shape of r8race's act turn.
        sys.stderr.write("stub pi: exiting mid-turn\n")
        sys.stderr.flush()
        sys.exit(9)
    out({"type": "agent_settled", "aborted": False})
"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("cfg.json"),
        json!({
            "die_for": cfg.die_for,
            "forever": cfg.forever,
            "race": cfg.race,
            "life_ms": cfg.life_ms,
            "wedge_after_prompts": cfg.wedge_after_prompts,
            "wedge_every_start": cfg.wedge_every_start,
            "bad_state": cfg.bad_state,
        })
        .to_string(),
    )
    .unwrap();
    let launcher = dir.join("stub-pi");
    std::fs::write(
        &launcher,
        format!("#!/bin/sh\nexec python3 {} \"$@\"\n", script.display()),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    Stub {
        bin: launcher.display().to_string(),
        dir,
    }
}

impl Stub {
    fn starts(&self) -> i64 {
        std::fs::read_to_string(self.dir.join("starts"))
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0)
    }

    /// The session file each start was given, in order.
    fn sessions(&self) -> Vec<String> {
        std::fs::read_to_string(self.dir.join("sessions"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

/// Run the respawn backoff at [`FAST_BACKOFF_MS`] for this agent only. The knob file is in this
/// agent's own state dir, so tests running at once do not move each other's curve.
fn fast_backoff(agent: &Arc<Agent>) {
    std::fs::write(
        agent.cfg.state_dir.join(crate::config::PI_BACKOFF_MS_FILE),
        FAST_BACKOFF_MS.to_string(),
    )
    .unwrap();
    assert_eq!(agent.cfg.pi_backoff_start_ms(), FAST_BACKOFF_MS);
}

/// Audit lines of one kind, newest last. This is the durable copy of the events `/inspect`
/// serves; the warn ring that also holds them is 500 entries for the whole box, and Test5's room
/// warnings were overwriting it within minutes.
fn audit_lines(agent: &Arc<Agent>, kind: &str) -> Vec<Value> {
    std::fs::read_to_string(agent.cfg.audit_path())
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|v| v.get("kind").and_then(Value::as_str) == Some(kind))
        .collect()
}

async fn wait_until<F, Fut>(what: &str, mut done: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let start = Instant::now();
    while start.elapsed() < SETTLE {
        if done().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("{what} did not happen within {SETTLE:?}");
}

async fn send_cmd(host: &SharedHost, text: &str) -> (String, CommandView) {
    let msg_id = ulid();
    {
        let h = host.lock().await;
        h.doc
            .append_user_message(&msg_id, text, "dev-mac", &UserMessageOpts::default())
            .expect("user message");
    }
    (
        msg_id.clone(),
        CommandView {
            id: ulid(),
            kind: "send".into(),
            payload: json!({ "messageId": msg_id }),
            issued_by: "dev-mac".into(),
            issued_at: now_ms(),
            status: "pending".into(),
        },
    )
}

/// Every assistant row the apps draw in reply to `user_msg`; hidden rows are skipped by both
/// clients, so a retired row is not a row the user sees.
async fn visible_answers(host: &SharedHost, user_msg: &str) -> Vec<(String, String)> {
    let h = host.lock().await;
    h.doc
        .messages()
        .into_iter()
        .filter(|m| {
            m.role == "assistant"
                && m.rest.get("inReplyTo").and_then(Value::as_str) == Some(user_msg)
                && m.rest.get("hidden").and_then(Value::as_bool) != Some(true)
        })
        .map(|m| (m.id.clone(), m.text()))
        .collect()
}

/// Every error part the user can see, as `code: text`. Test5's 25 messages left one each.
///
/// Hidden rows are skipped, because neither client draws them: a failed attempt the box then
/// re-ran and answered is retired, and what matters is that the chat the user reads has the
/// answer in it and no "send it again" above it.
async fn error_parts(host: &SharedHost) -> Vec<String> {
    let h = host.lock().await;
    h.doc
        .messages()
        .into_iter()
        .filter(|m| m.rest.get("hidden").and_then(Value::as_bool) != Some(true))
        .flat_map(|m| m.parts.clone())
        .filter(|p| p.get("kind").and_then(Value::as_str) == Some("error"))
        .map(|p| {
            format!(
                "{}: {}",
                p.get("code").and_then(Value::as_str).unwrap_or(""),
                p.get("message").and_then(Value::as_str).unwrap_or("")
            )
        })
        .collect()
}

/// Success criterion 1. A child that dies after every one of its first six starts, and then
/// works: the box gets back on its own, says why each one went, and answers the message that
/// arrived while it was down.
#[tokio::test(flavor = "multi_thread")]
async fn a_child_that_dies_six_times_recovers_and_every_death_says_why() {
    let s = stub(6, false);
    let (agent, _root) = test_agent_with_pi(&s.bin).await;
    fast_backoff(&agent);
    let chat = "01TESTCHATPICRASHLOOP0001";
    // The box knows its chats before it starts pi (`workspace.rs` upserts them at boot), and the
    // chat row is where a session file is recorded.
    agent
        .store
        .upsert_chat(chat, "crash loop", false, now_ms())
        .unwrap();
    let host = agent.sync.open_room(chat).await.unwrap();

    // Start 1 boots, answers, and dies 150 ms later — Test5's chain begins with a child that
    // worked, so the supervisor is running when the loop starts.
    agent.start_pi(chat).await.expect("the first child boots");
    // The file the first child was handed, read from the child's own record of it.
    let first_session = s.sessions().first().cloned().unwrap_or_default();
    assert!(
        !first_session.is_empty(),
        "the first child must have been given a session file to die on"
    );

    // A message sent into the outage. Test5 acked 25 of these and answered none.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let (msg, cmd) = send_cmd(&host, "are you there?").await;
    let sent = Instant::now();
    agent
        .handle_command(chat, cmd)
        .await
        .expect("a box that is restarting has not failed this message");

    let a = agent.clone();
    wait_until("the agent came back", || {
        let a = a.clone();
        async move { a.pi_ready.load(std::sync::atomic::Ordering::SeqCst) && a.pi_ready_for_ms() > 300 }
    })
    .await;

    // 1. Every death says why, with the exit code the incident never recorded once.
    let exits = audit_lines(&agent, "pi_child_exit");
    assert!(
        exits.len() >= 6,
        "six deaths owe six exit events, got {}: {exits:?}",
        exits.len()
    );
    let coded = exits
        .iter()
        .filter(|e| e.get("code").and_then(Value::as_i64) == Some(9))
        .count();
    assert!(
        coded >= 6,
        "every death must carry the child's exit code; {coded} of {} did: {exits:?}",
        exits.len()
    );
    assert!(
        exits.iter().any(|e| e
            .get("tail")
            .and_then(Value::as_str)
            .unwrap_or("")
            .contains("exiting after extension load")),
        "and the child's own stderr, which is what would have named the cause: {exits:?}"
    );

    // 2. The fifth death moved the chat to a fresh session file, and kept the old one.
    let rotations = audit_lines(&agent, "session_rotated");
    assert_eq!(
        rotations.len(),
        1,
        "five deaths rotate the session once, not once per death: {rotations:?}"
    );
    assert!(
        std::path::Path::new(&first_session).exists(),
        "the session the child died on is evidence and is never deleted: {first_session}"
    );
    let sessions = s.sessions();
    assert!(
        sessions.last().is_some_and(|last| *last != first_session),
        "the child that finally worked started on the fresh file: {sessions:?}"
    );

    // 3. The message sent into the outage is answered, and nothing in the chat says it failed.
    wait_until("the message was answered", || async {
        visible_answers(&host, &msg)
            .await
            .iter()
            .any(|(_, t)| t.contains("the agent is back"))
    })
    .await;
    let answers = visible_answers(&host, &msg).await;
    assert!(
        answers.iter().any(|(_, t)| t.contains("the agent is back")),
        "the message owed an answer got one: {answers:?}"
    );
    let errors = error_parts(&host).await;
    assert!(
        errors.is_empty(),
        "and no turn_failed row, no errno, nothing contentless: {errors:?}"
    );
    tracing::info!(starts = s.starts(), took = ?sent.elapsed(), "recovered");
}

/// Success criterion 2. A child that never works: the box keeps trying, says it is degraded, and
/// gives each message one readable line instead of an empty row.
#[tokio::test(flavor = "multi_thread")]
async fn a_child_that_never_works_is_reported_degraded_and_every_message_is_answered_in_words() {
    let s = stub(0, true);
    let (agent, _root) = test_agent_with_pi(&s.bin).await;
    fast_backoff(&agent);
    let chat = "01TESTCHATPICRASHLOOP0002";
    // The box knows its chats before it starts pi (`workspace.rs` upserts them at boot), and the
    // chat row is where a session file is recorded.
    agent
        .store
        .upsert_chat(chat, "crash loop", false, now_ms())
        .unwrap();
    let host = agent.sync.open_room(chat).await.unwrap();
    agent.start_pi(chat).await.expect("the first child boots");

    // 1. Degraded, well inside the two minutes the spec allows.
    let started = Instant::now();
    let a = agent.clone();
    wait_until("the box reported itself degraded", || {
        let a = a.clone();
        async move { a.pi_degraded.load(std::sync::atomic::Ordering::SeqCst) }
    })
    .await;
    assert!(
        started.elapsed() < Duration::from_secs(120),
        "degraded has to be reported within two minutes, took {:?}",
        started.elapsed()
    );
    assert!(
        !agent.is_healthy(),
        "a degraded agent is not healthy, whatever pi_ready reads between starts"
    );

    // 2. Each message gets one readable line and no empty error row.
    let mut sent = Vec::new();
    for text in ["first", "second", "third"] {
        let (msg, cmd) = send_cmd(&host, text).await;
        agent
            .handle_command(chat, cmd)
            .await
            .expect("a box that cannot answer has still not failed the message");
        sent.push(msg);
    }
    for msg in &sent {
        let rows = visible_answers(&host, msg).await;
        let lines: Vec<&String> = rows.iter().map(|(_, t)| t).collect();
        assert_eq!(
            rows.len(),
            1,
            "one row per message while the agent is down, not none and not several: {lines:?}"
        );
        assert!(
            rows[0].1.contains("I'm restarting"),
            "and it is the readable line, not a blank row: {:?}",
            rows[0]
        );
    }
    let errors = error_parts(&host).await;
    assert!(
        errors.is_empty(),
        "no message may get a contentless error row while the agent is down: {errors:?}"
    );

    // 3. The supervisor never stops. Bug Y's box looked as though it had: what had really
    // happened was the idle park, and round 9's probe found it respawning again. So this
    // measures the thing itself — more children lost after the box gave up on them — rather
    // than a count inside a fixed window, which the capped backoff can straddle.
    let before = audit_lines(&agent, "pi_child_exit").len();
    let a = agent.clone();
    wait_until("the supervisor started another child", || {
        let a = a.clone();
        async move { audit_lines(&a, "pi_child_exit").len() > before }
    })
    .await;
    assert!(
        agent
            .pi_supervisor_tick_ms
            .load(std::sync::atomic::Ordering::SeqCst)
            > now_ms() as u64 - 10_000,
        "and the supervise loop must still be going round"
    );
    assert_eq!(
        agent.cfg.pi_backoff_max_ms(),
        FAST_BACKOFF_MS * crate::config::PI_BACKOFF_MAX_RATIO,
        "the wait is capped rather than growing without end"
    );
}

/// Success criterion 3 (as corrected). A child that dies two seconds after becoming ready is
/// never parked, so a crash loop can never look like an idle box that tidied itself away.
///
/// Test5's park at 03:55:45 CR caught a child inside one of its alive windows and stopped the
/// respawns for fourteen minutes, with `is_healthy` reading the parked agent as well.
#[tokio::test(flavor = "multi_thread")]
async fn a_child_that_dies_young_is_never_parked_and_the_box_says_it_is_degraded() {
    // Each child outlives a supervisor tick, so the loop does find it alive and idle. Without
    // that the park branch is never even reached and this test proves nothing about parking.
    let s = stub_cfg(StubCfg {
        forever: true,
        life_ms: 2_500,
        ..StubCfg::default()
    });
    let (agent, _root) = test_agent_with_pi(&s.bin).await;
    fast_backoff(&agent);
    let chat = "01TESTCHATPICRASHLOOP0003";
    agent
        .store
        .upsert_chat(chat, "crash loop", false, now_ms())
        .unwrap();
    let _host = agent.sync.open_room(chat).await.unwrap();
    agent.start_pi(chat).await.expect("the first child boots");

    // The box has been idle far longer than the park threshold, which is the condition Test5
    // was in: the last real turn was hours before.
    agent.pi_last_use_ms.store(
        (now_ms() as u64).saturating_sub(Agent::PI_PARK_IDLE_MS + 60_000),
        std::sync::atomic::Ordering::SeqCst,
    );

    let a = agent.clone();
    wait_until("the box reported itself degraded", || {
        let a = a.clone();
        async move { a.pi_degraded.load(std::sync::atomic::Ordering::SeqCst) }
    })
    .await;

    // Several more children, each briefly alive while the box is far past its idle threshold —
    // every one of them a chance for the old rule to park one, which is what makes this a test
    // of parking rather than of the backoff.
    let want = s.starts() + 3;
    let deadline = Instant::now() + SETTLE;
    while s.starts() < want {
        assert!(
            !agent.pi_parked.load(std::sync::atomic::Ordering::SeqCst),
            "a child that has never held ready for a minute must never be parked"
        );
        assert!(
            Instant::now() < deadline,
            "more children came and went while the box was idle did not happen within {SETTLE:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        !agent.is_healthy(),
        "and the box must not read as healthy while it is in a crash loop"
    );
}

/// Success criterion 4: the race that started it all, replayed on a box that then loses two
/// more children, and the correction still lands.
///
/// r8race on 2026-11-05: the act turn ended at 01:16:08.396, the correction arrived in the same
/// moment, the main child went with `pi main child exited mid-turn` at 01:16:11.383 — and the
/// box never had a working agent again. #2784 answers the first half of that: a message lost to
/// a child that dies at turn end is re-run on the replacement. What it cannot do is survive the
/// replacement dying too, which is the half this PR adds, so the replay gives it a child that
/// dies in the turn and two more that die after boot before one holds.
///
/// **What this test does and does not measure.** The correction landing, once, with no error row
/// in the chat, and the box still ready afterwards: those are measured. The spec's "`pi_ready`
/// is true five minutes later" is not — `now_ms()` is `chrono::Utc::now()` with no test seam, so
/// five minutes cannot be waited for or faked, and this asserts the thing that would make it
/// true instead: no child lost after the box recovered, and nothing degraded. The CSV is stood
/// in for by the corrected answer, because producing a real one runs the file-delivery path this
/// fix does not touch.
#[tokio::test(flavor = "multi_thread")]
async fn the_turn_end_race_does_not_start_a_crash_loop_and_the_correction_lands() {
    let s = stub_cfg(StubCfg {
        die_for: 3,
        race: true,
        ..StubCfg::default()
    });
    let (agent, _root) = test_agent_with_pi(&s.bin).await;
    fast_backoff(&agent);
    let chat = "01TESTCHATPICRASHLOOP0004";
    agent
        .store
        .upsert_chat(chat, "crash loop", false, now_ms())
        .unwrap();
    let host = agent.sync.open_room(chat).await.unwrap();
    agent.start_pi(chat).await.expect("the first child boots");

    // The act turn: it speaks, and its child goes before it settles.
    let (first, first_cmd) = send_cmd(&host, "make a CSV of n squared up to 5000").await;
    let _ = agent.handle_command(chat, first_cmd).await;
    let turn_ended = Instant::now();
    assert!(
        !visible_answers(&host, &first).await.is_empty(),
        "the act turn said its piece before the child went, as r8race's did"
    );

    // The correction, inside the race window r8race measured, and the quick question 0.4 s later.
    assert!(
        turn_ended.elapsed() < Duration::from_millis(200),
        "the correction has to land in the race window to be a replay of it: {:?}",
        turn_ended.elapsed()
    );
    let (correction, correction_cmd) = send_cmd(&host, "cube them, not square").await;
    let agent2 = agent.clone();
    let correcting = tokio::spawn(async move { agent2.handle_command(chat, correction_cmd).await });
    tokio::time::sleep(Duration::from_millis(400)).await;
    let (question, question_cmd) = send_cmd(&host, "how far along are you?").await;
    let _ = agent.handle_command(chat, question_cmd).await;
    correcting
        .await
        .unwrap()
        .expect("the correction is answered, not failed");

    // 1. The correction was applied, once.
    wait_until("the correction was answered", || async {
        visible_answers(&host, &correction)
            .await
            .iter()
            .any(|(_, t)| t.contains("cubed"))
    })
    .await;
    let answers = visible_answers(&host, &correction).await;
    let cubed: Vec<&(String, String)> = answers
        .iter()
        .filter(|(_, t)| t.contains("cubed"))
        .collect();
    assert_eq!(
        cubed.len(),
        1,
        "the corrected work is done once, not twice and not never: {answers:?}"
    );
    assert!(
        !answers.iter().any(|(_, t)| t.contains("squared")),
        "and it is the correction that ran, not the brief it replaced: {answers:?}"
    );
    assert!(
        !visible_answers(&host, &question).await.is_empty(),
        "the question sent 0.4 s later is answered too"
    );

    // 2. Nothing in the chat says anything failed.
    let errors = error_parts(&host).await;
    assert!(
        errors.is_empty(),
        "no turn_failed row and no errno anywhere in the chat: {errors:?}"
    );

    // 3. The box is well, and stays well: this is where r8race turned into a two-hour outage.
    let a = agent.clone();
    wait_until("the box has a ready child again", || {
        let a = a.clone();
        async move { a.pi_ready.load(std::sync::atomic::Ordering::SeqCst) }
    })
    .await;
    let deaths_so_far = audit_lines(&agent, "pi_child_exit").len();
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(
        audit_lines(&agent, "pi_child_exit").len(),
        deaths_so_far,
        "the child that recovered the box has to still be the child that is running"
    );
    assert!(
        agent.pi_ready.load(std::sync::atomic::Ordering::SeqCst),
        "and the agent is still ready, which is what the five-minute criterion is asking"
    );
    assert!(
        !agent.pi_degraded.load(std::sync::atomic::Ordering::SeqCst),
        "a box that recovered inside the race is not degraded"
    );
}

/// Success criterion 5: two minutes without a ready child on an awake box is unhealthy.
///
/// Read directly rather than waited for — two minutes of real sleep is not a test, and
/// `now_ms()` has no seam — so this sets the clock the predicate reads and asks it the question.
/// `is_healthy` is what the relay's existing restart path consults; before this change it only
/// looked at whether a turn was wedged, so Test5's box answered "well" for two and a half hours
/// with no agent at all.
#[tokio::test(flavor = "multi_thread")]
async fn a_box_with_no_ready_child_for_two_minutes_is_not_healthy() {
    use std::sync::atomic::Ordering;
    let (agent, _root) = test_agent().await;
    assert!(
        agent.is_healthy(),
        "a box nobody has asked anything is well"
    );

    // No child, and the clock on being without one has just started.
    agent.note_pi_not_ready();
    assert!(
        agent.is_healthy(),
        "a box that lost its child a moment ago is restarting, not broken"
    );

    let now = now_ms() as u64;
    agent
        .pi_unready_since_ms
        .store(now - Agent::PI_UNHEALTHY_AFTER_MS + 5_000, Ordering::SeqCst);
    assert!(
        agent.is_healthy(),
        "five seconds short of the threshold is still restarting"
    );
    agent
        .pi_unready_since_ms
        .store(now - Agent::PI_UNHEALTHY_AFTER_MS - 1, Ordering::SeqCst);
    assert!(
        !agent.is_healthy(),
        "past two minutes with no child, the box is unhealthy and the restart path may have it"
    );

    // A child that answers clears it, and a parked one is not a missing one.
    agent.note_pi_ready();
    assert!(agent.is_healthy(), "a ready child makes the box well again");
    agent.note_pi_not_ready();
    agent
        .pi_unready_since_ms
        .store(now - Agent::PI_UNHEALTHY_AFTER_MS - 1, Ordering::SeqCst);
    agent.pi_parked.store(true, Ordering::SeqCst);
    assert!(
        agent.is_healthy(),
        "a box parked to save memory has an agent; it just has not started it yet"
    );
    assert_eq!(
        Agent::PI_UNHEALTHY_AFTER_MS,
        120_000,
        "the threshold the relay's restart path inherits"
    );
}

/// Phase two, the burst: a job start and three messages inside two seconds, all answered.
///
/// **What the burst is and is not.** Round 9's 04:35 burst is the traffic that exposed the
/// wedge, and a job start is load-bearing in the mechanism — a job runs its turn in its own
/// chat, so it moves the main child's session, and every message after it needs a switch back.
/// But the state that made the switch fail was created earlier, by the 04:29 `route=act` turn
/// that ended with `pi_ready` false. This test pins "a job start plus three quick sends is
/// handled correctly", which is worth pinning on its own; it is not evidence that the burst
/// caused the outage, and the production fix does not depend on it having done so.
///
/// The job's turn is stood in for by a turn in a second chat, which is what a job's own chat is
/// from the main child's point of view: the thing that leaves `current_chat` pointing elsewhere.
/// Starting a real job would bring in the worker, its approval card and its delivery path, none
/// of which this fix touches.
#[tokio::test(flavor = "multi_thread")]
async fn a_job_start_and_three_quick_messages_are_all_answered_and_the_child_never_wedges() {
    // The first child serves its own chat, then the job's, and is then stuck believing a run is
    // streaming — so every switch back is refused, which is phase two exactly.
    let s = stub_cfg(StubCfg {
        wedge_after_prompts: 2,
        ..StubCfg::default()
    });
    let (agent, _root) = test_agent_with_pi(&s.bin).await;
    fast_backoff(&agent);
    let chat = "01TESTCHATPICRASHLOOP0005";
    let job_chat = "01TESTCHATPICRASHLOOP0006";
    for c in [chat, job_chat] {
        agent
            .store
            .upsert_chat(c, "burst", false, now_ms())
            .unwrap();
    }
    let host = agent.sync.open_room(chat).await.unwrap();
    let job_host = agent.sync.open_room(job_chat).await.unwrap();
    agent.start_pi(chat).await.expect("the first child boots");

    // A turn in the user's chat, so the child has served one prompt and the session is this chat.
    let (first, first_cmd) = send_cmd(&host, "what is the box doing?").await;
    agent
        .handle_command(chat, first_cmd)
        .await
        .expect("answered");

    // The job's own turn: the child serves it, and is stuck from here.
    let (job_msg, job_cmd) = send_cmd(&job_host, "start the nightly export").await;
    let _ = agent.handle_command(job_chat, job_cmd).await;

    // The burst: three sends inside two seconds, each needing the session switched back. The
    // window is on delivering them, not on answering them — a wedged box takes seconds to fail
    // a send, and timing the answers would fire this guard before the real assertions below and
    // hide what it was meant to expose.
    let burst_opened = Instant::now();
    let mut burst = Vec::new();
    for text in [
        "make it CSV, not JSON",
        "how long will that take?",
        "and what is the box's disk at?",
    ] {
        let (id, cmd) = send_cmd(&host, text).await;
        let a = agent.clone();
        burst.push((
            id,
            tokio::spawn(async move { a.handle_command(chat, cmd).await }),
        ));
    }
    assert!(
        burst_opened.elapsed() < Duration::from_secs(2),
        "the three sends have to be delivered inside two seconds to be the burst: {:?}",
        burst_opened.elapsed()
    );
    for (_, task) in &mut burst {
        let _ = task.await.unwrap();
    }

    // And the fourth, two minutes later in the round 9 timeline.
    let (fourth, fourth_cmd) = send_cmd(&host, "still there?").await;
    let _ = agent.handle_command(chat, fourth_cmd).await;

    // Every message is answered, including the ones the wedge swallowed.
    for (what, id) in [
        ("the first turn", &first),
        ("the job", &job_msg),
        ("the correction", &burst[0].0),
        ("the first question", &burst[1].0),
        ("the second question", &burst[2].0),
        ("the message two minutes later", &fourth),
    ] {
        let h = match what == "the job" {
            true => &job_host,
            false => &host,
        };
        let id = id.clone();
        wait_until(&format!("{what} was answered"), || {
            let (h, id) = (h.clone(), id.clone());
            async move {
                visible_answers(&h, &id)
                    .await
                    .iter()
                    .any(|(_, t)| !t.is_empty() && t != PI_RESTARTING_LINE)
            }
        })
        .await;
    }
    for (what, h) in [("the chat", &host), ("the job's chat", &job_host)] {
        let errors = error_parts(h).await;
        assert!(errors.is_empty(), "{what} carries no error row: {errors:?}");
    }

    // The invariant that actually matters: the box never settles into "sends fail and readiness
    // reads true". One more message proves it, because a wedged child failed every one of them.
    let (last, last_cmd) = send_cmd(&host, "one more").await;
    agent
        .handle_command(chat, last_cmd)
        .await
        .expect("a box that is not wedged answers the next message");
    assert!(
        !visible_answers(&host, &last).await.is_empty(),
        "and the answer is in the chat"
    );
    assert_eq!(
        agent
            .pi_send_failures
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "no run of failures is still open"
    );
    assert!(
        agent.is_healthy(),
        "a box that answered every message is well"
    );
}

/// Phase two, the health half: a child that is up and cannot serve is not a healthy box.
///
/// Two ways for a child to be up and useless, and today's `pi_ready || pi_parked` calls both of
/// them well. The first is the one that kept Test5 reporting health for two and a half hours:
/// `respawn_pi` read only the transport of the boot `get_state`, never the `success` in the
/// body, so a child that answered badly was recorded ready. The second is a run of sends that
/// all fail before the router — `route=none`, no tool call — which nothing counted at all.
#[tokio::test(flavor = "multi_thread")]
async fn a_child_that_cannot_serve_is_reported_degraded_and_never_reads_ready() {
    let s = stub_cfg(StubCfg {
        bad_state: true,
        ..StubCfg::default()
    });
    let (agent, _root) = test_agent_with_pi(&s.bin).await;
    fast_backoff(&agent);
    let chat = "01TESTCHATPICRASHLOOP0007";
    agent
        .store
        .upsert_chat(chat, "bad state", false, now_ms())
        .unwrap();
    let _host = agent.sync.open_room(chat).await.unwrap();
    // The boot has to fail on the body: the process is up and its pipes are fine.
    let boot = agent.start_pi(chat).await;
    assert!(
        boot.is_err(),
        "a child that answers get_state with success: false has not booted"
    );
    assert!(
        !agent.pi_ready.load(std::sync::atomic::Ordering::SeqCst),
        "and it is never recorded ready"
    );

    // The supervisor keeps trying, each attempt fails the same way, and the box says so.
    let a = agent.clone();
    wait_until("the box reported itself degraded", || {
        let a = a.clone();
        async move { a.pi_degraded.load(std::sync::atomic::Ordering::SeqCst) }
    })
    .await;
    assert!(
        !agent.is_healthy(),
        "a box with no child that can answer is not healthy"
    );
    let said = audit_lines(&agent, "agent_degraded");
    assert!(
        said.iter()
            .any(|v| v.get("state").and_then(Value::as_str) == Some("degraded")),
        "and it is written down durably, not only in a warn ring: {said:?}"
    );

    // The second way in: a run of sends that fail before the router. Driven directly, because
    // three real failing turns measure the turn machinery rather than the counting.
    let (clean, _root2) = test_agent().await;
    assert!(
        clean.is_healthy(),
        "a box nobody has asked anything is well"
    );
    for i in 1..Agent::PI_SEND_FAILURES_DEGRADED {
        clean
            .note_send_outcome(chat, &Err(anyhow::anyhow!("switch_session: refused")))
            .await;
        assert!(
            clean.is_healthy(),
            "{i} failures is a bad turn, not a broken box"
        );
    }
    clean
        .note_send_outcome(chat, &Err(anyhow::anyhow!("switch_session: refused")))
        .await;
    assert!(
        !clean.is_healthy(),
        "{} sends in a row that failed before the router is a box that cannot answer",
        Agent::PI_SEND_FAILURES_DEGRADED
    );
    let failed = audit_lines(&clean, "pi_send_failed");
    assert_eq!(
        failed.len() as u32,
        Agent::PI_SEND_FAILURES_DEGRADED,
        "every one of them is written down, with its error text"
    );
    assert!(
        failed
            .iter()
            .all(|v| v.get("error").and_then(Value::as_str) == Some("switch_session: refused")),
        "and the text says what failed, so one occurrence is diagnosable: {failed:?}"
    );

    // One send that works ends the run: a bad turn must not accumulate towards degraded forever.
    clean.note_send_outcome(chat, &Ok(())).await;
    assert_eq!(
        clean
            .pi_send_failures
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "the count is a run of failures, not a lifetime total"
    );
}

/// The shipped curve, which the scaled tests above do not measure: 2 s, doubling, capped at a
/// minute. Six deaths are just over two minutes of waiting, which is why they are scaled.
#[test]
fn the_shipped_backoff_is_two_seconds_doubling_to_a_minute() {
    assert_eq!(crate::config::PI_BACKOFF_START_MS_DEFAULT, 2_000);
    assert_eq!(
        crate::config::PI_BACKOFF_START_MS_DEFAULT * crate::config::PI_BACKOFF_MAX_RATIO,
        60_000
    );
    let (start, max) = (
        crate::config::PI_BACKOFF_START_MS_DEFAULT,
        crate::config::PI_BACKOFF_START_MS_DEFAULT * crate::config::PI_BACKOFF_MAX_RATIO,
    );
    let mut held = 0u64;
    let waits: Vec<u64> = (0..8)
        .map(|_| {
            held = match held {
                0 => start,
                h => h.saturating_mul(2).min(max),
            };
            held
        })
        .collect();
    assert_eq!(
        waits,
        vec![2_000, 4_000, 8_000, 16_000, 32_000, 60_000, 60_000, 60_000],
        "2 s doubling to a 60 s cap"
    );
    assert_eq!(
        Agent::PI_CRASH_LOOP_DEATHS,
        5,
        "the fifth death is what clears the workspace and rotates the session"
    );
}

/// A message sent into an outage and promised an answer, with a working child on its way, and
/// `take_the_room` run at the one moment that makes the test about the room rather than about
/// timing.
///
/// That moment needs the owed list's own lock. A message is owed when the re-run after the
/// respawn loses a child too, which in a crash loop happens a breath before a child finally
/// survives — so a close made after this function returned would be racing the recovery, and
/// about one run in twenty the recovery won and answered the message by the ordinary route.
/// The drain's first act is to look at the owed list, so holding it across the close is what
/// makes the room already gone by the time the drain can start, every time.
async fn a_send_owed_into_an_outage<F, Fut>(
    chat: &str,
    take_the_room: F,
) -> (Arc<Agent>, Stub, crate::status::tests::TestRoot, String)
where
    F: FnOnce(Arc<Agent>) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let s = stub(6, false);
    let (agent, root) = test_agent_with_pi(&s.bin).await;
    fast_backoff(&agent);
    agent
        .store
        .upsert_chat(chat, "owed sends", false, now_ms())
        .unwrap();
    let host = agent.sync.open_room(chat).await.unwrap();
    agent.start_pi(chat).await.expect("the first child boots");
    tokio::time::sleep(Duration::from_millis(200)).await;
    let (msg, cmd) = send_cmd(&host, "are you there?").await;
    agent
        .handle_command(chat, cmd)
        .await
        .expect("a box that is restarting has not failed this message");
    agent.sync.persist(&host).await.unwrap();
    drop(host);

    let owed = agent.pi_owed_sends.lock().await;
    assert_eq!(
        owed.len(),
        1,
        "premise: the message was promised an answer rather than served"
    );
    take_the_room(agent.clone()).await;
    drop(owed);
    (agent, s, root, msg)
}

/// Wait for the drain to have finished, whichever way it went.
///
/// Two conditions, and both are needed. The emptied owed list says a drain got far enough to
/// commit: `mem::take` is the only thing that empties it, and it happens before the drain decides
/// anything — so a drain that drops the message reaches this too, which is what lets a test assert
/// on the outcome at once instead of waiting out the whole bound for a row a silent drop never
/// writes. But *only* that is the gate this helper used to be, and it is reached a few `await`
/// points before the record is appended. A test that read the audit file there raced the drain's
/// own write and saw nothing about one run in twenty, on an idle machine.
///
/// `pi_draining` closes it. It is raised before the drain is spawned and lowered only after
/// `drain_owed_sends` has returned, so an empty list with the flag down means the drain that
/// emptied it has finished writing. The pair cannot pass early: the list can only be emptied by a
/// drain, and no second drain can run while that one holds the flag.
async fn the_owed_send_drain_ran(agent: &Arc<Agent>) {
    let a = agent.clone();
    wait_until("the owed-send drain ran", || {
        let a = a.clone();
        async move {
            a.pi_owed_sends.lock().await.is_empty()
                && !a.pi_draining.load(std::sync::atomic::Ordering::SeqCst)
        }
    })
    .await;
}

/// The precondition for #2395's idle close: an owed send must never be dropped silently.
///
/// A chat goes quiet precisely because the box is down, so the chat with a message owed in it is
/// the likeliest in the box to meet #2367's idle close. The drain resolved its room with
/// `sync.host`, which a closed room answers `None` — and the owed list is emptied by `mem::take`
/// before that decision, so the skip consumed the promise and lost the message, with no answer
/// and no record. That is the contentless row bug Y exists to prevent, arriving by a new route.
#[tokio::test(flavor = "multi_thread")]
async fn an_owed_send_is_answered_after_the_idle_close_took_its_room() {
    let chat = "01TESTCHATOWEDCOLD000001";
    let (agent, _s, root, msg) = a_send_owed_into_an_outage(chat, |a| async move {
        a.sync.test_close_cold(chat).await;
        assert!(a.sync.host(chat).await.is_none(), "premise: no handle");
        assert!(
            a.sync.may_open(chat).await,
            "premise: a cold drop records nothing against the chat"
        );
    })
    .await;

    the_owed_send_drain_ran(&agent).await;

    // The promise, not the route that keeps it. Two recovery paths can answer a message lost to
    // a dying child — this drain, and bug V's re-run — and which one gets there is a race the
    // user cannot see and this test has no business pinning. What must never happen is the one
    // the idle close caused: the entry consumed and nothing in the chat.
    let answered = |a: Arc<Agent>, msg: String| async move {
        let Some(host) = a.sync.host(chat).await else {
            return false;
        };
        visible_answers(&host, &msg)
            .await
            .iter()
            .any(|(_, t)| !t.is_empty() && t != PI_RESTARTING_LINE)
    };
    wait_until("the promised message was answered", || {
        answered(agent.clone(), msg.clone())
    })
    .await;
    assert!(
        audit_lines(&agent, "owed_send_undeliverable").is_empty(),
        "a cold room is reopened, not recorded as somewhere the message cannot go"
    );
    assert!(
        agent.sync.is_open(chat).await,
        "and the answer went to a room the relay can push from"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// The same guarantee where the reopen cannot help, which is what makes this test discriminating:
/// a chat the agent left for good is refused by `Agent::room`, so a fix that only reopens cold
/// rooms still consumes this promise in silence. An undeliverable owed send is recorded.
#[tokio::test(flavor = "multi_thread")]
async fn an_owed_send_that_can_never_be_delivered_still_leaves_a_record() {
    let chat = "01TESTCHATOWEDLEFT000001";
    let (agent, _s, root, _msg) = a_send_owed_into_an_outage(chat, |a| async move {
        a.sync.close_room(chat).await;
        assert!(
            !a.sync.may_open(chat).await,
            "premise: the chat is recorded as one this box left"
        );
    })
    .await;

    the_owed_send_drain_ran(&agent).await;

    let records = audit_lines(&agent, "owed_send_undeliverable");
    assert_eq!(
        records.len(),
        1,
        "an owed send that cannot be delivered is recorded, never just dropped: {records:?}"
    );
    assert_eq!(
        records[0].get("chat_id").and_then(Value::as_str),
        Some(chat),
        "and the record names the chat: {records:?}"
    );
    assert!(
        records[0]
            .get("reason")
            .and_then(Value::as_str)
            .is_some_and(|r| !r.is_empty()),
        "and says why, so a silence is never the only trace: {records:?}"
    );
    assert!(
        !agent.sync.is_open(chat).await,
        "and the chat the agent left stays left"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// The one place an owed send is dropped by design: the queue is bounded, and a chat someone
/// keeps typing into while the agent is down owes answers rather than an endless queue. The bound
/// is deliberate and stays; what it must not be is silent, because the user whose message is
/// evicted has already been told the agent would come back to them.
///
/// Pre-existing on main and nothing to do with #2367 — the idle close neither causes it nor
/// worsens it.
#[tokio::test(flavor = "multi_thread")]
async fn the_oldest_owed_send_is_recorded_when_the_queue_overflows() {
    let s = stub(0, true);
    let (agent, root) = test_agent_with_pi(&s.bin).await;
    fast_backoff(&agent);
    let chat = "01TESTCHATOWEDCAP0000001";
    agent
        .store
        .upsert_chat(chat, "owed cap", false, now_ms())
        .unwrap();
    let host = agent.sync.open_room(chat).await.unwrap();
    agent.start_pi(chat).await.expect("the first child boots");
    tokio::time::sleep(Duration::from_millis(200)).await;

    // One more than the bound, so exactly one eviction is owed.
    for i in 0..=Agent::PI_OWED_MAX {
        let (_, cmd) = send_cmd(&host, &format!("message {i}")).await;
        agent
            .handle_command(chat, cmd)
            .await
            .expect("a box that is restarting has not failed this message");
    }

    assert_eq!(
        agent.pi_owed_sends.lock().await.len(),
        Agent::PI_OWED_MAX,
        "premise: the queue holds its bound and no more"
    );
    let evicted = audit_lines(&agent, "owed_send_evicted");
    assert_eq!(
        evicted.len(),
        1,
        "the promise the bound dropped is recorded: {evicted:?}"
    );
    assert_eq!(
        evicted[0].get("chat_id").and_then(Value::as_str),
        Some(chat),
        "and names the chat whose oldest promise went: {evicted:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}
