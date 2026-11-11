//! Storage for agent voice calls (Calls Phase 2, D75): one usage row per (call, agent), the
//! judge's decisions in group calls, agents present in a call, and per-user / per-agent voice
//! settings. Same SQLite file; created from `Db::open` through `VOICE_SCHEMA`.

use anyhow::Result;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use arbos_core::ids::now_ms;

use crate::db::Db;

pub const VOICE_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS voice_calls (
  call_id TEXT NOT NULL, project_id TEXT NOT NULL, payer TEXT NOT NULL, mode TEXT,
  started_at INTEGER NOT NULL, ended_at INTEGER, updated_at INTEGER NOT NULL,
  seconds REAL NOT NULL DEFAULT 0,
  audio_in_tokens INTEGER NOT NULL DEFAULT 0, audio_out_tokens INTEGER NOT NULL DEFAULT 0,
  text_in_tokens INTEGER NOT NULL DEFAULT 0, text_out_tokens INTEGER NOT NULL DEFAULT 0,
  cached_tokens INTEGER NOT NULL DEFAULT 0, stt_seconds REAL NOT NULL DEFAULT 0,
  decisions INTEGER NOT NULL DEFAULT 0, tool_calls INTEGER NOT NULL DEFAULT 0,
  turns_spoken INTEGER NOT NULL DEFAULT 0, usd REAL NOT NULL DEFAULT 0, usd_charged REAL NOT NULL DEFAULT 0,
  ttfa_ms_p50 INTEGER, ttfa_ms_p95 INTEGER, barge_ms_p50 INTEGER, barge_ms_max INTEGER,
  stop_reason TEXT,
  PRIMARY KEY (call_id, project_id)
);
CREATE INDEX IF NOT EXISTS voice_calls_started ON voice_calls(started_at);
CREATE TABLE IF NOT EXISTS voice_decisions (
  id INTEGER PRIMARY KEY AUTOINCREMENT, call_id TEXT NOT NULL, project_id TEXT NOT NULL, at INTEGER NOT NULL,
  speaker TEXT, text TEXT, decision TEXT NOT NULL, forced INTEGER NOT NULL DEFAULT 0,
  p_speak REAL, p_act REAL, p_silent REAL, provider TEXT, latency_ms INTEGER, cost_usd REAL
);
CREATE INDEX IF NOT EXISTS voice_decisions_call ON voice_decisions(call_id, at);
CREATE TABLE IF NOT EXISTS call_agents (
  call_id TEXT NOT NULL, project_id TEXT NOT NULL, joined_at INTEGER NOT NULL, left_at INTEGER,
  PRIMARY KEY (call_id, project_id)
);
CREATE TABLE IF NOT EXISTS voice_user_settings (
  user_id TEXT PRIMARY KEY, daily_cap_usd REAL
);
CREATE TABLE IF NOT EXISTS voice_agent_settings (
  project_id TEXT PRIMARY KEY, chattiness TEXT NOT NULL DEFAULT 'balanced', wake_name TEXT
);
CREATE TABLE IF NOT EXISTS voice_worker (
  id INTEGER PRIMARY KEY CHECK (id = 1), info TEXT NOT NULL, at INTEGER NOT NULL
);
"#;

/// Columns added after the first voice release (ALTER is idempotent through the error).
pub fn migrate_voice(c: &rusqlite::Connection) {
    let _ = c.execute("ALTER TABLE voice_agent_settings ADD COLUMN voice TEXT", []);
    // Camera vision: owner toggle, and the image part of the bill per (call, agent).
    let _ = c.execute(
        "ALTER TABLE voice_agent_settings ADD COLUMN vision_enabled INTEGER NOT NULL DEFAULT 1",
        [],
    );
    for col in [
        "image_in_tokens INTEGER NOT NULL DEFAULT 0",
        "frames INTEGER NOT NULL DEFAULT 0",
        "looks INTEGER NOT NULL DEFAULT 0",
        "vision_seconds REAL NOT NULL DEFAULT 0",
        "vision_usd REAL NOT NULL DEFAULT 0",
        "vision_charged REAL NOT NULL DEFAULT 0",
    ] {
        let _ = c.execute(&format!("ALTER TABLE voice_calls ADD COLUMN {col}"), []);
    }
}

/// What the worker reports for one agent in one call (cumulative since the call began).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VoiceUsage {
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub seconds: f64,
    #[serde(default)]
    pub audio_in_tokens: i64,
    #[serde(default)]
    pub audio_out_tokens: i64,
    #[serde(default)]
    pub text_in_tokens: i64,
    #[serde(default)]
    pub text_out_tokens: i64,
    #[serde(default)]
    pub cached_tokens: i64,
    /// Camera vision (a subset of the realtime bill): image tokens, frames and `look`s sent,
    /// camera-on seconds, and the image-token dollars.
    #[serde(default)]
    pub image_in_tokens: i64,
    #[serde(default)]
    pub frames: i64,
    #[serde(default)]
    pub looks: i64,
    #[serde(default)]
    pub vision_seconds: f64,
    #[serde(default)]
    pub vision_usd: f64,
    #[serde(default)]
    pub stt_seconds: f64,
    #[serde(default)]
    pub decisions: i64,
    #[serde(default)]
    pub tool_calls: i64,
    #[serde(default)]
    pub turns_spoken: i64,
    #[serde(default)]
    pub usd: f64,
    #[serde(default)]
    pub ttfa_ms_p50: Option<i64>,
    #[serde(default)]
    pub ttfa_ms_p95: Option<i64>,
    #[serde(default)]
    pub barge_ms_p50: Option<i64>,
    #[serde(default)]
    pub barge_ms_max: Option<i64>,
    /// Set on the last report of the call.
    #[serde(default)]
    pub ended: bool,
    #[serde(default)]
    pub stop_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct VoiceCallRow {
    pub call_id: String,
    pub project_id: String,
    pub payer: String,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub updated_at: i64,
    pub usage: VoiceUsage,
    /// Cumulative reported dollars settled, including policy refusals/clamps; ledger debits may be lower.
    pub usd_charged: f64,
    /// The vision portion of reported settlement; its ledger entry may be clamped or refused.
    pub vision_charged: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VoiceDecision {
    pub speaker: Option<String>,
    pub text: Option<String>,
    pub decision: String,
    #[serde(default)]
    pub forced: bool,
    pub p_speak: Option<f64>,
    pub p_act: Option<f64>,
    pub p_silent: Option<f64>,
    pub provider: Option<String>,
    pub latency_ms: Option<i64>,
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct VoiceAgentSettings {
    pub project_id: String,
    pub chattiness: String,
    pub wake_name: Option<String>,
    /// Speech-model voice id (`GET /voices`); `None` = the relay default.
    pub voice: Option<String>,
    /// May the agent see cameras on calls (default true)?
    pub vision_enabled: bool,
}

pub const CHATTINESS_LEVELS: [&str; 3] = ["quiet", "balanced", "chatty"];

fn voice_call_on(
    c: &rusqlite::Connection,
    call_id: &str,
    project_id: &str,
) -> rusqlite::Result<Option<VoiceCallRow>> {
    c.query_row(
        &format!("SELECT {VOICE_COLS} FROM voice_calls WHERE call_id = ?1 AND project_id = ?2"),
        params![call_id, project_id],
        voice_from_row,
    )
    .optional()
}

fn voice_usage_on(
    c: &rusqlite::Connection,
    call_id: &str,
    project_id: &str,
    payer: &str,
    started_at: i64,
    u: &VoiceUsage,
) -> rusqlite::Result<Option<VoiceCallRow>> {
    let now = now_ms();
    c.execute(
    "INSERT INTO voice_calls(call_id, project_id, payer, started_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)
     ON CONFLICT(call_id, project_id) DO NOTHING",
    params![call_id, project_id, payer, started_at, now],
)?;
    let updated = c.execute(
    "UPDATE voice_calls SET mode = COALESCE(?3, mode), updated_at = ?4,
       seconds = MAX(seconds, ?5), audio_in_tokens = MAX(audio_in_tokens, ?6), audio_out_tokens = MAX(audio_out_tokens, ?7),
       text_in_tokens = MAX(text_in_tokens, ?8), text_out_tokens = MAX(text_out_tokens, ?9), cached_tokens = MAX(cached_tokens, ?10),
       stt_seconds = MAX(stt_seconds, ?11), decisions = MAX(decisions, ?12), tool_calls = MAX(tool_calls, ?13),
       turns_spoken = MAX(turns_spoken, ?14), usd = MAX(usd, ?15),
       ttfa_ms_p50 = COALESCE(?16, ttfa_ms_p50), ttfa_ms_p95 = COALESCE(?17, ttfa_ms_p95),
       barge_ms_p50 = COALESCE(?18, barge_ms_p50), barge_ms_max = COALESCE(?19, barge_ms_max),
       ended_at = CASE WHEN ?20 THEN COALESCE(ended_at, ?4) ELSE ended_at END,
       stop_reason = COALESCE(?21, stop_reason),
       image_in_tokens = MAX(image_in_tokens, ?22), frames = MAX(frames, ?23), looks = MAX(looks, ?24),
       vision_seconds = MAX(vision_seconds, ?25), vision_usd = MAX(vision_usd, ?26)
     WHERE call_id = ?1 AND project_id = ?2",
    params![
        call_id, project_id, u.mode, now, u.seconds, u.audio_in_tokens, u.audio_out_tokens,
        u.text_in_tokens, u.text_out_tokens, u.cached_tokens, u.stt_seconds, u.decisions,
        u.tool_calls, u.turns_spoken, u.usd, u.ttfa_ms_p50, u.ttfa_ms_p95, u.barge_ms_p50,
        u.barge_ms_max, u.ended, u.stop_reason, u.image_in_tokens, u.frames, u.looks,
        u.vision_seconds, u.vision_usd
    ],
)?;
    if updated != 1 {
        return Err(rusqlite::Error::QueryReturnedNoRows);
    }
    voice_call_on(c, call_id, project_id)
}

pub(crate) struct VoiceBilling {
    pub metered: bool,
    pub legacy: crate::db_accounts::LegacyCreditDefaults,
}

/// One committed cumulative settlement; reported totals may exceed clamped ledger dollars.
pub(crate) struct VoiceSettlement {
    pub reported_total: f64,
    pub reported_delta: f64,
    pub charges: Vec<(f64, crate::db_accounts::PostfactCharge)>,
}

impl Db {
    /// Observe cumulative usage, apply the existing postfact debit policy, and settle once.
    pub(crate) fn settle_voice_usage(
        &self,
        call_id: &str,
        project_id: &str,
        payer: &str,
        started_at: i64,
        usage: &VoiceUsage,
        billing: &VoiceBilling,
    ) -> Result<VoiceSettlement> {
        self.with(|c| {
            let transaction = rusqlite::Transaction::new_unchecked(c, rusqlite::TransactionBehavior::Immediate)?;
            let row = voice_usage_on(&transaction, call_id, project_id, payer, started_at, usage)?
                .ok_or(rusqlite::Error::QueryReturnedNoRows)?;
            let delta = row.usage.usd - row.usd_charged;
            if delta <= 0.000_001 {
                transaction.commit()?;
                return Ok(VoiceSettlement { reported_total: row.usd_charged, reported_delta: 0.0, charges: Vec::new() });
            }
            let vision_delta = (row.usage.vision_usd - row.vision_charged).clamp(0.0, delta);
            let voice_delta = delta - vision_delta;
            let mut charges = Vec::new();
            let reference = format!("call:{call_id}");
            // Preserve voice-before-vision and the strict per-part threshold, including refusals.
            if billing.metered {
                if voice_delta > 0.000_001 {
                    if let Some(charged) = crate::db_accounts::charge_postfact_on(&transaction, payer, voice_delta, "voice", Some(&reference), &billing.legacy)? {
                        charges.push((voice_delta, charged));
                    }
                }
                if vision_delta > 0.000_001 {
                    let vision_ref = format!("{reference}#vision");
                    if let Some(charged) = crate::db_accounts::charge_postfact_on(&transaction, payer, vision_delta, "voice", Some(&vision_ref), &billing.legacy)? {
                        charges.push((vision_delta, charged));
                    }
                }
            }
            let updated = transaction.execute(
                "UPDATE voice_calls SET usd_charged = usd_charged + ?3, vision_charged = vision_charged + ?4
                 WHERE call_id = ?1 AND project_id = ?2",
                params![call_id, project_id, delta, vision_delta],
            )?;
            if updated != 1 { return Err(rusqlite::Error::QueryReturnedNoRows); }
            transaction.commit()?;
            Ok(VoiceSettlement { reported_total: row.usd_charged + delta, reported_delta: delta, charges })
        })
    }

    /// Camera-on minutes and image dollars over every agent call since `since_ms`.
    pub fn voice_vision_totals(&self, since_ms: i64) -> Result<(f64, f64, i64)> {
        self.with(|c| {
            c.query_row(
                "SELECT COALESCE(SUM(vision_seconds), 0.0) / 60.0, COALESCE(SUM(vision_usd), 0.0), COALESCE(SUM(frames), 0)
                 FROM voice_calls WHERE started_at >= ?1",
                params![since_ms],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
        })
    }

    pub fn voice_call(&self, call_id: &str, project_id: &str) -> Result<Option<VoiceCallRow>> {
        self.with(|c| voice_call_on(c, call_id, project_id))
    }

    /// Every agent's usage row of one call.
    pub fn voice_calls_of(&self, call_id: &str) -> Result<Vec<VoiceCallRow>> {
        self.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {VOICE_COLS} FROM voice_calls WHERE call_id = ?1 ORDER BY started_at"
            ))?;
            let rows = st.query_map(params![call_id], voice_from_row)?;
            rows.collect()
        })
    }

    pub fn voice_calls_recent(&self, limit: i64) -> Result<Vec<VoiceCallRow>> {
        self.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {VOICE_COLS} FROM voice_calls ORDER BY started_at DESC LIMIT ?1"
            ))?;
            let rows = st.query_map(params![limit], voice_from_row)?;
            rows.collect()
        })
    }

    /// What the user's agents spent on voice since `since_ms` (ledger debits of kind `voice`).
    pub fn voice_spent_since(&self, user_id: &str, since_ms: i64) -> Result<f64> {
        self.with(|c| {
            c.query_row(
                "SELECT COALESCE(-SUM(usd), 0.0) FROM credit_ledger WHERE user_id = ?1 AND kind = 'voice' AND usd < 0 AND at >= ?2",
                params![user_id, since_ms],
                |r| r.get(0),
            )
        })
    }

    pub fn voice_decision_insert(
        &self,
        call_id: &str,
        project_id: &str,
        at: i64,
        d: &VoiceDecision,
    ) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO voice_decisions(call_id, project_id, at, speaker, text, decision, forced, p_speak, p_act, p_silent, provider, latency_ms, cost_usd)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    call_id, project_id, at, d.speaker, d.text, d.decision, d.forced, d.p_speak, d.p_act,
                    d.p_silent, d.provider, d.latency_ms, d.cost_usd
                ],
            )
            .map(|_| ())
        })
    }

    pub fn voice_decisions_of(&self, call_id: &str, limit: i64) -> Result<Vec<Value>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT at, project_id, speaker, text, decision, forced, p_speak, p_act, p_silent, provider, latency_ms, cost_usd
                 FROM voice_decisions WHERE call_id = ?1 ORDER BY at DESC LIMIT ?2",
            )?;
            let rows = st.query_map(params![call_id, limit], |r| {
                Ok(serde_json::json!({
                    "at": r.get::<_, i64>(0)?, "project_id": r.get::<_, String>(1)?,
                    "speaker": r.get::<_, Option<String>>(2)?, "text": r.get::<_, Option<String>>(3)?,
                    "decision": r.get::<_, String>(4)?, "forced": r.get::<_, i64>(5)? != 0,
                    "p_speak": r.get::<_, Option<f64>>(6)?, "p_act": r.get::<_, Option<f64>>(7)?,
                    "p_silent": r.get::<_, Option<f64>>(8)?, "provider": r.get::<_, Option<String>>(9)?,
                    "latency_ms": r.get::<_, Option<i64>>(10)?, "cost_usd": r.get::<_, Option<f64>>(11)?,
                }))
            })?;
            rows.collect()
        })
    }

    // ---- agents in a call -----------------------------------------------------------------------

    pub fn call_agent_join(&self, call_id: &str, project_id: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO call_agents(call_id, project_id, joined_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(call_id, project_id) DO UPDATE SET left_at = NULL",
                params![call_id, project_id, now_ms()],
            )
            .map(|_| ())
        })
    }

    pub fn call_agent_leave(&self, call_id: &str, project_id: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE call_agents SET left_at = ?3 WHERE call_id = ?1 AND project_id = ?2 AND left_at IS NULL",
                params![call_id, project_id, now_ms()],
            )
            .map(|_| ())
        })
    }

    /// Agents in the call right now.
    pub fn call_agents_live(&self, call_id: &str) -> Result<Vec<String>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT project_id FROM call_agents WHERE call_id = ?1 AND left_at IS NULL ORDER BY joined_at",
            )?;
            let rows = st.query_map(params![call_id], |r| r.get(0))?;
            rows.collect()
        })
    }

    // ---- worker hello (survives relay restarts) ------------------------------------------------------

    pub fn set_voice_worker(&self, info: &Value) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO voice_worker(id, info, at) VALUES (1, ?1, ?2)
                 ON CONFLICT(id) DO UPDATE SET info = excluded.info, at = excluded.at",
                params![info.to_string(), now_ms()],
            )
            .map(|_| ())
        })
    }

    pub fn voice_worker(&self) -> Result<Option<Value>> {
        self.with(|c| {
            c.query_row("SELECT info FROM voice_worker WHERE id = 1", [], |r| {
                r.get::<_, String>(0)
            })
            .optional()
            .map(|s| s.and_then(|s| serde_json::from_str(&s).ok()))
        })
    }

    // ---- settings ---------------------------------------------------------------------------------

    pub fn voice_agent_settings(&self, project_id: &str) -> Result<VoiceAgentSettings> {
        self.with(|c| {
            let row = c
                .query_row(
                    "SELECT chattiness, wake_name, voice, vision_enabled FROM voice_agent_settings WHERE project_id = ?1",
                    params![project_id],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, Option<String>>(1)?,
                            r.get::<_, Option<String>>(2)?,
                            r.get::<_, i64>(3)? != 0,
                        ))
                    },
                )
                .optional()?;
            let (chattiness, wake_name, voice, vision_enabled) =
                row.unwrap_or_else(|| ("balanced".to_owned(), None, None, true));
            Ok(VoiceAgentSettings {
                project_id: project_id.to_owned(),
                chattiness,
                wake_name,
                voice,
                vision_enabled,
            })
        })
    }

    /// The agent's call voice (`None` = default).
    pub fn set_voice_agent_voice(&self, project_id: &str, voice: Option<&str>) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO voice_agent_settings(project_id, voice) VALUES (?1, ?2)
                 ON CONFLICT(project_id) DO UPDATE SET voice = excluded.voice",
                params![project_id, voice],
            )
            .map(|_| ())
        })
    }

    /// May the agent see cameras on calls?
    pub fn set_voice_agent_vision(&self, project_id: &str, enabled: bool) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO voice_agent_settings(project_id, vision_enabled) VALUES (?1, ?2)
                 ON CONFLICT(project_id) DO UPDATE SET vision_enabled = excluded.vision_enabled",
                params![project_id, enabled],
            )
            .map(|_| ())
        })
    }

    pub fn set_voice_agent_settings(
        &self,
        project_id: &str,
        chattiness: Option<&str>,
        wake_name: Option<Option<&str>>,
    ) -> Result<VoiceAgentSettings> {
        self.with(|c| {
            c.execute(
                "INSERT INTO voice_agent_settings(project_id) VALUES (?1) ON CONFLICT(project_id) DO NOTHING",
                params![project_id],
            )?;
            if let Some(ch) = chattiness {
                c.execute(
                    "UPDATE voice_agent_settings SET chattiness = ?2 WHERE project_id = ?1",
                    params![project_id, ch],
                )?;
            }
            if let Some(w) = wake_name {
                c.execute(
                    "UPDATE voice_agent_settings SET wake_name = ?2 WHERE project_id = ?1",
                    params![project_id, w],
                )?;
            }
            Ok(())
        })?;
        self.voice_agent_settings(project_id)
    }
}

const VOICE_COLS: &str = "call_id, project_id, payer, mode, started_at, ended_at, updated_at, seconds, audio_in_tokens, audio_out_tokens, text_in_tokens, text_out_tokens, cached_tokens, stt_seconds, decisions, tool_calls, turns_spoken, usd, usd_charged, ttfa_ms_p50, ttfa_ms_p95, barge_ms_p50, barge_ms_max, stop_reason, image_in_tokens, frames, looks, vision_seconds, vision_usd, vision_charged";

fn voice_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<VoiceCallRow> {
    let ended_at: Option<i64> = r.get(5)?;
    Ok(VoiceCallRow {
        call_id: r.get(0)?,
        project_id: r.get(1)?,
        payer: r.get(2)?,
        started_at: r.get(4)?,
        ended_at,
        updated_at: r.get(6)?,
        usage: VoiceUsage {
            mode: r.get(3)?,
            seconds: r.get(7)?,
            audio_in_tokens: r.get(8)?,
            audio_out_tokens: r.get(9)?,
            text_in_tokens: r.get(10)?,
            text_out_tokens: r.get(11)?,
            cached_tokens: r.get(12)?,
            stt_seconds: r.get(13)?,
            decisions: r.get(14)?,
            tool_calls: r.get(15)?,
            turns_spoken: r.get(16)?,
            usd: r.get(17)?,
            ttfa_ms_p50: r.get(19)?,
            ttfa_ms_p95: r.get(20)?,
            barge_ms_p50: r.get(21)?,
            barge_ms_max: r.get(22)?,
            ended: ended_at.is_some(),
            stop_reason: r.get(23)?,
            image_in_tokens: r.get(24)?,
            frames: r.get(25)?,
            looks: r.get(26)?,
            vision_seconds: r.get(27)?,
            vision_usd: r.get(28)?,
        },
        usd_charged: r.get(18)?,
        vision_charged: r.get(29)?,
    })
}
