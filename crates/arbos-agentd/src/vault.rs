//! Secrets tools (D83): `secrets_list`, `secrets_request`, `egress`, `browser_fill` (the F8 names
//! `vault_list` / `vault_request` stay as aliases for one release). The box never holds a
//! secret: the model works with placeholders (`arbos-secret://<NAME>/<field>`, also exposed to
//! child processes as `$<NAME>`), the relay swaps them in on `POST /egress` for the agent's
//! allowed hosts only, and logins are typed into agent-browser by agentd itself, with the values
//! held for a few seconds and registered for write-time redaction (D19). Every use is audited
//! (names, hosts, URLs; never values).
//!
//! Standing Access comes from the relay (`GET /box/secrets`) and is refreshed on the pushes
//! `secret_access_changed`, `secrets_paused`, `secret_value_replaced`, `secret_decided`,
//! `secret_scrub`, so a switch flipped in Settings reaches this process within one push poll
//! (5 s). Scrub asks the relay for the values once, then redacts them from chats, sessions,
//! T3 and memory.
//!
//! Real values (D83 decision 2 as decided 2026-10-18): a secret whose owner switched "Real value
//! in the agent's environment" on arrives from `GET /box/secrets/env` and is written to the
//! workspace's `secrets.env`, which every shell sources (`BASH_ENV` for `bash -c`, the image's
//! profile.d / bash.bashrc loader for login and interactive shells). The file is rewritten on
//! the same pushes, so a switch, Pause all, a delete or a disconnect takes the variable out of
//! the next command within the same 5 s; processes already running keep what they had. The
//! harness processes themselves never get a real value at spawn — only the file carries them.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::sync::oneshot;

use arbos_core::approval::{ApprovalOutcome, ApprovalRequest, Risk};
use arbos_core::ids::{now_ms, ulid};

use crate::agent::Agent;
use crate::approval::WaitMode;
use crate::config::Config;
use crate::mcp::{err_result, text_result};

const POLL: Duration = Duration::from_secs(2);
const DEFAULT_WAIT_S: u64 = 180;
/// Plaintext fields for a browser fill live at most this long in agentd.
const FILL_BUDGET: Duration = Duration::from_secs(30);
/// After a refusal the agent is told to stop; it may ask again after this long (D76 cool-down).
const DENIED_COOLDOWN: Duration = Duration::from_secs(10 * 60);
const USERNAME_SELECTORS: [&str; 7] = [
    "input[autocomplete=username]",
    "input[type=email]",
    "input[name=username]",
    "input[name=user]",
    "input[name=email]",
    "input[name=login]",
    "input[type=text]",
];
const PASSWORD_SELECTORS: [&str; 2] = [
    "input[autocomplete=current-password]",
    "input[type=password]",
];

/// One secret this project may use right now (an enabled Access on the relay), or one the
/// owner allowed once. Scoped by project: a D70 user box runs one agentd for several workspaces.
#[derive(Debug, Clone)]
struct Grant {
    project_id: String,
    /// The request that granted it (one-time grants and browser fills need it for `values`).
    request_id: Option<String>,
    /// Env name = the secret's name (`STRIPE_KEY`), D83 decision 1; an `op://` ref for a
    /// connected-vault item (no env for those: they travel through egress only).
    name: String,
    has_env: bool,
    kind: String,
    /// field → `arbos-secret://NAME/field`.
    placeholders: BTreeMap<String, String>,
    hosts: Vec<String>,
    note: String,
    /// `all` or `chat:<id>`.
    scope: String,
    /// The relay's connected-vault name (`Arbos` for the built-in one).
    vault_name: String,
    /// The owner put the real value in the environment (no card per use).
    inject_env: bool,
    /// Variable names the real value arrives under (`$NAME`, `$NAME_USERNAME`…).
    real_vars: Vec<String>,
    /// User ids on the secret's People list (rows that are on; the owner always may ask).
    director_ids: Vec<String>,
    /// `refuse | card`: what happens when someone not on the list asks (spec §7.7).
    others_ask: String,
    /// Constraints the agent must honour (secret note + per-person notes).
    use_notes: Vec<String>,
}

/// The real values of one secret or vault item, as `GET /box/secrets/env` handed them over.
#[derive(Debug, Clone)]
struct RealValues {
    /// The grant's name: the secret's env name, or the item's `op://vault/item`.
    name: String,
    vars: BTreeMap<String, String>,
}

#[derive(Default)]
struct State {
    grants: Vec<Grant>,
    /// Projects whose owner paused every secret.
    paused: Vec<String>,
    /// project → real values in its box right now (switch on, Access on, not paused).
    real: BTreeMap<String, Vec<RealValues>>,
    /// project → every variable name a secret of this user could occupy, so a shell can drop
    /// the ones that are no longer allowed.
    known_vars: BTreeMap<String, BTreeSet<String>>,
    /// (project, name) → when a denial was last heard; the agent does not re-ask inside the cool-down.
    denied_at: BTreeMap<(String, String), Instant>,
    /// Requests a tool call is waiting on: request id → waker fired by `secret_decided`.
    waiting: BTreeMap<String, oneshot::Sender<String>>,
}

fn state() -> &'static Mutex<State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(State::default()))
}

/// `op://vault/item/field` → `op://vault/item`; anything else unchanged.
fn item_base(name: &str) -> String {
    if !name.starts_with("op://") {
        return name.to_owned();
    }
    let parts: Vec<&str> = name.splitn(5, '/').collect();
    if parts.len() >= 5 {
        format!("op://{}/{}", parts[2], parts[3])
    } else {
        name.to_owned()
    }
}

fn grant_for(project_id: &str, name: &str) -> Option<Grant> {
    let want = item_base(name);
    state()
        .lock()
        .expect("secrets state")
        .grants
        .iter()
        .rev()
        .find(|g| g.project_id == project_id && item_base(&g.name) == want)
        .cloned()
}

fn remember_grant(g: Grant) {
    let mut st = state().lock().expect("secrets state");
    let base = item_base(&g.name);
    st.grants
        .retain(|x| !(x.project_id == g.project_id && item_base(&x.name) == base));
    st.grants.push(g);
}

fn is_paused(project_id: &str) -> bool {
    state()
        .lock()
        .expect("secrets state")
        .paused
        .iter()
        .any(|p| p == project_id)
}

/// `$NAME` (one field) or `$NAME_FIELD` → placeholder, for every grant of the project. Child
/// processes get these; a Daytona box may already carry the same names as Daytona Secrets,
/// which win. Nothing while the owner paused secrets.
pub fn grant_envs(project_id: &str) -> Vec<(String, String)> {
    if is_paused(project_id) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for g in state().lock().expect("secrets state").grants.iter() {
        if g.project_id != project_id {
            continue;
        }
        for (k, v) in g.env_pairs() {
            if std::env::var_os(&k).is_none() && !out.iter().any(|(x, _)| *x == k) {
                out.push((k, v));
            }
        }
    }
    out
}

impl Grant {
    fn from_item(project_id: &str, item: &Value) -> Option<Self> {
        // Arbos secrets are known by their env name; connected-vault items by their `op://` ref.
        let has_env = item["env"].as_str().is_some_and(|e| !e.is_empty());
        let name = item["env"]
            .as_str()
            .filter(|e| !e.is_empty())
            .or(item["ref"].as_str())?
            .to_owned();
        let placeholders: BTreeMap<String, String> = item["placeholders"]
            .as_object()
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_owned())))
                    .collect()
            })
            .unwrap_or_default();
        let hosts = item["hosts"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|h| h.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        let scope = match item["scope"]["kind"].as_str() {
            Some("chat") => format!(
                "chat:{}",
                item["scope"]["chat_id"].as_str().unwrap_or_default()
            ),
            _ => "all".to_owned(),
        };
        let inject_env = item["inject_env"].as_bool().unwrap_or(false);
        let real_vars: Vec<String> = match item["real_vars"].as_array() {
            Some(a) => a
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect(),
            None if has_env => real_var_names(&name, &placeholders),
            None => Vec::new(),
        };
        let director_ids: Vec<String> = item["people"]
            .as_array()
            .or_else(|| item["directors"].as_array())
            .map(|a| {
                a.iter()
                    .filter(|d| d["enabled"].as_bool().unwrap_or(true))
                    .filter_map(|d| d["user_id"].as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        let others_ask = item["others_ask"].as_str().unwrap_or("refuse").to_owned();
        let use_notes: Vec<String> = item["use_notes"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .filter(|s| !s.trim().is_empty())
                    .collect()
            })
            .unwrap_or_else(|| {
                let n = item["note"].as_str().unwrap_or("").trim();
                if n.is_empty() {
                    Vec::new()
                } else {
                    vec![n.to_owned()]
                }
            });
        Some(Grant {
            project_id: project_id.to_owned(),
            request_id: None,
            name,
            has_env,
            kind: item["kind"].as_str().unwrap_or("key").to_owned(),
            placeholders,
            hosts,
            note: item["note"].as_str().unwrap_or("").to_owned(),
            scope,
            vault_name: item["vault_name"].as_str().unwrap_or("Arbos").to_owned(),
            inject_env,
            real_vars,
            director_ids,
            others_ask,
            use_notes,
        })
    }

    fn env_pairs(&self) -> Vec<(String, String)> {
        if !self.has_env {
            return Vec::new();
        }
        if self.placeholders.len() == 1 {
            return self
                .placeholders
                .values()
                .map(|p| (self.name.clone(), p.clone()))
                .collect();
        }
        self.placeholders
            .iter()
            .map(|(f, p)| {
                (
                    format!("{}_{}", self.name, f.to_ascii_uppercase()),
                    p.clone(),
                )
            })
            .collect()
    }

    fn describe(&self, status: &str) -> Value {
        let env: BTreeMap<String, String> = self.env_pairs().into_iter().collect();
        json!({
            "status": status, "requestId": self.request_id, "name": self.name, "kind": self.kind,
            "placeholders": self.placeholders, "env": env, "hosts": self.hosts, "note": self.note, "scope": self.scope,
            "vault": self.vault_name, "real_value": self.inject_env,
            "use_notes": self.use_notes,
            "policy": policy_line(&self.use_notes),
            "note_rule": NOTE_POLICY,
            "who_may_direct": self.who_may_direct(),
            "how": if self.inject_env {
                format!("The owner put the real value in your environment: ${} works in any shell command or program. Keep it out of chat and logs. Only use it when the person asking you is the owner or is on this secret's People list; honour every use_notes constraint.", self.real_vars.first().cloned().unwrap_or_else(|| self.name.clone()))
            } else { match self.kind.as_str() {
                "login" => "Call browser_fill(name, url) to have Arbos type the login into the page; you never see the values.".to_owned(),
                _ => format!("Put the placeholder (or ${}) in a header or body and call the egress tool; the relay swaps in the real value only for the allowed hosts and redacts it from the reply. Never try to read, print, encode or copy the value. Only use it when the person asking you is allowed; honour every use_notes constraint.", self.name),
            } },
        })
    }
}

impl Grant {
    /// The one-line rule the agent reads next to every grant (spec §7.7).
    fn who_may_direct(&self) -> &'static str {
        if self.others_ask == "card" {
            "The owner, and the people on this secret's People list. For anyone else, call secrets_request first: the owner gets a card naming who asked, and you wait for the answer."
        } else {
            "Only the owner, and the people on this secret's People list. Anyone else who asks cannot use it — tell them plainly."
        }
    }
}

/// The chat a card for someone else's ask goes to: the owner's pinned ("main") chat with this
/// agent, opened if needed; the asking chat only when the project has no such chat.
async fn owner_card_chat(agent: &Arc<Agent>, asking_chat: &str) -> String {
    let chats = agent.store.chats().unwrap_or_default();
    let Some(main) = chats
        .iter()
        .find(|c| c.is_pinned && !agent.people.is_people_chat(&c.id))
        .or_else(|| chats.iter().find(|c| !agent.people.is_people_chat(&c.id)))
    else {
        return asking_chat.to_owned();
    };
    if agent.sync.host(&main.id).await.is_none() && agent.sync.open_room(&main.id).await.is_err() {
        return asking_chat.to_owned();
    }
    main.id.clone()
}

/// User id of the human who asked for the work in `chat_id`, when known.
///
/// The chat is the authority, never `people.current`: that slot names whichever chat holds the
/// turn lock right now and is cleared when that turn ends, so a background job, a subagent or a
/// second chat's turn would otherwise report a guest's request as the owner's — and the relay
/// enforces each secret's People list from the `asked_by` this returns.
fn asked_by_user_id(agent: &Agent, chat_id: &str) -> Option<String> {
    agent.people.asker(chat_id).map(|a| a.user_id)
}

/// The person who asked in this chat when they are not the owner (a people-chat guest or member).
fn non_owner_asker(agent: &Agent, chat_id: &str) -> Option<crate::people::Asker> {
    let asker = agent.people.asker(chat_id)?;
    let owner = agent.people.project_owner();
    if asker.role == "owner" || (!owner.is_empty() && asker.user_id == owner) {
        return None;
    }
    Some(asker)
}

/// May the person who asked in this chat direct use of this grant? Owner / no asker → yes; else
/// the People list.
fn asker_may_direct(agent: &Agent, chat_id: &str, g: &Grant) -> bool {
    match non_owner_asker(agent, chat_id) {
        None => true,
        Some(asker) => g.director_ids.iter().any(|id| id == &asker.user_id),
    }
}

/// The plain refusal for a person who is not on the list (same words as the relay's).
fn not_listed_message(name: &str) -> String {
    format!(
        "{name}: the person asking you is not on this secret's People list. Only the owner, or people the owner added under People on the secret's page, may ask for it. Tell them plainly, and do not retry with this secret for them."
    )
}

/// The variable names a secret's real value would take: the same names its placeholders use.
fn real_var_names(name: &str, placeholders: &BTreeMap<String, String>) -> Vec<String> {
    if placeholders.len() <= 1 {
        return vec![name.to_owned()];
    }
    placeholders
        .keys()
        .filter(|f| *f != "url")
        .map(|f| format!("{name}_{}", f.to_ascii_uppercase()))
        .collect()
}

/// The owner's note on a secret is the policy for that key: told to the agent in `secrets_list`,
/// in every grant description and as comments in `secrets.env`, in the same words.
pub const NOTE_POLICY: &str = "Each secret's note (use_notes) is the owner's policy for that key — for egress and for any CLI, SDK or script that reads the real $NAME. Spending guidance in it binds you: a limit (\"may spend up to $X without asking\") lets you proceed within it and no further; \"confirm before charging\" or \"read-only\" means ask first or never charge, whatever the brief says. No spending guidance in the note → the default money rule: show the chosen option with its price and get the owner's go-ahead before anything that charges. Never pass a confirmation-skipping flag (--yes, -y, --force, --no-confirm and the like) to a command that charges unless the note allows that spend.";

/// One line of policy for a key, from its notes: what the shell comment and the tool list show.
fn policy_line(use_notes: &[String]) -> String {
    if use_notes.is_empty() {
        "no note — default money rule: show the option with its price and confirm with the owner before anything that charges; no --yes/-y/--force on commands that charge".to_owned()
    } else {
        use_notes.join("; ")
    }
}

/// True when a reply *promises* a secret request or an approval card instead of making the
/// call — "I'll request it now and you'll get an approval card…". A stated request must be a
/// `secrets_request` tool call, never prose alone (#1440: the card never existed).
pub fn promises_secret_request(text: &str) -> bool {
    let t = text.to_lowercase();
    let about_secret = [
        "secret",
        "credential",
        "api key",
        "key from your vault",
        "the vault",
        "your vault",
        "stored key",
        "token",
    ]
    .iter()
    .any(|w| t.contains(w));
    if !about_secret {
        return false;
    }
    [
        "approval card",
        "card to approve",
        "approve in the chat",
        "approve it in",
        "once you approve",
        "when you approve",
        "after you approve",
        "needs your approval",
        "need your approval",
        "requires your approval",
        "i'll request",
        "i will request",
        "i’ll request",
        "request it now",
        "requesting it now",
        "requesting the",
        "request access",
        "ask for access",
        "ask you for access",
        "settings → vault",
        "settings -> vault",
        "settings → secrets",
    ]
    .iter()
    .any(|w| t.contains(w))
}

/// Env names of the secrets this project may use right now (enabled grants), for the route
/// guard's message to the model.
pub fn enabled_grant_names(project_id: &str) -> Vec<String> {
    state()
        .lock()
        .expect("secrets state")
        .grants
        .iter()
        .filter(|g| g.project_id == project_id)
        .map(|g| g.name.clone())
        .collect()
}

// ---- real values in the environment -------------------------------------------------------------

/// The file a workspace's shells source: `<state>/secrets.env`.
pub fn env_file(cfg: &Config) -> PathBuf {
    cfg.state_dir.join("secrets.env")
}

/// What every harness child gets so its shells find the file: `BASH_ENV` for `bash -c`,
/// `ARBOS_SECRETS_ENV` for the image's login/interactive loader. Plus the placeholder env.
pub fn shell_envs(cfg: &Config) -> Vec<(String, String)> {
    let path = env_file(cfg);
    if !path.is_file() {
        if let Err(e) = write_env_file(cfg) {
            tracing::warn!(error = %e, "secrets.env: first write failed");
        }
    }
    let mut envs = grant_envs(&cfg.project_id);
    let p = path.display().to_string();
    envs.push(("ARBOS_SECRETS_ENV".into(), p.clone()));
    envs.push(("BASH_ENV".into(), p));
    envs
}

fn sh_quote(v: &str) -> String {
    format!("'{}'", v.replace('\'', "'\\''"))
}

/// Render the file for one project from the current state: real values for grants with the
/// switch on, placeholders for the other grants (a Daytona Secret of the same name wins, as at
/// spawn), `unset` for every other name a secret could occupy — all of them while paused.
fn render_env_file(st: &State, project: &str, project_name: &str) -> String {
    let paused = st.paused.iter().any(|p| p == project);
    let mut exports: BTreeMap<String, String> = BTreeMap::new();
    let mut unset: BTreeSet<String> = st.known_vars.get(project).cloned().unwrap_or_default();
    // (variable names, policy line, real value?) per key, for the comment block.
    let mut policies: Vec<(Vec<String>, String, bool)> = Vec::new();
    if !paused {
        let real = st.real.get(project).cloned().unwrap_or_default();
        for g in st.grants.iter().filter(|g| g.project_id == project) {
            let base = item_base(&g.name);
            let values = real
                .iter()
                .find(|r| item_base(&r.name) == base)
                .filter(|_| g.inject_env);
            let mut names = Vec::new();
            match values {
                Some(r) => {
                    for (k, v) in &r.vars {
                        exports.insert(k.clone(), v.clone());
                        names.push(k.clone());
                    }
                }
                None => {
                    for (k, v) in g.env_pairs() {
                        if std::env::var_os(&k).is_none() {
                            exports.insert(k.clone(), v);
                        } else {
                            unset.remove(&k);
                        }
                        names.push(k);
                    }
                }
            }
            if !names.is_empty() {
                policies.push((names, policy_line(&g.use_notes), values.is_some()));
            }
        }
    }
    for k in exports.keys() {
        unset.remove(k);
    }
    let mut out = String::new();
    out.push_str(&format!(
        "# Arbos secrets for {project_name} — written by agentd, read by every shell. Do not edit.\n"
    ));
    out.push_str("# Real values are here only for secrets whose owner switched \"Real value in the agent's\n# environment\" on; the rest hold placeholders for the egress tool.\n");
    if paused {
        out.push_str("# Secrets are paused by the owner.\n");
    }
    if !policies.is_empty() {
        out.push_str("#\n# The owner's note on each key is its policy, for egress and for any program that reads the\n# real value — spending included. No note: show the option with its price and confirm before\n# anything that charges. Never pass --yes / -y / --force to a command that charges unless the\n# note allows that spend.\n");
        for (names, policy, real) in &policies {
            out.push_str(&format!(
                "# {} ({}): {}\n",
                names.join(", "),
                if *real { "real value" } else { "placeholder" },
                policy.replace('\n', " ")
            ));
        }
    }
    if !unset.is_empty() {
        out.push_str("unset ");
        out.push_str(&unset.iter().cloned().collect::<Vec<_>>().join(" "));
        out.push('\n');
    }
    for (k, v) in &exports {
        out.push_str(&format!("export {k}={}\n", sh_quote(v)));
    }
    out
}

/// Write the file atomically; readable by the workspace user, writable by nobody else.
pub fn write_env_file(cfg: &Config) -> std::io::Result<()> {
    let text = render_env_file(
        &state().lock().expect("secrets state"),
        &cfg.project_id,
        &cfg.project_name,
    );
    let path = env_file(cfg);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("env.tmp");
    std::fs::write(&tmp, text)?;
    secure_env_file(&tmp, cfg)?;
    std::fs::rename(&tmp, &path)
}

fn secure_env_file(path: &Path, cfg: &Config) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    match &cfg.run_as {
        Some(ra) => {
            std::os::unix::fs::chown(path, Some(0), Some(ra.gid))?;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o640))
        }
        None => std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)),
    }
}

fn mock_on() -> bool {
    std::env::var("ARBOS_VAULT_MOCK").is_ok_and(|v| v == "1")
}

pub(crate) fn wait_budget() -> Duration {
    Duration::from_secs(
        std::env::var("ARBOS_VAULT_WAIT_S")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_WAIT_S),
    )
}

pub fn tool_specs() -> Vec<Value> {
    let list = json!({ "name": "secrets_list", "description": "Secrets the user keeps in Arbos (and in connected vaults such as 1Password): names, kinds (key | login | text), field names, allowed hosts, and whether this agent has access. Values are never shown. A secret you have access to is already in your environment as $NAME holding a placeholder; otherwise call secrets_request.",
        "inputSchema": { "type": "object", "properties": {} } });
    let request = json!({ "name": "secrets_request", "description": "Ask the owner to let you use a secret. name = the secret's name (or an op:// reference), purpose = one sentence shown on the card, use = 'header' or 'env' for keys (you get a placeholder usable only through egress or as $NAME in a shell), 'browser_fill' for logins (then call browser_fill), host = the host you will use it at. Returns at once when the owner already gave you access; otherwise posts a card and waits for the answer (Allow once / Allow for this chat / Always allow / Deny).",
        "inputSchema": { "type": "object", "properties": { "name": { "type": "string" }, "purpose": { "type": "string", "description": "one sentence shown to the user" }, "use": { "type": "string", "enum": ["header", "env", "browser_fill"] }, "host": { "type": "string", "description": "the host you will send it to, e.g. api.stripe.com" } }, "required": ["name", "purpose"] } });
    let mut alias_list = list.clone();
    alias_list["name"] = json!("vault_list");
    let mut alias_request = request.clone();
    alias_request["name"] = json!("vault_request");
    let mut out = vec![
        list,
        request,
        json!({ "name": "egress", "description": "HTTP request performed by the relay with secret placeholders (arbos-secret://NAME/field, $NAME, or op:// references) substituted in headers/body for the secret's allowed hosts only. Returns status, headers and body with secret values redacted. Use this instead of curl whenever a request needs a secret.",
                "inputSchema": { "type": "object", "properties": { "url": { "type": "string" }, "method": { "type": "string" }, "headers": { "type": "object", "additionalProperties": { "type": "string" } }, "body": { "type": ["string", "object", "null"] } }, "required": ["url"] } }),
        json!({ "name": "browser_fill", "description": "Arbos opens url in the box browser and types the login's username and password into the sign-in form (access needed; the values are never shown to you). Afterwards use the browser as usual (click Sign in, read the page).",
                "inputSchema": { "type": "object", "properties": { "name": { "type": "string" }, "url": { "type": "string" } }, "required": ["name", "url"] } }),
        alias_list,
        alias_request,
    ];
    out.extend(crate::handoff::tool_specs());
    out
}

pub(crate) async fn relay_get(agent: &Agent, path: &str) -> Result<Value, String> {
    let r = agent
        .http_client()
        .get(format!("{}{}", agent.cfg.relay_url, path))
        .bearer_auth(&agent.cfg.box_token)
        .header("x-arbos-project", &agent.cfg.project_id)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = r.status();
    let text = r.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(relay_error(status, &text, path));
    }
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// `(status, body)` → one line for the model: the relay's `error.message` when it has one.
fn relay_error(status: reqwest::StatusCode, text: &str, path: &str) -> String {
    if serde_json::from_str::<arbos_core::api::ApiErrorDto>(text).is_ok() {
        crate::relay_error::message(text)
    } else {
        format!("relay {path}: HTTP {status} {text}")
    }
}

pub(crate) async fn relay_post(
    agent: &Agent,
    path: &str,
    body: &Value,
) -> Result<(u16, Value), String> {
    let r = agent
        .http_client()
        .post(format!("{}{}", agent.cfg.relay_url, path))
        .bearer_auth(&agent.cfg.box_token)
        .header("x-arbos-project", &agent.cfg.project_id)
        .json(body)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = r.status();
    let text = r.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(relay_error(status, &text, path));
    }
    Ok((
        status.as_u16(),
        serde_json::from_str(&text).map_err(|e| e.to_string())?,
    ))
}

/// Pull `GET /box/secrets` and rebuild this project's standing grants. Called at workspace start
/// and on every secrets push.
pub async fn refresh_grants(agent: &Agent) -> Result<Vec<Value>, String> {
    let v = relay_get(agent, "/box/secrets").await?;
    let items = v["items"].as_array().cloned().unwrap_or_default();
    let paused = v["paused"].as_bool().unwrap_or(false);
    let project = agent.cfg.project_id.clone();
    {
        let mut st = state().lock().expect("secrets state");
        st.grants
            .retain(|g| g.project_id != project || g.request_id.is_some());
        let mut known = BTreeSet::new();
        for it in &items {
            if let Some(g) = Grant::from_item(&project, it) {
                known.extend(g.env_pairs().into_iter().map(|(k, _)| k));
                known.extend(g.real_vars.iter().cloned());
                if it["enabled"].as_bool().unwrap_or(false) {
                    st.grants
                        .retain(|x| !(x.project_id == project && x.name == g.name));
                    st.grants.push(g);
                }
            }
        }
        st.known_vars.insert(project.clone(), known);
        st.paused.retain(|p| *p != project);
        if paused {
            st.paused.push(project);
        }
    }
    if let Err(e) = write_env_file(&agent.cfg) {
        tracing::warn!(error = %e, "secrets.env: write failed");
    }
    Ok(items)
}

/// Pull `GET /box/secrets/env` — the real values this box may hold — register them for
/// redaction and rewrite the file. Called with `refresh_grants` at start and on every push.
pub async fn refresh_env(agent: &Agent) -> Result<(), String> {
    let v = relay_get(agent, "/box/secrets/env").await?;
    let project = agent.cfg.project_id.clone();
    let mut real = Vec::new();
    for it in v["items"].as_array().cloned().unwrap_or_default() {
        let name = it["ref"]
            .as_str()
            .filter(|r| r.starts_with("op://"))
            .or(it["env"].as_str())
            .unwrap_or("")
            .to_owned();
        let vars: BTreeMap<String, String> = it["vars"]
            .as_object()
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_owned())))
                    .collect()
            })
            .unwrap_or_default();
        if name.is_empty() || vars.is_empty() {
            continue;
        }
        for val in vars.values() {
            crate::store::redact_live_value(val);
        }
        real.push(RealValues { name, vars });
    }
    let names: Vec<String> = real.iter().map(|r| r.name.clone()).collect();
    state()
        .lock()
        .expect("secrets state")
        .real
        .insert(project, real);
    write_env_file(&agent.cfg).map_err(|e| e.to_string())?;
    tracing::info!(names = ?names, "secrets.env: real values written");
    Ok(())
}

/// Grants and real values in one go: what every push and the workspace start need.
pub async fn refresh_all(agent: &Agent) -> Result<(), String> {
    refresh_grants(agent).await?;
    refresh_env(agent).await
}

/// A secrets push from the relay (`secret_access_changed`, `secrets_paused`,
/// `secret_value_replaced`, `secret_decided`, `secret_notice`, `secret_scrub`).
pub async fn on_push(agent: &Arc<Agent>, kind: &str, payload: &Value) {
    match kind {
        "secret_decided" => {
            let id = payload["request_id"].as_str().unwrap_or("").to_owned();
            let status = payload["status"].as_str().unwrap_or("").to_owned();
            let waker = state().lock().expect("secrets state").waiting.remove(&id);
            if let Some(tx) = waker {
                let _ = tx.send(status);
            }
            let _ = refresh_all(agent).await;
        }
        "secret_notice" => {
            let text = payload["text"].as_str().unwrap_or("").to_owned();
            if text.is_empty() {
                return;
            }
            let chat_hint = payload["chat_id"].as_str().unwrap_or("").to_owned();
            let chat_id = agent.approval_chat(&chat_hint).await.unwrap_or(chat_hint);
            if let Some(host) = agent.sync.host(&chat_id).await {
                let h = host.lock().await;
                h.doc.append_system_note(
                    &ulid(),
                    &format!("agent:{}", agent.cfg.project_id),
                    &text,
                    &agent.cfg.device_id,
                );
            }
            agent.append_audit(
                "secret_notice",
                json!({ "text": text, "host": payload["host"], "name": payload["env"] }),
            );
        }
        "secret_scrub" => {
            let secret_id = payload["secret_id"].as_str().unwrap_or("").to_owned();
            let env = payload["env"].as_str().unwrap_or("").to_owned();
            if secret_id.is_empty() {
                return;
            }
            match scrub_secret(agent, &secret_id).await {
                Ok(stats) => {
                    tracing::info!(
                        secret_id = %secret_id,
                        env = %env,
                        chats = stats.chats,
                        sessions = stats.sessions,
                        t3 = stats.t3,
                        memory = stats.memory,
                        "secret scrubbed from context"
                    );
                    agent.append_audit(
                        "secret_scrub",
                        json!({ "env": env, "secret_id": secret_id, "chats": stats.chats,
                            "sessions": stats.sessions, "t3": stats.t3, "memory": stats.memory }),
                    );
                }
                Err(e) => {
                    tracing::warn!(error = %e, secret_id = %secret_id, "secret scrub failed");
                }
            }
        }
        _ => {
            // `secret_access_changed`, `secrets_paused`, `secret_value_replaced`: the relay is
            // the authority; take its current view (≤ 5 s after the switch, PUSH_POLL).
            if let Err(e) = refresh_all(agent).await {
                tracing::warn!(error = %e, kind, "secrets: refresh after push failed");
            }
            agent.append_audit(
                "secrets_changed",
                json!({ "kind": kind, "env": payload["env"], "enabled": payload["enabled"], "paused": payload["paused"] }),
            );
        }
    }
}

/// Scrub one secret's values from this box now (the relay pushes `secret_scrub` too; the
/// hand-off calls this first so the agent's own session is clean before its turn goes on).
pub(crate) async fn scrub_now(agent: &Arc<Agent>, secret_id: &str) {
    match scrub_secret(agent, secret_id).await {
        Ok(stats) => {
            tracing::info!(
                secret_id,
                chats = stats.chats,
                sessions = stats.sessions,
                t3 = stats.t3,
                memory = stats.memory,
                files = stats.files,
                "secret scrubbed from context (hand-off)"
            );
            agent.append_audit(
                "secret_scrub",
                json!({ "secret_id": secret_id, "chats": stats.chats, "sessions": stats.sessions,
                    "t3": stats.t3, "memory": stats.memory, "files": stats.files, "path": "handoff" }),
            );
        }
        Err(e) => tracing::warn!(error = %e, secret_id, "secret scrub failed (hand-off)"),
    }
}

struct ScrubStats {
    chats: usize,
    sessions: usize,
    t3: usize,
    memory: usize,
    /// Other on-disk text the value could have reached: harness logs, subagent packs, tool
    /// output blobs, the system prompt, the workspace's `.arbos` scratch files.
    files: usize,
}

/// Pull the scrub values from the relay and redact them from every local context store.
async fn scrub_secret(agent: &Arc<Agent>, secret_id: &str) -> Result<ScrubStats, String> {
    let v = relay_get(agent, &format!("/box/secrets/{secret_id}/scrub")).await?;
    let values: Vec<String> = v["values"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_owned))
                .filter(|s| s.len() >= 6)
                .collect()
        })
        .unwrap_or_default();
    if values.is_empty() {
        return Ok(ScrubStats {
            chats: 0,
            sessions: 0,
            t3: 0,
            memory: 0,
            files: 0,
        });
    }
    for val in &values {
        crate::store::redact_live_value(val);
    }

    // Open every known chat so historical leaks are covered, not only the ones already loaded.
    if let Ok(chats) = agent.store.chats() {
        for c in chats {
            if agent.sync.host(&c.id).await.is_none() {
                let _ = agent.sync.open_room(&c.id).await;
            }
        }
    }

    let mut chats = 0;
    let hosts: Vec<_> = {
        let map = agent.sync.hosts.lock().await;
        map.values().cloned().collect()
    };
    for host in hosts {
        let (n, repersist) = {
            let h = host.lock().await;
            let n = h.doc.scrub_values(&values);
            // A relay-side redaction already arrived as a row: the doc is in shallow mode but
            // the persisted snapshot may still be the full one — write it again.
            (n, h.doc.is_shallow_mode())
        };
        if n > 0 {
            chats += n;
        }
        if n > 0 || repersist {
            let _ = agent.sync.persist(&host).await;
        }
    }

    let sessions = scrub_session_files(&agent.cfg, &values);
    let t3 = agent.store.t3_scrub(&values).unwrap_or(0);
    let memory = agent.store.memory_scrub(&values).unwrap_or(0);
    let files = scrub_other_files(&agent.cfg, &values);
    let _ = agent.store.flush_after_scrub();
    Ok(ScrubStats {
        chats,
        sessions,
        t3,
        memory,
        files,
    })
}

/// Everything else under the state dir that holds text: harness logs, subagent packs, tool
/// output blobs, the written system prompt, scratch. Binary files are skipped (not UTF-8).
fn scrub_other_files(cfg: &crate::config::Config, values: &[String]) -> usize {
    let mut n = 0;
    for dir in ["logs", "packs", "blobs", "scratch"] {
        n += scrub_dir_files(&cfg.state_dir.join(dir), values);
    }
    let prompt = cfg.pi_dir().join("system-prompt.md");
    if scrub_one_file(&prompt, values) {
        n += 1;
    }
    n
}

/// Walk pi / Prime / Claude session files and rewrite any that still hold a plaintext value.
fn scrub_session_files(cfg: &crate::config::Config, values: &[String]) -> usize {
    let mut n = 0;
    let dirs = [
        cfg.pi_sessions_dir(),
        cfg.prime_dir().join("sessions"),
        cfg.state_dir.join("claude").join("sessions"),
        cfg.pi_dir().join("subagent-sessions"),
    ];
    for dir in dirs {
        n += scrub_dir_files(&dir, values);
    }
    n
}

fn scrub_dir_files(dir: &Path, values: &[String]) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut n = 0;
    for ent in entries.flatten() {
        let path = ent.path();
        if path.is_dir() {
            n += scrub_dir_files(&path, values);
            continue;
        }
        if scrub_one_file(&path, values) {
            n += 1;
        }
    }
    n
}

fn scrub_one_file(path: &Path, values: &[String]) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    let mut next = text.clone();
    for v in values {
        if next.contains(v) {
            next = next.replace(v, "[REDACTED:vault]");
        }
    }
    if next == text {
        return false;
    }
    std::fs::write(path, next).is_ok()
}

pub async fn secrets_list(agent: &Agent) -> Value {
    let list = match refresh_grants(agent).await {
        Ok(l) => l,
        Err(e) => return err_result(e),
    };
    let paused = is_paused(&agent.cfg.project_id);
    let items: Vec<Value> = list
        .iter()
        .map(|it| {
            let access = it["enabled"].as_bool().unwrap_or(false);
            json!({ "name": if it["env"].is_null() { it["title"].clone() } else { it["env"].clone() }, "kind": it["kind"], "fields": it["fields"], "placeholders": it["placeholders"],
                "hosts": it["hosts"], "note": it["note"], "use_notes": it["use_notes"], "people": if it["people"].is_null() { it["directors"].clone() } else { it["people"].clone() },
                "others_ask": it["others_ask"], "who_may_direct": it["who_may_direct"],
                "policy": policy_line(&it["use_notes"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect::<Vec<_>>()).unwrap_or_default()),
                "vault": it["vault_name"], "ref": it["ref"],
                "access": if paused { "paused" } else if access { "on" } else { "off" },
                "ask_every_time": it["ask_every_time"],
                "real_value": it["inject_env"], "real_env": if it["env"].is_null() { it["real_env"].clone() } else { it["env"].clone() } })
        })
        .collect();
    text_result(
        json!({ "paused": paused, "items": items, "note_rule": NOTE_POLICY,
            "note": if paused { "Secrets are paused by the owner: every use fails until they turn Pause all secrets off. Stop and say so." }
                    else { "A secret with access on is already $NAME in your shell: the real value when real_value is true (use it directly in commands and programs; keep it out of chat and logs), otherwise a placeholder usable through egress. For one with access off call secrets_request(name, purpose, use, host). Only the owner — or people on that secret's People list — may ask you to use it; when others_ask is card, anyone else's ask goes through secrets_request and the owner's card; otherwise tell them it is not theirs to ask for. Honour every use_notes constraint." } })
            .to_string(),
    )
}

async fn relay_status(agent: &Agent, request_id: &str) -> Result<String, String> {
    let v = relay_get(agent, &format!("/box/secrets/requests/{request_id}")).await?;
    Ok(v["request"]["status"]
        .as_str()
        .unwrap_or("pending")
        .to_owned())
}

/// Flip the chat card to match the relay's decision. Already decided from the card itself is
/// fine: first decision wins.
async fn settle_card(agent: &Arc<Agent>, approval_id: Option<&str>, status: &str) {
    let Some(id) = approval_id else { return };
    let outcome = match status {
        "allowed_once" | "allowed_chat" | "allowed_always" => ApprovalOutcome::Approved,
        "denied" => ApprovalOutcome::Rejected {
            reason: Some("denied by the owner".into()),
        },
        _ => ApprovalOutcome::Expired,
    };
    // The decision rides in `decidedBy` so clients can show which row was picked.
    let _ = agent
        .decide_approval(id, outcome, &format!("arbos:secrets:{status}"))
        .await;
}

/// Wait for the decision. The relay row is the authority: Settings writes it directly, and a
/// client that answers the chat card forwards the decision there (`secretRequestId` in the card
/// payload). The relay's `secret_decided` push ends the wait at once; polling covers the rest.
/// `Ok(None)` when the budget runs out with the request still pending.
async fn wait_decision(
    agent: &Arc<Agent>,
    request_id: &str,
    approval_id: Option<&str>,
    mut card: Option<oneshot::Receiver<ApprovalOutcome>>,
    budget: Duration,
) -> Result<Option<String>, String> {
    let start = Instant::now();
    let (tx, mut pushed) = oneshot::channel::<String>();
    state()
        .lock()
        .expect("secrets state")
        .waiting
        .insert(request_id.to_owned(), tx);
    let result = loop {
        let status = relay_status(agent, request_id).await?;
        if status != "pending" {
            settle_card(agent, approval_id, &status).await;
            break Ok(Some(status));
        }
        if start.elapsed() >= budget {
            break Ok(None);
        }
        tokio::select! {
            res = &mut pushed => {
                if let Ok(status) = res {
                    if status != "pending" {
                        settle_card(agent, approval_id, &status).await;
                        break Ok(Some(status));
                    }
                }
            }
            res = async { match card.as_mut() { Some(rx) => rx.await, None => std::future::pending().await } } => {
                card = None;
                match res {
                    // The client forwards the approval to the relay; keep polling.
                    Ok(ApprovalOutcome::Approved) => {}
                    Ok(ApprovalOutcome::Rejected { .. }) => break Ok(Some("denied".into())),
                    Ok(ApprovalOutcome::Expired) | Err(_) => break Ok(Some("expired".into())),
                }
            }
            _ = tokio::time::sleep(POLL) => {}
        }
    };
    state()
        .lock()
        .expect("secrets state")
        .waiting
        .remove(request_id);
    result
}

/// Find the standing grant or ask the relay; post the card when the relay says a decision is
/// needed; wait for it. Shared by `secrets_request` and `browser_fill`.
async fn obtain_grant(
    agent: &Arc<Agent>,
    chat_hint: &str,
    name: &str,
    purpose: &str,
    use_kind: &str,
    host: Option<&str>,
) -> Result<Grant, Value> {
    // Access cards belong on the pinned main chat (same rule as `approval_chat`).
    let chat_id = agent
        .approval_chat(chat_hint)
        .await
        .unwrap_or_else(|| chat_hint.to_owned());
    let chat_id = chat_id.as_str();
    let project = agent.cfg.project_id.clone();
    if is_paused(&project) {
        return Err(err_result("Secrets are paused by the owner. Stop and tell the user; do not ask again until they turn Pause all secrets off."));
    }
    let name = if name.starts_with("op://") {
        name.to_owned()
    } else {
        name.strip_prefix("arbos-secret://")
            .and_then(|r| r.split('/').next())
            .unwrap_or(name)
            .trim_start_matches('$')
            .to_owned()
    };
    let cooled = state()
        .lock()
        .expect("secrets state")
        .denied_at
        .get(&(project.clone(), name.clone()))
        .is_some_and(|t| t.elapsed() < DENIED_COOLDOWN);
    if cooled {
        return Err(err_result(format!(
            "{name}: the owner denied this in the last 10 minutes. Do not ask again now; tell the user what you needed it for."
        )));
    }
    let host_ok = |g: &Grant| {
        host.is_none()
            || g.hosts.is_empty()
            || host.is_some_and(|h| {
                g.hosts.iter().any(|a| {
                    let a = a.to_ascii_lowercase();
                    h.eq_ignore_ascii_case(&a)
                        || a.strip_prefix("*.")
                            .is_some_and(|suf| h.to_ascii_lowercase().ends_with(suf))
                })
            })
    };
    let asker = non_owner_asker(agent, chat_hint);
    if let Some(g) = grant_for(&project, &name) {
        if g.request_id.is_none() && host_ok(&g) && use_kind != "browser_fill" {
            if asker_may_direct(agent, chat_hint, &g) {
                return Ok(g);
            }
            if g.others_ask != "card" {
                return Err(err_result(not_listed_message(&name)));
            }
            // Ask me: fall through to the relay, which raises the owner's card.
        }
    }
    let mut req = json!({ "name": name, "purpose": purpose, "use": use_kind, "host": host, "chat_id": chat_id });
    if let Some(uid) = asked_by_user_id(agent, chat_hint) {
        req["asked_by"] = json!(uid);
    }
    let (status_code, created) = relay_post(agent, "/box/secrets/request", &req)
        .await
        .map_err(err_result)?;
    let request = &created["request"];
    let request_id = request["id"].as_str().unwrap_or("").to_owned();
    if status_code == 200 {
        // Standing Access covers it: no card.
        let _ = refresh_grants(agent).await;
        let mut g = grant_for(&project, &name)
            .or_else(|| Grant::from_item(&project, &created["item"]))
            .ok_or_else(|| err_result(format!("{name}: no such secret")))?;
        g.request_id = Some(request_id);
        remember_grant(g.clone());
        return Ok(g);
    }
    let resumed = created["resumed"].as_bool().unwrap_or(false);
    let kind = {
        let items = refresh_grants(agent).await.unwrap_or_default();
        items
            .iter()
            .find(|it| {
                it["env"].as_str() == Some(name.as_str())
                    || it["ref"]
                        .as_str()
                        .is_some_and(|r| item_base(r) == item_base(&name))
            })
            .and_then(|it| it["kind"].as_str())
            .unwrap_or("key")
            .to_owned()
    };
    if use_kind == "browser_fill" && kind != "login" {
        return Err(err_result(format!(
            "{name} is a {kind}; browser_fill needs a Login"
        )));
    }
    agent.append_audit(
        "secret_requested",
        json!({ "requestId": request_id, "name": name, "kind": kind, "use": use_kind, "host": host, "purpose": purpose, "chatId": chat_id, "resumed": resumed }),
    );
    let _ = agent.store.t3_append(
        chat_id,
        None,
        Some(chat_id),
        "secret_request",
        Some("assistant"),
        &format!("secrets_request {name} ({use_kind}): {purpose}"),
        None,
    );
    let (approval_id, card) = if resumed {
        (None, None)
    } else {
        // One card per request, four answers (D76 A–D). `secretRequestId` lets the client
        // forward the answer to the relay row, which stays the authority (`mcp.rs` skips its
        // generic gate for the secrets tools so this is the only card).
        let (tx, rx) = oneshot::channel();
        let agent_name = agent.cfg.project_name.clone();
        // A person other than the owner asked: the card names them, "Always allow" adds them
        // to the People list, and it goes to the owner's own chat with this agent — never
        // into the shared chat (spec §7.7).
        let for_whom = asker
            .as_ref()
            .map(|a| format!(" for @{}", a.handle))
            .unwrap_or_default();
        let title = match (kind.as_str(), host) {
            ("login", Some(h)) => format!("Let {agent_name} sign in to {h} with {name}{for_whom}?"),
            ("login", None) => format!("Let {agent_name} sign in with {name}{for_whom}?"),
            (_, Some(h)) => format!("Let {agent_name} use {name}{for_whom} at {h}?"),
            (_, None) => format!("Let {agent_name} use {name}{for_whom}?"),
        };
        let always = match &asker {
            Some(a) => format!("Always allow @{}", a.handle),
            None => format!("Always allow {agent_name}"),
        };
        let summary = match &asker {
            Some(a) => format!("{} asked · {purpose}", a.spoken()),
            None => purpose.to_owned(),
        };
        let req = ApprovalRequest {
            title,
            summary,
            risk: Risk::Credential,
            payload: json!({ "kind": "secret_request", "secretRequestId": request_id, "name": name, "secretKind": kind,
                "use": use_kind, "host": host, "purpose": purpose, "agent": agent_name,
                "askedBy": asker.as_ref().map(|a| a.user_id.clone()),
                "askedByHandle": asker.as_ref().map(|a| a.handle.clone()),
                "askedByName": asker.as_ref().map(|a| a.spoken().to_owned()),
                "askedIn": asker.as_ref().map(|_| chat_id),
                "answers": [
                    { "id": "once", "label": "Allow once" },
                    { "id": "chat", "label": "Allow for this chat" },
                    { "id": "always", "label": always },
                    { "id": "deny", "label": "Deny" },
                ] }),
        };
        let card_chat = match &asker {
            Some(_) => owner_card_chat(agent, chat_id).await,
            None => chat_id.to_owned(),
        };
        match agent
            .begin_approval(&card_chat, req, WaitMode::Inline, Some(tx))
            .await
        {
            Ok(id) => (Some(id), Some(rx)),
            Err(e) => {
                tracing::warn!(error = %e, name = %name, "secrets: approval card could not be posted; Settings → Secrets still decides");
                (None, None)
            }
        }
    };

    let decided = wait_decision(
        agent,
        &request_id,
        approval_id.as_deref(),
        card,
        wait_budget(),
    )
    .await
    .map_err(err_result)?;
    let Some(status) = decided else {
        let note = match &asker {
            Some(a) => format!("The owner has not decided yet; the card is in the owner's own chat with you. Tell {} only that you have asked the owner and end the turn; calling secrets_request again with the same name resumes waiting and returns at once once it is allowed.", a.spoken()),
            None => "The owner has not decided yet (the card in this chat, or Arbos → Settings → Secrets). Tell them you are waiting for access and end the turn; calling secrets_request again with the same name resumes waiting and returns at once once it is allowed.".to_owned(),
        };
        return Err(text_result(
            json!({ "status": "pending", "requestId": request_id, "name": name, "use": use_kind, "note": note })
                .to_string(),
        ));
    };
    agent.append_audit(
        "secret_decided",
        json!({ "requestId": request_id, "name": name, "use": use_kind, "status": status, "decidedBy": "owner" }),
    );
    let msg_id = agent
        .turn
        .lock()
        .await
        .as_ref()
        .filter(|t| t.chat_id == chat_id)
        .map(|t| t.assistant_msg_id.clone());
    if let Some(msg_id) = msg_id {
        if let Some(host) = agent.sync.host(chat_id).await {
            host.lock().await.doc.add_hidden_part(
                &msg_id,
                "secret_request",
                &format!("secret {name} ({use_kind}): {status}"),
                &json!({ "requestId": request_id, "name": name, "use": use_kind, "status": status, "decidedAt": now_ms() }),
            );
        }
    }
    if !matches!(
        status.as_str(),
        "allowed_once" | "allowed_chat" | "allowed_always"
    ) {
        if status == "denied" {
            state()
                .lock()
                .expect("secrets state")
                .denied_at
                .insert((project.clone(), name.clone()), Instant::now());
        }
        return Err(err_result(format!(
            "{name}: the owner {} this request. Stop, and tell the user what you needed it for; do not ask again for 10 minutes.",
            if status == "denied" { "denied" } else { "did not answer" }
        )));
    }
    let items = refresh_grants(agent).await.unwrap_or_default();
    let mut g = grant_for(&project, &name)
        .or_else(|| {
            items
                .iter()
                .find(|it| {
                    it["env"].as_str() == Some(name.as_str())
                        || it["ref"]
                            .as_str()
                            .is_some_and(|r| item_base(r) == item_base(&name))
                })
                .and_then(|it| Grant::from_item(&project, it))
        })
        .ok_or_else(|| err_result(format!("{name}: the secret is gone")))?;
    g.request_id = Some(request_id);
    if let Some(h) = host {
        if !g.hosts.iter().any(|x| x == h) {
            g.hosts.push(h.to_owned());
        }
    }
    remember_grant(g.clone());
    Ok(g)
}

pub async fn secrets_request(agent: &Arc<Agent>, args: &Value, chat_id: &str) -> Value {
    let name = args["name"]
        .as_str()
        .or(args["ref"].as_str())
        .unwrap_or("")
        .trim()
        .to_owned();
    let purpose = args["purpose"].as_str().unwrap_or("").trim().to_owned();
    let use_kind = args["use"].as_str().unwrap_or("header").trim().to_owned();
    let host = args["host"]
        .as_str()
        .map(str::trim)
        .filter(|h| !h.is_empty())
        .map(|h| {
            reqwest::Url::parse(h)
                .ok()
                .and_then(|u| u.host_str().map(str::to_owned))
                .unwrap_or_else(|| h.to_ascii_lowercase())
        });
    if name.is_empty() {
        return err_result("name is required (see secrets_list)");
    }
    if !matches!(use_kind.as_str(), "env" | "header" | "browser_fill") {
        return err_result("use must be header, env or browser_fill");
    }
    match obtain_grant(agent, chat_id, &name, &purpose, &use_kind, host.as_deref()).await {
        Ok(g) => text_result(g.describe("allowed").to_string()),
        Err(v) => v,
    }
}

/// `$NAME` / `${NAME}` → the granted placeholder, so shell-style references work.
fn expand_env_refs(project_id: &str, text: &str) -> String {
    let mut out = text.to_owned();
    for g in state().lock().expect("secrets state").grants.iter() {
        if g.project_id != project_id {
            continue;
        }
        for (k, v) in g.env_pairs() {
            out = out
                .replace(&format!("${{{k}}}"), &v)
                .replace(&format!("${k}"), &v);
        }
    }
    out
}

pub async fn egress(agent: &Arc<Agent>, args: &Value, chat_id: &str) -> Value {
    let url = args["url"].as_str().unwrap_or("").trim().to_owned();
    if url.is_empty() {
        return err_result("url is required");
    }
    let method = args["method"]
        .as_str()
        .unwrap_or("GET")
        .trim()
        .to_ascii_uppercase();
    let project = agent.cfg.project_id.as_str();
    if is_paused(project) {
        return err_result("Secrets are paused by the owner. Stop and tell the user.");
    }
    let headers: BTreeMap<String, String> = args["headers"]
        .as_object()
        .map(|m| {
            m.iter()
                .map(|(k, v)| {
                    let s = v
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| v.to_string());
                    (k.clone(), expand_env_refs(project, &s))
                })
                .collect()
        })
        .unwrap_or_default();
    let body = match &args["body"] {
        Value::Null => Value::Null,
        Value::String(s) => Value::String(expand_env_refs(project, s)),
        other => Value::String(expand_env_refs(project, &other.to_string())),
    };
    let mut placeholders: Vec<String> = Vec::new();
    for text in headers
        .values()
        .cloned()
        .chain(body.as_str().map(str::to_owned))
    {
        for ph in text
            .split(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ',' | ';'))
            .filter(|t| t.starts_with("arbos-secret://") || t.starts_with("op://"))
        {
            if !placeholders.contains(&ph.to_owned()) {
                placeholders.push(ph.to_owned());
            }
        }
    }
    agent.append_audit(
        "egress",
        json!({ "url": url, "method": method, "placeholders": placeholders, "chatId": chat_id }),
    );
    let _ = agent.store.t3_append(
        chat_id,
        None,
        Some(chat_id),
        "egress",
        Some("assistant"),
        &format!(
            "egress {method} {url} placeholders={}",
            placeholders.join(",")
        ),
        None,
    );
    let mut egress_body = json!({ "url": url, "method": method, "headers": headers, "body": body, "placeholders": placeholders, "chat_id": chat_id });
    if let Some(uid) = asked_by_user_id(agent, chat_id) {
        egress_body["asked_by"] = json!(uid);
    }
    let r = agent
        .http_client()
        .post(format!("{}/egress", agent.cfg.relay_url))
        .bearer_auth(&agent.cfg.box_token)
        .header("x-arbos-project", &agent.cfg.project_id)
        .json(&egress_body)
        .send()
        .await;
    let (status, text) = match r {
        Ok(r) => {
            let st = r.status();
            (st, r.text().await.unwrap_or_default())
        }
        Err(e) => return err_result(format!("relay unreachable: {e}")),
    };
    let text = crate::store::redact(&text);
    if !status.is_success() && status.as_u16() != 502 {
        if status.as_u16() == 403 && text.contains("\"paused\"") {
            let _ = refresh_grants(agent).await;
            return err_result("Secrets are paused by the owner. Stop and tell the user; do not retry until they turn Pause all secrets off.");
        }
        return err_result(format!("egress: {}", relay_error(status, &text, "/egress")));
    }
    // A refusal the relay reported inside a 200 (the request went out with the placeholder
    // verbatim) is worth a fresh look at the grants: the switch may have flipped.
    if text.contains("access was turned off") || text.contains("no access for this agent") {
        let _ = refresh_grants(agent).await;
    }
    text_result(text)
}

/// Run `agent-browser` with a timeout; stdout/stderr are redacted before anyone reads them.
async fn agent_browser(args: &[&str], timeout: Duration) -> Result<String, String> {
    let fut = tokio::process::Command::new("agent-browser")
        .args(args)
        .kill_on_drop(true)
        .output();
    let out = tokio::time::timeout(timeout, fut)
        .await
        .map_err(|_| "agent-browser timed out".to_owned())?
        .map_err(|e| format!("agent-browser: {e}"))?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let text = crate::store::redact(&text);
    if out.status.success() {
        Ok(text)
    } else {
        Err(text.chars().take(300).collect())
    }
}

fn agent_browser_available() -> bool {
    std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join("agent-browser").is_file()))
}

pub async fn browser_fill(agent: &Arc<Agent>, args: &Value, chat_id: &str) -> Value {
    let name = args["name"].as_str().unwrap_or("").trim().to_owned();
    let url = args["url"].as_str().unwrap_or("").trim().to_owned();
    if name.is_empty() || url.is_empty() {
        return err_result("name and url are required");
    }
    let host = reqwest::Url::parse(&url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_default();
    let grant = match obtain_grant(
        agent,
        chat_id,
        &name,
        &format!("sign in at {url}"),
        "browser_fill",
        Some(&host),
    )
    .await
    {
        Ok(g) => g,
        Err(v) => return v,
    };
    let Some(request_id) = grant.request_id.clone() else {
        return err_result(format!(
            "{name}: no fill window; call secrets_request first"
        ));
    };
    // The values arrive now and are gone when this function returns.
    let values = match relay_post(
        agent,
        &format!("/box/secrets/requests/{request_id}/values"),
        &json!({}),
    )
    .await
    {
        Ok((_, v)) => v,
        Err(e) => {
            // The approval window may have closed; drop the grant so the next call asks again.
            state()
                .lock()
                .expect("secrets state")
                .grants
                .retain(|g| g.request_id.as_deref() != Some(request_id.as_str()));
            return err_result(format!("could not fetch login fields: {e}"));
        }
    };
    let fields: BTreeMap<String, String> = values["fields"]
        .as_object()
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_owned())))
                .collect()
        })
        .unwrap_or_default();
    for v in fields.values() {
        crate::store::redact_live_value(v);
    }
    let started = Instant::now();
    let mut filled: Vec<&str> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    let simulated = !agent_browser_available();
    if simulated && !mock_on() {
        return err_result("agent-browser is not installed in this box");
    }
    if simulated {
        // Mock: no browser here; stand in for the form submit with one direct POST so the demo
        // target can show it received the real login, which the model still never sees.
        let form: Vec<(String, String)> = ["username", "password"]
            .iter()
            .filter_map(|k| fields.get(*k).map(|v| ((*k).to_owned(), v.clone())))
            .collect();
        match agent
            .http_client()
            .post(&url)
            .timeout(Duration::from_secs(10))
            .form(&form)
            .send()
            .await
        {
            Ok(r) => {
                notes.push(format!("simulated form submit: HTTP {}", r.status()));
                filled.extend(form.iter().map(|(k, _)| {
                    if k == "username" {
                        "username"
                    } else {
                        "password"
                    }
                }));
            }
            Err(e) => notes.push(format!("simulated form submit failed: {e}")),
        }
    } else {
        match agent_browser(&["open", &url], FILL_BUDGET).await {
            Ok(_) => {}
            Err(e) => return err_result(format!("open {url}: {e}")),
        }
        let remaining = || FILL_BUDGET.saturating_sub(started.elapsed());
        if let Some(u) = fields.get("username") {
            for sel in USERNAME_SELECTORS {
                if remaining().is_zero() {
                    break;
                }
                if agent_browser(&["fill", sel, u], remaining()).await.is_ok() {
                    filled.push("username");
                    break;
                }
            }
        }
        if let Some(p) = fields.get("password") {
            for sel in PASSWORD_SELECTORS {
                if remaining().is_zero() {
                    break;
                }
                if agent_browser(&["fill", sel, p], remaining()).await.is_ok() {
                    filled.push("password");
                    break;
                }
            }
        }
        if filled.is_empty() {
            notes.push("no sign-in fields found on the page".into());
        }
    }
    drop(fields);
    agent.append_audit(
        "browser_fill",
        json!({ "name": name, "url": url, "filled": filled, "simulated": simulated, "ms": started.elapsed().as_millis() as i64, "chatId": chat_id }),
    );
    let _ = agent.store.t3_append(
        chat_id,
        None,
        Some(chat_id),
        "browser_fill",
        Some("assistant"),
        &format!("browser_fill {name} at {url}: filled {}", filled.join(", ")),
        None,
    );
    text_result(
        json!({ "ok": !filled.is_empty(), "name": name, "url": url, "filled": filled, "simulated": simulated, "notes": notes,
            "note": "Arbos typed the values; they are not available to you. Continue in the browser (submit the form, read the result)." })
            .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A secret use is attributed to the chat it was asked in, not to whichever chat happens to
    /// hold the turn lock. The relay enforces a secret's People list from `asked_by`, and
    /// `asker_may_direct` reads the same asker, so attributing a guest's request to the owner
    /// hands the guest the owner's authority over every secret.
    #[tokio::test]
    async fn a_secret_use_is_attributed_to_the_chat_it_was_asked_in() {
        let seen: Arc<std::sync::Mutex<Vec<Value>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorder = seen.clone();
        let extra = axum::Router::new().route(
            "/egress",
            axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
                let recorder = recorder.clone();
                async move {
                    recorder.lock().expect("seen").push(body);
                    axum::Json(json!({ "status": 200, "body": "ok" }))
                }
            }),
        );
        let (agent, root) = crate::status::tests::test_agent_with(extra).await;
        agent.people.set_project_owner("u-owner");
        agent.people.test_set_asker("c-owner", "u-owner", "owner");
        agent.people.test_set_asker("c-guest", "u-guest", "guest");

        // The owner's chat holds the turn lock; the call belongs to the guest's chat.
        agent.people.set_current(Some("c-owner"));
        let v = egress(
            &agent,
            &json!({ "url": "https://api.example.com/v1/x", "method": "GET",
                     "headers": { "authorization": "Bearer arbos-secret://STRIPE_KEY/value" } }),
            "c-guest",
        )
        .await;
        assert!(v.get("isError").is_none(), "{v}");
        let body = seen.lock().expect("seen").last().cloned().expect("a call");
        assert_eq!(body["chat_id"], json!("c-guest"));
        assert_eq!(
            body["asked_by"],
            json!("u-guest"),
            "the guest who asked, not the owner whose chat holds the lock"
        );

        // No turn at all (a background job, after the turn that started it ended): still the
        // guest, never an absent `asked_by` the relay reads as the owner.
        agent.people.set_current(None);
        let v = egress(
            &agent,
            &json!({ "url": "https://api.example.com/v1/x", "method": "GET" }),
            "c-guest",
        )
        .await;
        assert!(v.get("isError").is_none(), "{v}");
        let body = seen.lock().expect("seen").last().cloned().expect("a call");
        assert_eq!(body["asked_by"], json!("u-guest"));

        // The owner's own chat is still the owner's: no asker on the body is the owner's case.
        let v = egress(
            &agent,
            &json!({ "url": "https://api.example.com/v1/x", "method": "GET" }),
            "c-owner",
        )
        .await;
        assert!(v.get("isError").is_none(), "{v}");
        let body = seen.lock().expect("seen").last().cloned().expect("a call");
        assert_eq!(body["asked_by"], json!("u-owner"));
        let _ = std::fs::remove_dir_all(root);
    }

    /// The People list is read for the asker of the chat the secret is wanted in: a guest the
    /// owner did not list cannot direct the grant from their own chat while the owner's chat is
    /// the one holding the turn lock.
    #[tokio::test]
    async fn the_people_list_is_read_for_the_chat_the_secret_is_wanted_in() {
        let (agent, root) = crate::status::tests::test_agent().await;
        agent.people.set_project_owner("u-owner");
        agent.people.test_set_asker("c-owner", "u-owner", "owner");
        agent.people.test_set_asker("c-guest", "u-guest", "guest");
        let g = Grant::from_item(
            &agent.cfg.project_id,
            &json!({ "env": "ASKERTEST_KEY", "kind": "key", "enabled": true, "hosts": [],
                "placeholders": { "value": "arbos-secret://ASKERTEST_KEY/value" },
                "people": [ { "user_id": "u-listed", "enabled": true } ], "others_ask": "refuse" }),
        )
        .expect("grant");

        agent.people.set_current(Some("c-owner"));
        assert!(
            !asker_may_direct(&agent, "c-guest", &g),
            "an unlisted guest may not direct it from their own chat"
        );
        assert!(asker_may_direct(&agent, "c-owner", &g), "the owner may");
        agent.people.set_current(None);
        assert!(
            !asker_may_direct(&agent, "c-guest", &g),
            "and not once the turn that set the lock has ended"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// The People list reaches the box as `people[]` (rows that are on) plus `others_ask`;
    /// an older relay's `directors[]` still works; the rule line follows the switch.
    #[test]
    fn people_list_and_others_ask_come_from_the_box_item() {
        let item = json!({ "env": "ENGY_API_KEY", "kind": "key", "enabled": true, "hosts": [],
            "placeholders": { "value": "arbos-secret://ENGY_API_KEY/value" }, "note": "", "vault_name": "Arbos",
            "people": [ { "user_id": "u_sam", "enabled": true }, { "user_id": "u_samir", "enabled": false } ],
            "directors": [ { "user_id": "u_old" } ],
            "others_ask": "card" });
        let g = Grant::from_item("p1", &item).unwrap();
        assert_eq!(g.director_ids, vec!["u_sam".to_owned()]);
        assert_eq!(g.others_ask, "card");
        assert!(g.who_may_direct().contains("secrets_request first"));

        let legacy = json!({ "env": "K", "kind": "key", "enabled": true, "hosts": [],
            "placeholders": { "value": "arbos-secret://K/value" }, "note": "", "vault_name": "Arbos",
            "directors": [ { "user_id": "u_old" } ] });
        let g = Grant::from_item("p1", &legacy).unwrap();
        assert_eq!(g.director_ids, vec!["u_old".to_owned()]);
        assert_eq!(g.others_ask, "refuse");
        assert!(g.who_may_direct().contains("cannot use it"));
        assert!(not_listed_message("K").contains("People list"));
    }

    /// Env name = the secret's own name (decision 1): one-field secrets are `$NAME`, logins
    /// `$NAME_USERNAME` / `$NAME_PASSWORD`; nothing while paused.
    #[test]
    fn env_pairs_follow_decision_1() {
        let item = json!({ "env": "STRIPE_KEY", "kind": "key", "enabled": true, "hosts": ["api.stripe.com"],
            "placeholders": { "value": "arbos-secret://STRIPE_KEY/value" }, "note": "refunds", "vault_name": "Arbos" });
        let g = Grant::from_item("p1", &item).unwrap();
        assert_eq!(
            g.env_pairs(),
            vec![(
                "STRIPE_KEY".to_owned(),
                "arbos-secret://STRIPE_KEY/value".to_owned()
            )]
        );
        let login = json!({ "env": "GITHUB_LOGIN", "kind": "login", "enabled": true, "hosts": [],
            "placeholders": { "username": "arbos-secret://GITHUB_LOGIN/username", "password": "arbos-secret://GITHUB_LOGIN/password" },
            "scope": { "kind": "chat", "chat_id": "c7" } });
        let g2 = Grant::from_item("p1", &login).unwrap();
        let mut names: Vec<String> = g2.env_pairs().into_iter().map(|(k, _)| k).collect();
        names.sort();
        assert_eq!(
            names,
            vec!["GITHUB_LOGIN_PASSWORD", "GITHUB_LOGIN_USERNAME"]
        );
        assert_eq!(g2.scope, "chat:c7");

        remember_grant(g.clone());
        assert_eq!(grant_envs("p1").len(), 1);
        assert!(grant_envs("p2").is_empty());
        state().lock().unwrap().paused.push("p1".into());
        assert!(grant_envs("p1").is_empty());
        state().lock().unwrap().paused.clear();
        assert_eq!(
            expand_env_refs("p1", "Bearer $STRIPE_KEY and ${STRIPE_KEY}"),
            "Bearer arbos-secret://STRIPE_KEY/value and arbos-secret://STRIPE_KEY/value"
        );
        let described = g.describe("allowed").to_string();
        assert!(described.contains("$STRIPE_KEY") && !described.contains("sk-"));
    }

    /// #1440: "I'll request it now and you'll get an approval card" with no tool call.
    #[test]
    fn prose_that_promises_a_secret_request_is_caught() {
        assert!(promises_secret_request(
            "Arbos, I can make that inference call, but I need the Engy key from your vault first. Using a stored credential needs your approval, so I'll request it now and you'll get an approval card to approve in the chat or in Settings → Vault."
        ));
        assert!(promises_secret_request(
            "Requesting the API key from your vault now."
        ));
        assert!(promises_secret_request(
            "I'll ask for access to the Stripe secret."
        ));
        // Doing it, or talking about something else, is fine.
        assert!(!promises_secret_request(
            "The inference call worked; the model replied hello."
        ));
        assert!(!promises_secret_request(
            "I'll request the weather forecast and get back to you."
        ));
        assert!(!promises_secret_request(
            "You can approve the design in the chat."
        ));
    }

    /// The file every shell sources: the real value for a switched-on grant, the placeholder for
    /// the others, `unset` for names without Access — and for all of them while paused.
    #[test]
    fn env_file_carries_real_values_only_while_allowed() {
        let mut st = State::default();
        let stripe = Grant::from_item(
            "p1",
            &json!({ "env": "STRIPE_KEY", "kind": "key", "enabled": true, "inject_env": true,
            "placeholders": { "value": "arbos-secret://STRIPE_KEY/value" } }),
        )
        .unwrap();
        let mailgun = Grant::from_item(
            "p1",
            &json!({ "env": "MAILGUN_API_KEY", "kind": "key", "enabled": true,
            "placeholders": { "value": "arbos-secret://MAILGUN_API_KEY/value" } }),
        )
        .unwrap();
        st.grants = vec![stripe, mailgun];
        st.known_vars.insert(
            "p1".into(),
            [
                "STRIPE_KEY",
                "MAILGUN_API_KEY",
                "GITHUB_LOGIN_USERNAME",
                "GITHUB_LOGIN_PASSWORD",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
        );
        st.real.insert(
            "p1".into(),
            vec![RealValues {
                name: "STRIPE_KEY".into(),
                vars: [("STRIPE_KEY".to_owned(), "sk_live_it's".to_owned())].into(),
            }],
        );
        st.grants[0].use_notes = vec!["may spend up to $20 without asking".into()];
        let text = render_env_file(&st, "p1", "Research");
        assert!(
            text.contains("export STRIPE_KEY='sk_live_it'\\''s'\n"),
            "{text}"
        );
        assert!(
            text.contains("# STRIPE_KEY (real value): may spend up to $20 without asking\n"),
            "{text}"
        );
        assert!(text.contains("# MAILGUN_API_KEY (placeholder): no note — default money rule"));
        assert!(text.contains("export MAILGUN_API_KEY='arbos-secret://MAILGUN_API_KEY/value'"));
        assert!(text.contains("unset GITHUB_LOGIN_PASSWORD GITHUB_LOGIN_USERNAME\n"));
        assert!(!text.contains("unset STRIPE_KEY"));

        // Switch off (the relay stops sending the value, the grant says so): placeholder again.
        st.grants[0].inject_env = false;
        st.real.insert("p1".into(), Vec::new());
        let text = render_env_file(&st, "p1", "Research");
        assert!(text.contains("export STRIPE_KEY='arbos-secret://STRIPE_KEY/value'"));
        assert!(!text.contains("sk_live"));

        st.paused.push("p1".into());
        let text = render_env_file(&st, "p1", "Research");
        assert!(text.contains(
            "unset GITHUB_LOGIN_PASSWORD GITHUB_LOGIN_USERNAME MAILGUN_API_KEY STRIPE_KEY\n"
        ));
        assert!(!text.contains("export"));
    }
}
