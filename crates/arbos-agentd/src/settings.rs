//! Project settings (`Project.models`, name, icon, instructions, spend cap) kept in sync three ways:
//! relay registry (REST) ⇄ project doc `settings` map (CRDT, what clients read and may write) ⇄ the
//! live pi session (`set_model` / `set_thinking_level`). The box is the only writer of the doc's
//! `chats` index and `status`; clients may write `settings.agentModel` / `agentEffort` (SetModel).

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use serde_json::{json, Value};
use tokio::sync::RwLock;

use arbos_core::rpc::Command as RpcCommand;
use arbos_core::wire::PROJECT_DOC_CHAT_ID;

use crate::agent::Agent;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot {
    pub id: String,
    pub effort: String,
}

/// Who makes the "which agents reply" decision in group chats (`decision_model.provider`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionProvider {
    /// A System One decision model (TypeSafe Jev through OpenRouter's Decisions API): one
    /// calibrated yes/no probability per agent, thresholded. Falls open to `Llm`.
    Jev,
    /// The previous router: the `decision_model.id` chat model answering with JSON.
    Llm,
}

impl DecisionProvider {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "jev" | "decider" | "decision" => Some(Self::Jev),
            "llm" | "chat" => Some(Self::Llm),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Jev => "jev",
            Self::Llm => "llm",
        }
    }
}

/// The responder decision's settings, carried inside the `decision_model` slot JSON next to
/// `id`/`effort` (`provider`, `scorer`, `threshold`) so the relay registry and the project doc
/// keep them without a schema change.
#[derive(Debug, Clone, PartialEq)]
pub struct DecisionConfig {
    pub provider: DecisionProvider,
    /// Decision-model id on the Decisions API (not a chat model; never validated against the
    /// chat catalog).
    pub scorer: String,
    /// An agent replies when its yes-probability is at or above this.
    pub threshold: f64,
    /// Memory tree (D77): `on` injects the walked pack, `log` only records it, `off` disables.
    pub memory_tree: crate::tree::TreeMode,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Models {
    pub talking: Slot,
    pub subagent: Slot,
    pub decision: Slot,
    pub decision_cfg: DecisionConfig,
}

impl Models {
    pub fn from_json(v: &Value, fallback: &Models) -> Models {
        let slot = |k: &str, f: &Slot| Slot {
            id: v
                .pointer(&format!("/{k}/id"))
                .and_then(Value::as_str)
                .unwrap_or(&f.id)
                .to_owned(),
            effort: v
                .pointer(&format!("/{k}/effort"))
                .and_then(Value::as_str)
                .unwrap_or(&f.effort)
                .to_owned(),
        };
        let d = &fallback.decision_cfg;
        let decision_cfg = DecisionConfig {
            provider: v
                .pointer("/decision_model/provider")
                .and_then(Value::as_str)
                .and_then(DecisionProvider::parse)
                .unwrap_or(d.provider),
            scorer: v
                .pointer("/decision_model/scorer")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .unwrap_or(&d.scorer)
                .to_owned(),
            threshold: v
                .pointer("/decision_model/threshold")
                .and_then(Value::as_f64)
                .filter(|t| (0.0..=1.0).contains(t))
                .unwrap_or(d.threshold),
            memory_tree: v
                .pointer("/decision_model/memory_tree")
                .and_then(Value::as_str)
                .and_then(crate::tree::TreeMode::parse)
                .unwrap_or(d.memory_tree),
        };
        Models {
            talking: slot("talking_model", &fallback.talking),
            subagent: slot("subagent_model", &fallback.subagent),
            decision: slot("decision_model", &fallback.decision),
            decision_cfg,
        }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "talking_model": { "id": self.talking.id, "effort": self.talking.effort },
            "subagent_model": { "id": self.subagent.id, "effort": self.subagent.effort },
            "decision_model": {
                "id": self.decision.id, "effort": self.decision.effort,
                "provider": self.decision_cfg.provider.as_str(),
                "scorer": self.decision_cfg.scorer,
                "threshold": self.decision_cfg.threshold,
                "memory_tree": self.decision_cfg.memory_tree.as_str(),
            },
        })
    }
}

/// UI effort words → pi thinking levels (`off minimal low medium high xhigh max`).
pub fn effort_to_thinking(effort: &str) -> &'static str {
    match effort.to_ascii_lowercase().as_str() {
        "off" | "none" => "off",
        "minimal" => "minimal",
        "low" => "low",
        "medium" | "med" => "medium",
        "high" => "high",
        "xhigh" => "xhigh",
        "max" => "max",
        _ => "off",
    }
}

/// GPT-6 Astra / GPT-6.1 Sol / GPT-5 refuse thinking level `off`/`none`/`minimal`.
fn openai_requires_min_effort(model: &str) -> bool {
    let leaf = model
        .rsplit('/')
        .next()
        .unwrap_or(model)
        .to_ascii_lowercase();
    leaf.contains("astra") || leaf.contains("6.1-sol") || leaf.starts_with("gpt-5")
}

fn clamp_effort_for_model(model: &str, effort: &str) -> String {
    if openai_requires_min_effort(model) {
        match effort.to_ascii_lowercase().as_str() {
            "off" | "none" | "minimal" | "" => "low".into(),
            other => other.to_owned(),
        }
    } else {
        effort.to_owned()
    }
}

#[derive(Debug, Default, Clone)]
pub struct SettingsState {
    pub name: String,
    pub icon: String,
    pub color: String,
    /// Tree avatar slug from the registry (`Project.avatar`), mirrored into the doc settings.
    pub avatar: String,
    /// Avatar colour slug (`Project.avatar_color`).
    pub avatar_color: String,
    pub instructions: String,
    pub spend_cap_usd: f64,
    /// Visible note when a configured model id was unknown and a default took its place.
    pub model_note: String,
    /// Last `models` JSON seen from the registry poll (validated once per distinct value).
    pub last_registry_models: Option<Models>,
    /// Last `settings` JSON the box wrote into the project doc; used to tell client writes from echoes.
    pub last_doc_write: Option<Value>,
    pub last_chats_write: Option<Value>,
}

/// Model ids the proxy serves (`GET /proxy/v1/models`), refreshed every `CATALOG_TTL`.
pub struct ModelCatalog {
    pub ids: HashSet<String>,
    pub fetched_at: Instant,
}

const CATALOG_TTL: Duration = Duration::from_secs(10 * 60);

/// Safe slots when a configured id is not in the catalog (the same defaults `Config` uses).
pub const DEFAULT_TALKING_MODEL: &str = "anthropic/claude-haiku-5.5";
pub const DEFAULT_SUBAGENT_MODEL: &str = "anthropic/claude-sonnet-5.5";
pub const DEFAULT_DECISION_MODEL: &str = "anthropic/claude-haiku-5.5";

pub struct Settings {
    pub models: RwLock<Models>,
    pub state: RwLock<SettingsState>,
    pub catalog: RwLock<Option<ModelCatalog>>,
    /// Boot-time slots: the fallback when a client or the registry names an unknown model.
    pub defaults: Models,
}

impl Settings {
    pub fn new(cfg: &crate::config::Config) -> Self {
        let models = Models {
            talking: Slot {
                id: cfg.talking_model.clone(),
                effort: "off".into(),
            },
            subagent: Slot {
                id: cfg.subagent_model.clone(),
                effort: "medium".into(),
            },
            decision: Slot {
                id: cfg.decision_model.clone(),
                effort: "off".into(),
            },
            decision_cfg: cfg.decision_cfg.clone(),
        };
        Self {
            models: RwLock::new(models.clone()),
            state: RwLock::new(SettingsState {
                name: cfg.project_name.clone(),
                instructions: cfg.instructions.clone(),
                spend_cap_usd: 25.0,
                ..Default::default()
            }),
            catalog: RwLock::new(None),
            defaults: models,
        }
    }
}

/// A model id the proxy can serve has the `vendor/name` shape and no spaces; picker labels such
/// as "Claude Fable 5.1" never do.
pub fn plausible_model_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 120
        && id.contains('/')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '-' | '.' | '_' | ':'))
}

impl Agent {
    /// Known model ids from the proxy, cached for `CATALOG_TTL`. `None` when the proxy cannot be
    /// asked right now (validation then falls back to the shape check only).
    async fn known_model_ids(self: &Arc<Self>) -> Option<HashSet<String>> {
        if let Some(c) = self.settings.catalog.read().await.as_ref() {
            if c.fetched_at.elapsed() < CATALOG_TTL {
                return Some(c.ids.clone());
            }
        }
        let r = self
            .http_client()
            .get(format!("{}/proxy/v1/models", self.cfg.relay_url))
            .bearer_auth(&self.cfg.box_token)
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .ok()?;
        if !r.status().is_success() {
            return None;
        }
        let v: Value = r.json().await.ok()?;
        let ids: HashSet<String> = v
            .get("data")
            .and_then(Value::as_array)?
            .iter()
            .filter_map(|m| m.get("id").and_then(Value::as_str))
            .map(str::to_owned)
            .collect();
        if ids.is_empty() {
            return None;
        }
        *self.settings.catalog.write().await = Some(ModelCatalog {
            ids: ids.clone(),
            fetched_at: Instant::now(),
        });
        Some(ids)
    }

    /// Replace unknown model ids with the defaults. Returns the cleaned slots and one note per
    /// replaced slot (shown to the user through `settings.modelNote`).
    async fn sanitize_models(self: &Arc<Self>, wanted: &Models) -> (Models, Vec<String>) {
        let known = self.known_model_ids().await;
        let valid = |id: &str| -> bool {
            plausible_model_id(id) && known.as_ref().is_none_or(|k| k.contains(id))
        };
        let mut out = wanted.clone();
        let mut notes = Vec::new();
        let mut fix = |label: &str, slot: &mut Slot, default: &str| {
            if !valid(&slot.id) {
                notes.push(format!(
                    "Unknown model \"{}\" for {label}; using {default}.",
                    slot.id
                ));
                slot.id = default.to_owned();
            }
        };
        let d = &self.settings.defaults;
        let pick = |boot: &str, hard: &str| -> String {
            if valid(boot) {
                boot.to_owned()
            } else {
                hard.to_owned()
            }
        };
        let talking_default = pick(&d.talking.id, DEFAULT_TALKING_MODEL);
        let subagent_default = pick(&d.subagent.id, DEFAULT_SUBAGENT_MODEL);
        let decision_default = pick(&d.decision.id, DEFAULT_DECISION_MODEL);
        fix("the talking model", &mut out.talking, &talking_default);
        fix("the subagent model", &mut out.subagent, &subagent_default);
        fix("the decision model", &mut out.decision, &decision_default);
        // OpenAI Astra (and kin) reject reasoning_effort none/off — bump before apply.
        out.talking.effort = clamp_effort_for_model(&out.talking.id, &out.talking.effort);
        out.subagent.effort = clamp_effort_for_model(&out.subagent.id, &out.subagent.effort);
        out.decision.effort = clamp_effort_for_model(&out.decision.id, &out.decision.effort);
        (out, notes)
    }

    /// Validate, apply live, and converge the registry + doc on the cleaned slots.
    async fn adopt_models(self: &Arc<Self>, wanted: &Models, source: &str) -> Result<()> {
        let (clean, notes) = self.sanitize_models(wanted).await;
        if !notes.is_empty() {
            tracing::warn!(source, ?notes, "model ids rejected");
        }
        {
            let mut st = self.settings.state.write().await;
            st.model_note = notes.join(" ");
        }
        let current = self.settings.models.read().await.clone();
        if clean != current {
            self.apply_models(&clean).await?;
        }
        if clean != *wanted || source == "doc" {
            // The bad value must leave the registry too, or the next poll brings it back.
            let _ = self
                .http_client()
                .patch(format!(
                    "{}/projects/{}",
                    self.cfg.relay_url, self.cfg.project_id
                ))
                .bearer_auth(&self.cfg.box_token)
                .json(&json!({ "models": clean.to_json() }))
                .send()
                .await;
        }
        Ok(())
    }
    /// The relay registry view arrived (boot hello or the 3 s poll): apply models and mirror into the doc.
    pub async fn settings_from_relay(
        self: &Arc<Self>,
        project: &Value,
        chats: &[Value],
    ) -> Result<()> {
        let current = self.settings.models.read().await.clone();
        let incoming = project
            .get("models")
            .map(|m| Models::from_json(m, &current))
            .unwrap_or_else(|| current.clone());
        let last_seen = self
            .settings
            .state
            .read()
            .await
            .last_registry_models
            .clone();
        if incoming != current && last_seen.as_ref() != Some(&incoming) {
            // Validate once per distinct registry value: a rejected id is PATCHed away, and until
            // that lands the poll must not re-apply (or re-reject) the same stale JSON.
            self.settings.state.write().await.last_registry_models = Some(incoming.clone());
            self.adopt_models(&incoming, "registry").await?;
        }
        let mut prompt_changed = false;
        {
            let mut st = self.settings.state.write().await;
            if let Some(n) = project.get("name").and_then(Value::as_str) {
                st.name = n.to_owned();
            }
            if let Some(i) = project.get("instructions").and_then(Value::as_str) {
                prompt_changed |= st.instructions.trim() != i.trim();
            }
            if let Some(i) = project.get("icon").and_then(Value::as_str) {
                st.icon = i.to_owned();
            }
            if let Some(c) = project.get("color").and_then(Value::as_str) {
                st.color = c.to_owned();
            }
            if let Some(a) = project.get("avatar").and_then(Value::as_str) {
                st.avatar = a.to_owned();
            }
            if let Some(a) = project.get("avatar_color").and_then(Value::as_str) {
                st.avatar_color = a.to_owned();
            }
            if let Some(i) = project.get("instructions").and_then(Value::as_str) {
                st.instructions = i.to_owned();
            }
            if let Some(c) = project.get("spend_cap_usd").and_then(Value::as_f64) {
                st.spend_cap_usd = c;
            }
        }
        self.write_settings_doc().await;
        self.write_chats_index(chats).await;
        // Identity (#1431): the relay's name is the agent's name, at every poll.
        if let Some(name) = project.get("name").and_then(Value::as_str) {
            if let Some(r) = self.identity.observe(name) {
                self.on_renamed(&r).await;
                prompt_changed = false;
            }
        }
        if prompt_changed {
            self.refresh_session_prompt("instructions changed").await;
        }
        Ok(())
    }

    /// The relay renamed this project while the box runs: every place that spoke the old name
    /// now speaks the new one on the next turn (system prompt, owner note, group prelude, quick
    /// reply), people-chat `@slug` / member handles stay addressable under the new name, the
    /// memory tree gets a one-line fact so later walks attribute the old name to this agent, and
    /// the pi session re-reads its prompt.
    pub async fn on_renamed(self: &Arc<Self>, r: &crate::identity::Rename) {
        tracing::info!(from = %r.from, to = %r.to, "project renamed; identity refreshed");
        self.append_audit(
            "renamed",
            json!({ "from": r.from, "to": r.to, "handle": self.identity.handle(), "at": r.at_ms }),
        );
        let note = format!(
            "This agent was renamed from \"{}\" to \"{}\"; notes and messages that say \"{}\" refer to it. It introduces itself as \"{}\".",
            r.from, r.to, r.from, r.to
        );
        if let Err(e) = self.store.memory_add(
            "T1",
            None,
            "project",
            "project",
            &note,
            Some(&json!({ "kind": "rename", "from": r.from, "to": r.to })),
            Some(r.at_ms),
            1.0,
            9,
            &[],
        ) {
            tracing::warn!(error = %e, "rename note into memory");
        }
        let _ = self.store.t3_append(
            PROJECT_DOC_CHAT_ID,
            None,
            None,
            "renamed",
            Some("system"),
            &note,
            Some(&json!({ "from": r.from, "to": r.to })),
        );
        // People chats snapshotted the old @handle at join time; without this, @newname would
        // not wake the agent and the prelude would still say @oldname (#1481).
        self.refresh_people_identity_after_rename();
        // The prompt's memory block is debounced (D87); a rename is worth the prefix rewrite.
        if let Ok(mut held) = self.prompt_memory.lock() {
            *held = None;
        }
        self.refresh_session_prompt("renamed").await;
    }

    /// Rewrite the system prompt file and have the live pi session read it: the supervisor loop
    /// restarts the session at the next turn boundary once the change has settled
    /// (`schedule_prompt_refresh`, #2007) — never under a running or starting turn.
    pub async fn refresh_session_prompt(self: &Arc<Self>, why: &str) {
        if let Err(e) = self.write_system_prompt() {
            tracing::warn!(error = %e, why, "system prompt rewrite");
            return;
        }
        // The prompt file is current now; the session re-reads it when the supervisor
        // restarts it — at the next turn boundary, after a quiet window (#2007). A turn that
        // starts first runs on the previous prompt, which the stable-prefix model tolerates.
        self.schedule_prompt_refresh(why);
    }

    /// Switch the live pi session's model / thinking level and remember the new slots.
    pub async fn apply_models(self: &Arc<Self>, m: &Models) -> Result<()> {
        let prev = self.settings.models.read().await.clone();
        // Remember first so a respawn / system-prompt rewrite reads the new slots.
        *self.settings.models.write().await = m.clone();
        // The new models need their context-window override before pi (re)reads models.json.
        if let Err(e) =
            crate::workspace::write_models_json(&self.cfg, &m.talking.id, &m.subagent.id)
        {
            tracing::warn!(error = %e, "models.json rewrite after a model change");
        }
        let mut switched = prev.talking.id == m.talking.id;
        if let Ok(pi) = self.pi_child().await {
            if prev.talking.id != m.talking.id {
                let r = pi
                    .call(RpcCommand::SetModel {
                        id: pi.next_id("sm"),
                        provider: self.cfg.pi_provider(),
                        model_id: m.talking.id.clone(),
                    })
                    .await?;
                if r.success {
                    switched = true;
                } else {
                    // pi's in-session catalog may not list the new id yet; respawn
                    // with live_models() so the next turn actually uses it.
                    tracing::warn!(model = %m.talking.id, error = ?r.error, "set_model refused; respawning pi");
                    if let Err(e) = self.respawn_pi_current().await {
                        tracing::warn!(error = %e, "pi respawn after set_model refused");
                    } else {
                        switched = true;
                    }
                }
            }
            if prev.talking.effort != m.talking.effort {
                if let Ok(pi) = self.pi_child().await {
                    let _ = pi
                        .call(RpcCommand::SetThinkingLevel {
                            id: pi.next_id("tl"),
                            level: effort_to_thinking(&m.talking.effort).into(),
                        })
                        .await;
                }
            }
        } else if prev.talking.id != m.talking.id {
            // No live pi yet; the next start_pi/respawn picks up live_models().
            switched = true;
        }
        // The router prompt names the talking model under "This project"; rewrite
        // so a picker change is what the agent reports on the next turn.
        if let Err(e) = self.write_system_prompt() {
            tracing::warn!(error = %e, "rewrite system prompt after model change");
        }
        tracing::info!(
            talking = %m.talking.id,
            effort = %m.talking.effort,
            subagent = %m.subagent.id,
            switched,
            "models applied"
        );
        Ok(())
    }

    /// Mirror the registry settings into the project doc (`settings` map, crdt-sync §6.3).
    pub async fn write_settings_doc(self: &Arc<Self>) {
        let m = self.settings.models.read().await.clone();
        let st = self.settings.state.read().await.clone();
        let v = json!({
            "name": st.name, "icon": st.icon, "color": st.color, "avatar": st.avatar, "avatarColor": st.avatar_color, "instructions": st.instructions,
            "agentModel": m.talking.id, "agentEffort": m.talking.effort,
            "defaultSubagentModel": m.subagent.id, "defaultSubagentEffort": m.subagent.effort,
            "decisionModel": m.decision.id, "decisionProvider": m.decision_cfg.provider.as_str(),
            "decisionScorer": m.decision_cfg.scorer, "decisionThreshold": m.decision_cfg.threshold,
            "spendCapUsd": st.spend_cap_usd,
            "modelNote": st.model_note,
        });
        if self.settings.state.read().await.last_doc_write.as_ref() == Some(&v) {
            return;
        }
        let Some(h) = self.sync.host(PROJECT_DOC_CHAT_ID).await else {
            return;
        };
        {
            let h = h.lock().await;
            let settings = h.doc.doc.get_map("settings");
            if let Some(o) = v.as_object() {
                for (k, val) in o {
                    let lv: loro::LoroValue =
                        serde_json::from_value(val.clone()).unwrap_or(loro::LoroValue::Null);
                    settings.insert(k, lv).ok();
                }
            }
            h.doc.doc.commit();
        }
        self.settings.state.write().await.last_doc_write = Some(v);
    }

    /// `chats` index in the project doc (box is the only writer); rewritten only when it changed.
    pub async fn write_chats_index(self: &Arc<Self>, chats: &[Value]) {
        self.note_chats_index(chats);
        let v = Value::Array(chats.to_vec());
        if self.settings.state.read().await.last_chats_write.as_ref() == Some(&v) {
            return;
        }
        let Some(h) = self.sync.host(PROJECT_DOC_CHAT_ID).await else {
            return;
        };
        {
            let h = h.lock().await;
            let list = h.doc.doc.get_movable_list("chats");
            let n = list.len();
            if n > 0 {
                list.delete(0, n).ok();
            }
            for c in chats {
                let entry = json!({ "id": c.get("id"), "title": c.get("title"), "isPinned": c.get("is_pinned"), "isSystem": c.get("is_system").and_then(Value::as_bool).unwrap_or(false), "lastMessageAt": c.get("last_message_at"), "position": c.get("position"), "kind": c.get("kind").and_then(Value::as_str).unwrap_or("chat"), "participants": c.get("participants").cloned().unwrap_or_else(|| json!([])) });
                let lv: loro::LoroValue =
                    serde_json::from_value(entry).unwrap_or(loro::LoroValue::Null);
                list.push(lv).ok();
            }
            h.doc.doc.commit();
        }
        self.settings.state.write().await.last_chats_write = Some(v);
    }

    /// A client wrote into the project doc: pick up `settings.agentModel` / `agentEffort` (SetModel)
    /// and push it to the registry so every peer converges.
    pub async fn settings_from_doc(self: &Arc<Self>) {
        let Some(h) = self.sync.host(PROJECT_DOC_CHAT_ID).await else {
            return;
        };
        let settings: Value = {
            let h = h.lock().await;
            serde_json::to_value(h.doc.doc.get_map("settings").get_deep_value())
                .unwrap_or(Value::Null)
        };
        let current = self.settings.models.read().await.clone();
        let model = settings
            .get("agentModel")
            .and_then(Value::as_str)
            .unwrap_or(&current.talking.id)
            .to_owned();
        let effort = settings
            .get("agentEffort")
            .and_then(Value::as_str)
            .unwrap_or(&current.talking.effort)
            .to_owned();
        let sub_model = settings
            .get("defaultSubagentModel")
            .and_then(Value::as_str)
            .unwrap_or(&current.subagent.id)
            .to_owned();
        let sub_effort = settings
            .get("defaultSubagentEffort")
            .and_then(Value::as_str)
            .unwrap_or(&current.subagent.effort)
            .to_owned();
        let wanted = Models {
            talking: Slot { id: model, effort },
            subagent: Slot {
                id: sub_model,
                effort: sub_effort,
            },
            decision: current.decision.clone(),
            decision_cfg: current.decision_cfg.clone(),
        };
        if wanted == current {
            return;
        }
        tracing::info!(?wanted, "settings change from a client in the project doc");
        // `decision` stays as it is: a chat picker only ever selects the talking/subagent slots.
        if let Err(e) = self.adopt_models(&wanted, "doc").await {
            tracing::warn!(error = %e, "apply models from doc");
        }
        // Overwrite the client's write with the validated value (an echo of a bad id would loop).
        self.settings.state.write().await.last_doc_write = None;
        self.write_settings_doc().await;
    }
}
