//! Memory tree (D77): a read-only side index over the Project's own T3 history.
//!
//! Leaves are the existing `t3` rows (every message, tool call, tool result, subagent step);
//! nothing in `t3` is ever changed. A background builder groups leaves into episodes, episodes
//! into threads (chats, DMs, groups, subagent sessions) under the Project root, and writes one
//! index card per node (heading, summary, entities) with the project's decision-model slot.
//! Big tool results also get a page card (what the page/output is, its salient labels and
//! numbers). Before every reply and every subagent launch, the decision model (Jev through the
//! relay's `/proxy/decisions`) walks the tree top-down — one request per sibling group, all
//! siblings scored in parallel — expands what scores above a per-content-type threshold, adds
//! keyword hits as extra leaf candidates, and packs the chosen raw leaves verbatim under their
//! headings. The pack is injected into the prompt (mode `on`), only logged (mode `log`), or
//! skipped (`off`). Every walk is logged next to what the model had anyway (the pi session's
//! recent window), with Jev scores, tokens, cost and latency. A per-box daily spend cap stops
//! both the builder and the walk; the walk then falls back to today's behaviour.
//!
//! Each Project's store holds only its own history, so the tree is private to that agent by
//! construction. Group and people chats see only their own subtree.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use futures::future::join_all;
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use arbos_core::ids::{now_ms, ulid};

use crate::config::Config;
use crate::llm::Llm;
use crate::store::{Store, T3Row};

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS tree_nodes (
  id TEXT PRIMARY KEY, level TEXT NOT NULL, thread_id TEXT, parents TEXT NOT NULL DEFAULT '[]',
  children TEXT NOT NULL DEFAULT '[]', heading TEXT NOT NULL DEFAULT '', summary TEXT NOT NULL DEFAULT '',
  entities TEXT NOT NULL DEFAULT '[]', t0 INTEGER NOT NULL DEFAULT 0, t1 INTEGER NOT NULL DEFAULT 0,
  tokens INTEGER NOT NULL DEFAULT 0, kind TEXT NOT NULL DEFAULT 'chat', status TEXT NOT NULL DEFAULT 'open',
  title TEXT NOT NULL DEFAULT '', updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS tree_nodes_level ON tree_nodes(level, thread_id);
CREATE TABLE IF NOT EXISTS tree_page_cards (
  page_key TEXT PRIMARY KEY, page TEXT NOT NULL, facts TEXT NOT NULL DEFAULT '[]', created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS tree_leaf_pages (leaf_id TEXT PRIMARY KEY, page_key TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS tree_state (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS tree_cost (
  id TEXT PRIMARY KEY, kind TEXT NOT NULL, usd REAL NOT NULL, tokens_in INTEGER NOT NULL, tokens_out INTEGER NOT NULL, created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS tree_cost_at ON tree_cost(created_at);
CREATE TABLE IF NOT EXISTS tree_walk_log (
  id TEXT PRIMARY KEY, chat_id TEXT NOT NULL, message_id TEXT, purpose TEXT NOT NULL, mode TEXT NOT NULL, outcome TEXT NOT NULL,
  reason TEXT, query TEXT, pack TEXT, decisions TEXT, jev_calls INTEGER NOT NULL DEFAULT 0, jev_cost REAL NOT NULL DEFAULT 0,
  jev_ms INTEGER NOT NULL DEFAULT 0, walk_ms INTEGER NOT NULL DEFAULT 0, pack_tokens INTEGER NOT NULL DEFAULT 0,
  recent_window TEXT, created_at INTEGER NOT NULL
);
CREATE VIRTUAL TABLE IF NOT EXISTS tree_cards_fts USING fts5(text, node_id UNINDEXED, thread_id UNINDEXED, tokenize = 'porter unicode61');
CREATE TABLE IF NOT EXISTS tree_score_cache (key TEXT PRIMARY KEY, p REAL NOT NULL, created_at INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS tree_score_cache_at ON tree_score_cache(created_at);
"#;

const EPISODE_GAP_MS: i64 = 30 * 60 * 1000;
const SETTLE_MS: i64 = 90 * 1000;
const THREAD_CARD_DEBOUNCE_MS: i64 = 10 * 60 * 1000;
const ROOT_CARD_DEBOUNCE_MS: i64 = 30 * 60 * 1000;
/// A card call that failed (model answered no JSON, upstream error) is retried no sooner than
/// this. Without it a node whose card keeps failing was retried on every 60 s build pass — one
/// Haiku call a minute per workspace, forever, with nothing new to summarise.
const CARD_FAIL_BACKOFF_MS: i64 = 60 * 60 * 1000;
const MAX_CARDS_PER_PASS: usize = 40;
const PAGE_CARD_MIN_CHARS: usize = 1500;
const PAGE_CARD_CLIP: usize = 7000;
const TOOL_CLIP_FOR_CARDS: usize = 1500;
const LEAF_CLIP_FOR_JEV: usize = 1200;
const JEV_CHUNK: usize = 24;
const BM25_EXTRA: i64 = 20;
const WALK_DEADLINE_MS: u64 = 2500;
const JEV_TIMEOUT_MS: u64 = 1500;
/// Fast walk: when the prefilter leaves at most this many threads, their episodes and leaves
/// are gathered up front and, if the whole candidate set fits `FLAT_MAX_CANDIDATES`, every level
/// is scored in one parallel round instead of three sequential ones. Jev judges each candidate on
/// its own, so the keep decisions are the same; only the round trips differ.
const FLAT_MAX_THREADS: usize = 6;
const FLAT_MAX_CANDIDATES: usize = 48;
/// Cached Jev scores live this long (same candidate card, same question words, same day).
const SCORE_CACHE_TTL_MS: i64 = 14 * 24 * 60 * 60 * 1000;
// Pack budgets (estimated tokens). "Full" is the pack as shipped with D77: one budget by content
// kind, raw leaf text clipped. "Compact" (`ARBOS_MEMORY_PACK_COMPACT`) is the D87 follow-up: with
// the prompt prefix stable the per-turn block is the largest fresh cache write of a turn, and
// Haiku writes cost 12.5× reads, so chat and tool leaves each get their own budget, the whole
// block is capped, a tool output that has a page card is packed as the card + facts only (the raw
// text is one `context_open` away), exact repeats and leaves the prompt's project-memory block
// already states are dropped.
const BUDGET_CHAT_FULL: usize = 6000;
const BUDGET_TOOL_FULL: usize = 12000;
const CLIP_CHAT: usize = 2000;
const CLIP_TOOL_FULL: usize = 6000;
const BUDGET_CHAT: usize = 4000;
const BUDGET_TOOL: usize = 3000;
const BUDGET_PACK: usize = 5000;
const CLIP_TOOL_PACK: usize = 1500;
/// Leaves shorter than this are never deduplicated against the prompt's project-memory block
/// (short lines match by accident).
const DEDUPE_MIN_CHARS: usize = 40;
const ROOT_ID: &str = "root";

const CARD_SYSTEM: &str = "You write index cards for an agent's memory tree. Given raw events (or child cards), return JSON with keys: \"heading\" (ONE line, <= 110 chars: who did what, and the outcome), \"summary\" (2-3 plain sentences with the concrete facts, names, numbers, decisions, file paths and results that someone searching later would need), \"entities\" (list of <= 10 short strings: people, projects, tools, repos, topics). Never invent facts. Prefer specifics over generalities. Output JSON only.";
const PAGE_CARD_SYSTEM: &str = "You index tool results an agent saw. Given a tool call and its output (clipped), return JSON {\"page\": \"<one line: what this output/page is and what it is for>\", \"facts\": [<up to 14 short strings>]}. The facts are the concrete things someone might later ask about: labels, option names in order, column headers, button names, prices, counts, dates, status values, error text, file names, record names. Quote them exactly. JSON only.";

/// Where the pack goes: into the prompt, only into the log, or nowhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeMode {
    On,
    Log,
    Off,
}

impl TreeMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "on" | "inject" | "true" | "1" => Some(Self::On),
            "log" | "log_only" | "log-only" | "shadow" => Some(Self::Log),
            "off" | "false" | "0" | "none" => Some(Self::Off),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::On => "on",
            Self::Log => "log",
            Self::Off => "off",
        }
    }
}

/// Content kind of a thread; picks the walk thresholds and the pack clip (chat vs tool history).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Kind {
    Chat,
    Tool,
}

impl Kind {
    fn parse(s: &str) -> Self {
        if s == "tool" {
            Self::Tool
        } else {
            Self::Chat
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Tool => "tool",
        }
    }
    /// Recall over precision: the benchmark sweeps kept full gold-leaf recall at these values
    /// (chat history 0.3–0.5; tool history needs lower thread/episode cuts), and card wording
    /// shifts Jev's probabilities by a few tenths, so the chat cuts sit at the low end.
    fn threshold(self, level: &str) -> f64 {
        match (self, level) {
            (Self::Chat, "thread") => 0.3,
            (Self::Chat, "episode") => 0.3,
            (Self::Chat, "leaf") => 0.4,
            (Self::Tool, "thread") => 0.2,
            (Self::Tool, "episode") => 0.3,
            (Self::Tool, "leaf") => 0.4,
            _ => 0.5,
        }
    }
    fn beam(self, level: &str) -> usize {
        match (self, level) {
            (Self::Chat, "thread") => 12,
            (Self::Chat, "episode") => 16,
            (Self::Tool, "thread") => 16,
            (Self::Tool, "episode") => 20,
            _ => 80,
        }
    }
    fn episode_max_tokens(self) -> usize {
        match self {
            Self::Chat => 2500,
            Self::Tool => 8000,
        }
    }
    fn episode_max_leaves(self) -> usize {
        match self {
            Self::Chat => 24,
            Self::Tool => 12,
        }
    }
    fn clip_full(self) -> usize {
        match self {
            Self::Chat => CLIP_CHAT,
            Self::Tool => CLIP_TOOL_FULL,
        }
    }
    fn clip(self) -> usize {
        match self {
            Self::Chat => CLIP_CHAT,
            Self::Tool => CLIP_TOOL_PACK,
        }
    }
    fn budget(self) -> usize {
        match self {
            Self::Chat => BUDGET_CHAT,
            Self::Tool => BUDGET_TOOL,
        }
    }
}

#[derive(Debug, Clone)]
struct Node {
    id: String,
    level: String,
    thread_id: Option<String>,
    parents: Vec<String>,
    children: Vec<String>,
    heading: String,
    summary: String,
    entities: Vec<String>,
    t0: i64,
    t1: i64,
    tokens: i64,
    kind: Kind,
    status: String,
    title: String,
    updated_at: i64,
}

/// The pack for one turn, plus everything the log needs.
#[derive(Debug, Clone, Default)]
pub struct Pack {
    pub text: String,
    pub tokens: usize,
    pub leaf_ids: Vec<String>,
    pub node_ids: Vec<String>,
    pub decisions: Vec<Value>,
    pub jev_calls: i64,
    pub jev_cost: f64,
    pub jev_ms: i64,
    pub walk_ms: i64,
    /// `injected` | `fallback` | `empty` | `logged`.
    pub outcome: String,
    pub reason: Option<String>,
    /// Threads the keyword prefilter kept away from Jev.
    pub prefiltered: usize,
    /// Candidates answered from `tree_score_cache` instead of a Jev request.
    pub cache_hits: i64,
    /// Every level scored in one parallel round.
    pub flat: bool,
    /// What the prefilter did: `off` (D77 walk), `evidence` (only matching threads expanded on
    /// their own), `first-person` (nothing matched but the question is about the user's own
    /// world, so every thread was scored), `no-words` (no content words; every thread scored).
    pub prefilter: &'static str,
}

/// How one walk renders and runs. `compact` is the D87 pack policy; `fast` is the prefilter +
/// score cache + one-round scoring (`ARBOS_MEMORY_WALK_FAST`); off = the D77 walk exactly.
#[derive(Debug, Clone, Copy)]
pub struct WalkOptions {
    pub compact: bool,
    pub fast: bool,
}

/// Where a leaf candidate came from; decides whether it stays once its parents are scored.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Via {
    Episode(String),
    Thread(String),
}

/// What one walk may see: the asking chat, the allowed threads (restricted scopes), and — after
/// a session compaction — the asking chat's own leaves older than `own_cut` (D91: those left the
/// model's window and live on only in the tree).
struct Scope<'a> {
    chat_id: &'a str,
    allowed: Option<HashSet<String>>,
    own_cut: Option<String>,
}

impl Scope<'_> {
    fn thread_ok(&self, thread_id: &str) -> bool {
        self.allowed
            .as_ref()
            .map(|a| a.contains(thread_id))
            .unwrap_or(true)
    }

    /// A leaf of the asking chat is a candidate only when it was compacted away.
    fn leaf_ok(&self, thread_id: &str, leaf_id: &str) -> bool {
        if thread_id != self.chat_id {
            return self.thread_ok(thread_id);
        }
        self.own_cut.as_deref().is_some_and(|cut| leaf_id < cut)
    }
}

/// Episode-level candidates of a set of threads.
#[derive(Debug, Default)]
struct Gathered {
    /// Ready episodes as cards for Jev.
    ep_cards: Vec<(String, Value)>,
    ep_nodes: HashMap<String, Node>,
    /// Leaves of episodes without a card yet: (leaf id, thread node id).
    direct: Vec<(String, String)>,
}

pub struct Tree {
    project_id: String,
    project_name: String,
    /// Chat model for the cards (the project's `decision_model` slot).
    card_model: String,
    store: Store,
    llm: Arc<Llm>,
    build_lock: tokio::sync::Mutex<()>,
    cap_warned: std::sync::Mutex<Option<i64>>,
}

pub fn est_tokens(s: &str) -> usize {
    s.chars().count() / 4 + 1
}

fn fmt_ts(ms: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms)
        .map(|d| d.format("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_default()
}

/// Lower-cased, whitespace-collapsed text for the pack's repeat / prompt-memory checks.
fn norm_text(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn clip_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        let head: String = s.chars().take(max).collect();
        format!("{head} …[{} more chars]", s.chars().count() - max)
    }
}

fn sha16(s: &str) -> String {
    let d = Sha256::digest(s.as_bytes());
    hex::encode(&d[..8])
}

const STOPWORDS: &[&str] = &[
    "the", "and", "for", "that", "this", "with", "what", "which", "who", "whom", "when", "where",
    "why", "how", "was", "were", "are", "is", "did", "does", "do", "you", "your", "our", "its",
    "can", "could", "would", "should", "have", "has", "had", "from", "into", "about", "tell",
    "please", "there", "they", "them", "then", "than", "also", "just", "any", "some", "all", "not",
    "but", "yet", "again", "still", "ever", "me", "my",
];

/// Content words of a question, for the keyword candidates (OR query).
fn content_words(q: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for w in q
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-' && c != '.')
    {
        let w = w.trim_matches(|c: char| c == '.' || c == '-');
        if w.len() > 2 && !STOPWORDS.contains(&w) && !out.iter().any(|x| x == w) {
            out.push(w.to_owned());
        }
    }
    out.truncate(12);
    out
}

/// Parse a card; a truncated or fenced answer still yields heading + summary through a regex.
fn parse_card(text: &str) -> Option<(String, String, Vec<String>)> {
    let take = |s: &str, n: usize| s.chars().take(n).collect::<String>();
    if let Some(v) = crate::llm::parse_json_loose(text) {
        if let Some(h) = v.get("heading").and_then(Value::as_str) {
            let entities = v
                .get("entities")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str())
                        .map(|s| take(s, 40))
                        .take(10)
                        .collect()
                })
                .unwrap_or_default();
            return Some((
                take(h, 160),
                take(v.get("summary").and_then(Value::as_str).unwrap_or(""), 900),
                entities,
            ));
        }
    }
    let re_h = regex_lite::Regex::new(r#""heading"\s*:\s*"((?:[^"\\]|\\.)*)""#).ok()?;
    let re_s = regex_lite::Regex::new(r#""summary"\s*:\s*"((?:[^"\\]|\\.)*)"#).ok()?;
    let h = re_h.captures(text)?.get(1)?.as_str().replace("\\\"", "\"");
    let s = re_s
        .captures(text)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().replace("\\\"", "\""))
        .unwrap_or_default();
    Some((take(&h, 160), take(&s, 900), Vec::new()))
}

fn day_start_ms(now: i64) -> i64 {
    now - now.rem_euclid(86_400_000)
}

/// FTS5 query for the question's content words: an OR of quoted terms. The porter tokenizer
/// stems both sides, so "remembered" meets "remember" and "cats" meets "cat".
fn fts_or(words: &[String]) -> Option<String> {
    let terms: Vec<String> = words
        .iter()
        .map(|w| w.replace('"', ""))
        .filter(|w| !w.trim().is_empty())
        .map(|w| format!("\"{w}\""))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" OR "))
}

/// The part of a node's card the prefilter indexes.
fn card_text(n: &Node) -> String {
    format!(
        "{} {} {} {}",
        n.title,
        n.heading,
        n.summary,
        n.entities.join(" ")
    )
}

/// Does the question speak of the user's own world ("my", "our", "I", "we")? Such a question can
/// need a record that shares no word with it ("what breed is my pet?" vs "Biscuit, our golden
/// retriever"), so when no thread has keyword evidence it still gets the full walk.
fn first_person(q: &str) -> bool {
    const MARKERS: &[&str] = &[
        "i",
        "i'm",
        "i've",
        "i'd",
        "i'll",
        "my",
        "mine",
        "myself",
        "we",
        "we're",
        "we've",
        "we'd",
        "we'll",
        "our",
        "ours",
        "ourselves",
    ];
    q.to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '\'')
        .any(|w| MARKERS.contains(&w))
}

/// Cache context for one walk: the question's content words (order-free), the UTC day it was
/// asked (Jev sees `asked_at`, so "yesterday" must not outlive the day), who asked, and the
/// visibility scope.
fn cache_ctx(words: &[String], asked_at: i64, asker: &str, restricted: bool) -> String {
    let mut w = words.to_vec();
    w.sort();
    format!(
        "{}|{}|{asker}|{restricted}",
        w.join(" "),
        day_start_ms(asked_at)
    )
}

impl Tree {
    pub fn new(cfg: &Config, store: Store, llm: Arc<Llm>) -> Self {
        Self::with_ids(
            &cfg.project_id,
            &cfg.project_name,
            &cfg.decision_model,
            store,
            llm,
        )
    }

    pub fn with_ids(
        project_id: &str,
        project_name: &str,
        card_model: &str,
        store: Store,
        llm: Arc<Llm>,
    ) -> Self {
        let _ = store.exec_batch(SCHEMA);
        // Every card and walk call carries `x-arbos-purpose: memory-tree`, so the relay's usage
        // table and credit ledger show the feature's spend on its own.
        let llm = Arc::new(llm.with_purpose("memory-tree"));
        let tree = Self {
            project_id: project_id.to_owned(),
            project_name: project_name.to_owned(),
            card_model: card_model.to_owned(),
            store,
            llm,
            build_lock: tokio::sync::Mutex::new(()),
            cap_warned: std::sync::Mutex::new(None),
        };
        tree.backfill_cards_fts();
        tree
    }

    /// Trees built before the prefilter existed have cards but no `tree_cards_fts` rows: index
    /// them once (no model calls; a few thousand rows at most).
    fn backfill_cards_fts(&self) {
        if self.state_get("cards_fts_built").is_some() {
            return;
        }
        let nodes = self.nodes_where("level IN ('thread','episode')", &[]);
        let _ = self
            .store
            .with(|c| c.execute("DELETE FROM tree_cards_fts", []));
        for n in &nodes {
            self.index_card(n);
        }
        self.state_set("cards_fts_built", "1");
    }

    // ---- settings -----------------------------------------------------------------------------

    /// Per-box daily cap on tree spend (cards + Jev), USD. `ARBOS_MEMORY_TREE_DAILY_CAP_USD`,
    /// default 0.80 so six boxes stay under the $5/day rule.
    pub fn daily_cap_usd() -> f64 {
        std::env::var("ARBOS_MEMORY_TREE_DAILY_CAP_USD")
            .ok()
            .and_then(|s| s.parse::<f64>().ok())
            .filter(|v| *v >= 0.0)
            .unwrap_or(0.80)
    }

    pub fn spend_today(&self) -> f64 {
        let since = day_start_ms(now_ms());
        self.store
            .with(|c| {
                c.query_row(
                    "SELECT COALESCE(SUM(usd), 0) FROM tree_cost WHERE created_at >= ?1",
                    params![since],
                    |r| r.get::<_, f64>(0),
                )
            })
            .unwrap_or(0.0)
    }

    /// Spend today split by kind, plus a straight-line projection to 24 h.
    pub fn spend_report(&self) -> Value {
        let now = now_ms();
        let since = day_start_ms(now);
        let rows: Vec<(String, f64, i64)> = self
            .store
            .with(|c| {
                let mut st = c.prepare("SELECT kind, COALESCE(SUM(usd),0), COUNT(*) FROM tree_cost WHERE created_at >= ?1 GROUP BY kind")?;
                let it = st.query_map(params![since], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
                it.collect()
            })
            .unwrap_or_default();
        let total: f64 = rows.iter().map(|r| r.1).sum();
        let elapsed_h = ((now - since) as f64 / 3_600_000.0).max(0.25);
        json!({
            "project_id": self.project_id,
            "today_usd": total,
            "by_kind": rows.iter().map(|(k, u, n)| json!({"kind": k, "usd": u, "calls": n})).collect::<Vec<_>>(),
            "projected_24h_usd": total / elapsed_h * 24.0,
            "cap_usd": Self::daily_cap_usd(),
            "capped": self.capped(),
        })
    }

    /// True once today's spend reached the cap; warns once a day.
    pub fn capped(&self) -> bool {
        let cap = Self::daily_cap_usd();
        if cap <= 0.0 {
            return false;
        }
        let spent = self.spend_today();
        if spent < cap {
            return false;
        }
        let today = day_start_ms(now_ms());
        let mut w = self.cap_warned.lock().expect("cap warned");
        if *w != Some(today) {
            *w = Some(today);
            tracing::warn!(project = %self.project_id, spent_usd = spent, cap_usd = cap, "memory tree: daily spend cap reached; builder and walk paused until tomorrow (UTC)");
        }
        true
    }

    fn add_cost(&self, kind: &str, usage: &Value) {
        let usd = usage
            .get("cost")
            .or_else(|| usage.get("cost_usd"))
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let tin = usage
            .get("prompt_tokens")
            .or_else(|| usage.get("input_tokens"))
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let tout = usage
            .get("completion_tokens")
            .or_else(|| usage.get("output_tokens"))
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let _ = self.store.with(|c| {
            c.execute(
                "INSERT INTO tree_cost(id, kind, usd, tokens_in, tokens_out, created_at) VALUES (?1,?2,?3,?4,?5,?6)",
                params![ulid(), kind, usd, tin, tout, now_ms()],
            )
        });
    }

    fn state_get(&self, key: &str) -> Option<String> {
        self.store
            .with(|c| {
                c.query_row(
                    "SELECT value FROM tree_state WHERE key = ?1",
                    params![key],
                    |r| r.get::<_, String>(0),
                )
                .optional()
            })
            .ok()
            .flatten()
    }

    fn state_set(&self, key: &str, value: &str) {
        let _ = self.store.with(|c| {
            c.execute(
                "INSERT INTO tree_state(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )
        });
    }

    // ---- nodes --------------------------------------------------------------------------------

    fn node_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Node> {
        let parents: String = r.get(3)?;
        let children: String = r.get(4)?;
        let entities: String = r.get(7)?;
        let kind: String = r.get(11)?;
        Ok(Node {
            id: r.get(0)?,
            level: r.get(1)?,
            thread_id: r.get(2)?,
            parents: serde_json::from_str(&parents).unwrap_or_default(),
            children: serde_json::from_str(&children).unwrap_or_default(),
            heading: r.get(5)?,
            summary: r.get(6)?,
            entities: serde_json::from_str(&entities).unwrap_or_default(),
            t0: r.get(8)?,
            t1: r.get(9)?,
            tokens: r.get(10)?,
            kind: Kind::parse(&kind),
            status: r.get(12)?,
            title: r.get(13)?,
            updated_at: r.get(14)?,
        })
    }

    const NODE_COLS: &'static str = "id, level, thread_id, parents, children, heading, summary, entities, t0, t1, tokens, kind, status, title, updated_at";

    fn node(&self, id: &str) -> Option<Node> {
        self.store
            .with(|c| {
                c.query_row(
                    &format!("SELECT {} FROM tree_nodes WHERE id = ?1", Self::NODE_COLS),
                    params![id],
                    Self::node_from_row,
                )
                .optional()
            })
            .ok()
            .flatten()
    }

    fn nodes_where(&self, where_sql: &str, p: &[&dyn rusqlite::ToSql]) -> Vec<Node> {
        self.store
            .with(|c| {
                let mut st = c.prepare(&format!(
                    "SELECT {} FROM tree_nodes WHERE {where_sql}",
                    Self::NODE_COLS
                ))?;
                let it = st.query_map(p, Self::node_from_row)?;
                it.collect()
            })
            .unwrap_or_default()
    }

    fn upsert_node(&self, n: &Node) {
        let _ = self.store.with(|c| {
            c.execute(
                "INSERT INTO tree_nodes(id, level, thread_id, parents, children, heading, summary, entities, t0, t1, tokens, kind, status, title, updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)
                 ON CONFLICT(id) DO UPDATE SET parents=excluded.parents, children=excluded.children, heading=excluded.heading, summary=excluded.summary,
                   entities=excluded.entities, t0=excluded.t0, t1=excluded.t1, tokens=excluded.tokens, kind=excluded.kind, status=excluded.status, title=excluded.title, updated_at=excluded.updated_at",
                params![
                    n.id, n.level, n.thread_id, serde_json::to_string(&n.parents).unwrap_or_default(), serde_json::to_string(&n.children).unwrap_or_default(),
                    n.heading, n.summary, serde_json::to_string(&n.entities).unwrap_or_default(), n.t0, n.t1, n.tokens, n.kind.as_str(), n.status, n.title, n.updated_at
                ],
            )
        });
    }

    fn set_status(&self, id: &str, status: &str) {
        let _ = self.store.with(|c| {
            c.execute(
                "UPDATE tree_nodes SET status = ?2, updated_at = CASE WHEN ?2 = 'card_failed' THEN ?3 ELSE updated_at END WHERE id = ?1",
                params![id, status, now_ms()],
            )
        });
    }

    /// Keep the prefilter's card index in step with a thread or episode card.
    fn index_card(&self, n: &Node) {
        if n.level != "thread" && n.level != "episode" {
            return;
        }
        let text = card_text(n);
        let _ = self.store.with(|c| {
            c.execute(
                "DELETE FROM tree_cards_fts WHERE node_id = ?1",
                params![n.id],
            )?;
            if !text.trim().is_empty() {
                c.execute(
                    "INSERT INTO tree_cards_fts(text, node_id, thread_id) VALUES (?1, ?2, ?3)",
                    params![text, n.id, n.thread_id],
                )?;
            }
            Ok(())
        });
    }

    /// Threads with keyword evidence for the question: a leaf (`t3_fts`) or an index card
    /// (`tree_cards_fts`) shares a stemmed content word with it. A word that occurs in most
    /// threads of the project (the agent's greetings, the user's name, "hello", "say") tells
    /// nothing about *which* thread to read, so once a project has a few threads such words are
    /// not evidence: only words found in at most half of the threads count. `None` = no filter:
    /// the question has no content words, or the lookup failed (fail open — memory is never lost
    /// to a query syntax error).
    fn threads_with_evidence(
        &self,
        words: &[String],
        thread_count: usize,
    ) -> Option<HashSet<String>> {
        fts_or(words)?;
        let common_at = if thread_count >= 4 {
            thread_count / 2
        } else {
            usize::MAX
        };
        let found = self.store.with(|c| {
            let mut leaves = c.prepare("SELECT DISTINCT thread_id FROM t3_fts WHERE t3_fts MATCH ?1")?;
            let mut cards = c.prepare(
                "SELECT DISTINCT thread_id FROM tree_cards_fts WHERE tree_cards_fts MATCH ?1 AND thread_id IS NOT NULL",
            )?;
            let mut set: HashSet<String> = HashSet::new();
            for w in words {
                let Some(q) = fts_or(std::slice::from_ref(w)) else {
                    continue;
                };
                let mut per_word: HashSet<String> = HashSet::new();
                for r in leaves.query_map(params![q], |r| r.get::<_, String>(0))? {
                    per_word.insert(r?);
                }
                for r in cards.query_map(params![q], |r| r.get::<_, String>(0))? {
                    per_word.insert(r?);
                }
                if per_word.len() > common_at {
                    continue;
                }
                set.extend(per_word);
            }
            Ok(set)
        });
        match found {
            Ok(set) => Some(set),
            Err(e) => {
                tracing::debug!(error = %e, "tree walk: keyword prefilter failed; scoring every thread");
                None
            }
        }
    }

    fn cached_scores(&self, keys: &[String]) -> HashMap<String, f64> {
        let mut out = HashMap::new();
        for chunk in keys.chunks(200) {
            let marks = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let p: Vec<&dyn rusqlite::ToSql> =
                chunk.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
            let rows: Vec<(String, f64)> = self
                .store
                .with(|c| {
                    let mut st = c.prepare(&format!(
                        "SELECT key, p FROM tree_score_cache WHERE key IN ({marks})"
                    ))?;
                    let it = st.query_map(p.as_slice(), |r| Ok((r.get(0)?, r.get(1)?)))?;
                    it.collect()
                })
                .unwrap_or_default();
            out.extend(rows);
        }
        out
    }

    fn cache_put(&self, entries: &[(String, f64)]) {
        if entries.is_empty() {
            return;
        }
        let now = now_ms();
        let _ = self.store.with(|c| {
            for (k, p) in entries {
                c.execute(
                    "INSERT INTO tree_score_cache(key, p, created_at) VALUES (?1, ?2, ?3) ON CONFLICT(key) DO UPDATE SET p = excluded.p, created_at = excluded.created_at",
                    params![k, p, now],
                )?;
            }
            Ok(())
        });
    }

    fn expire_score_cache(&self) {
        let _ = self.store.with(|c| {
            c.execute(
                "DELETE FROM tree_score_cache WHERE created_at < ?1",
                params![now_ms() - SCORE_CACHE_TTL_MS],
            )
        });
    }

    fn t3_rows(&self, sql: &str, p: &[&dyn rusqlite::ToSql]) -> Vec<T3Row> {
        self.store
            .with(|c| {
                let mut st = c.prepare(&format!(
                    "SELECT id, thread_id, subagent_id, session_id, kind, role, at, text, size FROM t3 {sql}"
                ))?;
                let it = st.query_map(p, |r| {
                    Ok(T3Row {
                        id: r.get(0)?,
                        thread_id: r.get(1)?,
                        subagent_id: r.get(2)?,
                        session_id: r.get(3)?,
                        kind: r.get(4)?,
                        role: r.get(5)?,
                        at: r.get(6)?,
                        text: r.get(7)?,
                        size: r.get(8)?,
                    })
                })?;
                it.collect()
            })
            .unwrap_or_default()
    }

    fn leaves_by_ids(&self, ids: &[String]) -> Vec<T3Row> {
        let mut out = Vec::new();
        for chunk in ids.chunks(200) {
            let marks = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let p: Vec<&dyn rusqlite::ToSql> =
                chunk.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
            out.extend(self.t3_rows(&format!("WHERE id IN ({marks}) ORDER BY at, id"), &p));
        }
        out
    }

    fn page_card(&self, leaf_id: &str) -> Option<(String, Vec<String>)> {
        self.store
            .with(|c| {
                c.query_row(
                    "SELECT p.page, p.facts FROM tree_leaf_pages l JOIN tree_page_cards p ON p.page_key = l.page_key WHERE l.leaf_id = ?1",
                    params![leaf_id],
                    |r| {
                        let facts: String = r.get(1)?;
                        Ok((r.get::<_, String>(0)?, serde_json::from_str(&facts).unwrap_or_default()))
                    },
                )
                .optional()
            })
            .ok()
            .flatten()
    }

    /// One leaf as text: timestamp, role, kind, page card when there is one, the text clipped.
    fn leaf_line(&self, leaf: &T3Row, clip: usize) -> String {
        self.leaf_line_with(leaf, clip, false)
    }

    /// `leaf_line` for the per-turn pack: a leaf with a page card is rendered as the card and its
    /// facts only, plus the `context_open` pointer — the raw output is what made the block large.
    fn leaf_line_packed(&self, leaf: &T3Row, clip: usize) -> String {
        self.leaf_line_with(leaf, clip, true)
    }

    fn leaf_line_with(&self, leaf: &T3Row, clip: usize, card_only: bool) -> String {
        let who = leaf.role.clone().unwrap_or_else(|| leaf.kind.clone());
        let mut s = format!("[{}] {who} ({}): ", fmt_ts(leaf.at), leaf.kind);
        if let Some((page, facts)) = self.page_card(&leaf.id) {
            // a parked (failed) page card is empty: show nothing rather than an empty bracket
            if !page.is_empty() {
                s.push_str(&format!("〔{page}〕 "));
            }
            if !facts.is_empty() {
                s.push_str(&format!("facts: {} — ", facts.join(" | ")));
            }
            if card_only && !page.is_empty() {
                s.push_str(&format!(
                    "[id {} — context_open for the full output]",
                    leaf.id
                ));
                return s;
            }
        }
        let body = clip_chars(&leaf.text, clip);
        s.push_str(&body);
        if leaf.text.chars().count() > clip {
            s.push_str(&format!(
                " [id {} — context_open for the full text]",
                leaf.id
            ));
        }
        s
    }

    /// Compact pack: newest first into per-kind budgets and the cap for the whole block. Before
    /// budgeting, exact repeats (same role and text — the newest stays) and leaves the prompt's
    /// project-memory block already states verbatim are dropped. `blocks` come back in time order.
    fn pack_compact(
        &self,
        kept: &mut [&T3Row],
        leaf_kind: &HashMap<String, Kind>,
        prompt_memory: &str,
        pack: &mut Pack,
        blocks: &mut Vec<(i64, String, usize, String)>,
        total: &mut usize,
    ) {
        let memory_norm = norm_text(prompt_memory);
        // memory items read `- text` (T0) or `- [kind] text` (T1): compare the text only
        let memory_lines: Vec<String> = prompt_memory
            .lines()
            .map(|l| {
                let l = l.trim().trim_start_matches("- ");
                let l = match (l.starts_with('['), l.find("] ")) {
                    (true, Some(i)) => &l[i + 2..],
                    _ => l,
                };
                norm_text(l)
            })
            .filter(|l| l.chars().count() >= DEDUPE_MIN_CHARS)
            .collect();
        kept.sort_by(|a, b| (b.at, &b.id).cmp(&(a.at, &a.id)));
        let mut seen: HashSet<(String, String)> = HashSet::new();
        let mut used: HashMap<Kind, usize> = HashMap::new();
        for l in kept.iter() {
            let kind = leaf_kind.get(&l.id).copied().unwrap_or(Kind::Chat);
            let body = norm_text(&l.text);
            let key = (
                l.role.clone().unwrap_or_default(),
                body.chars().take(400).collect::<String>(),
            );
            let long_enough = body.chars().count() >= DEDUPE_MIN_CHARS;
            let dropped = if !seen.insert(key) {
                Some("repeat")
            } else if long_enough
                && (memory_norm.contains(body.as_str())
                    || memory_lines.iter().any(|m| body.contains(m.as_str())))
            {
                Some("in prompt memory")
            } else {
                None
            };
            if let Some(why) = dropped {
                pack.decisions
                    .push(json!({ "level": "pack", "node_id": l.id, "dropped": why }));
                continue;
            }
            let line = self.leaf_line_packed(l, kind.clip());
            let tok = est_tokens(&line);
            let kind_used = used.get(&kind).copied().unwrap_or(0);
            if kind_used + tok > kind.budget() || *total + tok > BUDGET_PACK {
                pack.decisions.push(json!({ "level": "pack", "node_id": l.id, "dropped": "budget", "tokens": tok, "kind": kind.as_str() }));
                continue;
            }
            used.insert(kind, kind_used + tok);
            *total += tok;
            blocks.push((l.at, line, tok, l.id.clone()));
        }
        blocks.sort_by(|a, b| (a.0, &a.3).cmp(&(b.0, &b.3)));
    }

    // ---- thread metadata ----------------------------------------------------------------------

    fn thread_meta(&self, thread_id: &str) -> (String, Kind, Vec<String>) {
        // chat?
        if let Ok(Some(c)) = self.store.chat(thread_id) {
            return (c.title, Kind::Chat, vec![ROOT_ID.to_owned()]);
        }
        if let Ok(Some(s)) = self.store.subagent(thread_id) {
            let mut parents = vec![ROOT_ID.to_owned()];
            if let Some(cid) = &s.chat_id {
                parents.push(format!("thread:{cid}"));
            }
            return (
                format!("subagent {}: {}", s.name, s.purpose),
                Kind::Tool,
                parents,
            );
        }
        (
            format!("chat {thread_id}"),
            Kind::Chat,
            vec![ROOT_ID.to_owned()],
        )
    }

    fn ensure_root(&self) {
        if self.node(ROOT_ID).is_none() {
            self.upsert_node(&Node {
                id: ROOT_ID.into(),
                level: "project".into(),
                thread_id: None,
                parents: vec![],
                children: vec![],
                heading: format!("Project {}", self.project_name),
                summary: String::new(),
                entities: vec![],
                t0: 0,
                t1: 0,
                tokens: 0,
                kind: Kind::Chat,
                status: "stale".into(),
                title: self.project_name.clone(),
                updated_at: 0,
            });
        }
    }

    fn ensure_thread(&self, thread_id: &str, at: i64) -> Node {
        let id = format!("thread:{thread_id}");
        if let Some(n) = self.node(&id) {
            return n;
        }
        let (title, kind, parents) = self.thread_meta(thread_id);
        let n = Node {
            id: id.clone(),
            level: "thread".into(),
            thread_id: Some(thread_id.to_owned()),
            parents: parents.clone(),
            children: vec![],
            heading: String::new(),
            summary: String::new(),
            entities: vec![],
            t0: at,
            t1: at,
            tokens: 0,
            kind,
            status: "stale".into(),
            title,
            updated_at: 0,
        };
        self.upsert_node(&n);
        self.index_card(&n);
        for p in &parents {
            if let Some(mut pn) = self.node(p) {
                if !pn.children.contains(&id) {
                    pn.children.push(id.clone());
                    pn.status = "stale".into();
                    self.upsert_node(&pn);
                }
            }
        }
        n
    }

    // ---- builder ------------------------------------------------------------------------------

    /// One incremental pass: new leaves → episodes; page cards; cards for settled episodes, stale
    /// threads and the root. Bounded work per pass; safe to call often.
    pub async fn build_pass(&self) -> Result<Value> {
        let _g = self.build_lock.lock().await;
        self.ensure_root();
        self.expire_score_cache();
        let t0 = Instant::now();
        let mut stats = json!({ "leaves": 0, "episodes_new": 0, "cards": 0, "page_cards": 0, "skipped_cap": false });
        // 1. leaves → episodes (no model calls)
        let through = self.state_get("built_through").unwrap_or_default();
        let rows = self.t3_rows(
            "WHERE id > ?1 AND kind <> 'route' ORDER BY id LIMIT 2000",
            &[&through],
        );
        let mut last_id = through.clone();
        let mut new_eps = 0;
        for leaf in &rows {
            let thread = self.ensure_thread(&leaf.thread_id, leaf.at);
            let kind = thread.kind;
            let tokens = est_tokens(&leaf.text) as i64;
            let open = self
                .nodes_where(
                    "level = 'episode' AND thread_id = ?1 ORDER BY t1 DESC LIMIT 1",
                    &[&leaf.thread_id],
                )
                .into_iter()
                .next();
            let starts_new = match &open {
                None => true,
                Some(ep) => {
                    let gap = leaf.at - ep.t1;
                    let new_turn = leaf.kind == "user"
                        && ep.tokens as usize > kind.episode_max_tokens() / 2
                        && ep.children.len() >= 6;
                    gap > EPISODE_GAP_MS
                        || (ep.tokens + tokens) as usize > kind.episode_max_tokens()
                        || ep.children.len() >= kind.episode_max_leaves()
                        || new_turn
                        || ep.status == "ready"
                            && (ep.children.len() >= kind.episode_max_leaves() / 2)
                }
            };
            if starts_new {
                if let Some(ep) = &open {
                    if ep.status == "open" {
                        self.set_status(&ep.id, "closed");
                    }
                }
                let ep = Node {
                    id: format!("ep:{}", ulid()),
                    level: "episode".into(),
                    thread_id: Some(leaf.thread_id.clone()),
                    parents: vec![thread.id.clone()],
                    children: vec![leaf.id.clone()],
                    heading: String::new(),
                    summary: String::new(),
                    entities: vec![],
                    t0: leaf.at,
                    t1: leaf.at,
                    tokens,
                    kind,
                    status: "open".into(),
                    title: String::new(),
                    updated_at: now_ms(),
                };
                self.upsert_node(&ep);
                let mut th = thread.clone();
                th.children.push(ep.id.clone());
                th.t1 = leaf.at;
                th.tokens += tokens;
                th.status = "stale".into();
                self.upsert_node(&th);
                new_eps += 1;
            } else if let Some(mut ep) = open {
                ep.children.push(leaf.id.clone());
                ep.t1 = leaf.at;
                ep.tokens += tokens;
                // a card written for an episode that then grew is stale again
                ep.status = "open".into();
                self.upsert_node(&ep);
                let mut th = thread.clone();
                th.t1 = leaf.at;
                th.tokens += tokens;
                th.status = "stale".into();
                self.upsert_node(&th);
            }
            last_id = leaf.id.clone();
        }
        if last_id != through {
            self.state_set("built_through", &last_id);
        }
        stats["leaves"] = json!(rows.len());
        stats["episodes_new"] = json!(new_eps);
        if self.capped() {
            stats["skipped_cap"] = json!(true);
            stats["ms"] = json!(t0.elapsed().as_millis() as i64);
            return Ok(stats);
        }
        let model = self.card_model();
        let mut calls = 0usize;
        let mut errors = 0usize;
        // 2. page cards for big tool results (deduped by content)
        let pages_through = self.state_get("pages_through").unwrap_or_default();
        let big = self.t3_rows(
            "WHERE id > ?1 AND kind IN ('tool_output','session_entry') AND size > ?2 ORDER BY id LIMIT 60",
            &[&pages_through, &(PAGE_CARD_MIN_CHARS as i64)],
        );
        let mut pages_last = pages_through.clone();
        for leaf in &big {
            if calls >= MAX_CARDS_PER_PASS {
                break;
            }
            let head: String = leaf.text.chars().take(PAGE_CARD_CLIP).collect();
            let key = sha16(&head);
            let exists: bool = self
                .store
                .with(|c| {
                    c.query_row(
                        "SELECT 1 FROM tree_page_cards WHERE page_key = ?1",
                        params![key],
                        |_| Ok(()),
                    )
                    .optional()
                    .map(|o| o.is_some())
                })
                .unwrap_or(false);
            if !exists {
                let call = self
                    .t3_rows(
                        "WHERE thread_id = ?1 AND kind = 'tool_call' AND id < ?2 ORDER BY id DESC LIMIT 1",
                        &[&leaf.thread_id, &leaf.id],
                    )
                    .into_iter()
                    .next()
                    .map(|c| clip_chars(&c.text, 400))
                    .unwrap_or_default();
                match self
                    .llm
                    .complete_json_with_usage(
                        &model,
                        PAGE_CARD_SYSTEM,
                        &format!("Tool call: {call}\n\nOutput:\n{head}"),
                        600,
                    )
                    .await
                {
                    Ok((v, _, usage)) => {
                        self.add_cost("page_cards", &usage);
                        calls += 1;
                        let page = v
                            .get("page")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .chars()
                            .take(200)
                            .collect::<String>();
                        let facts: Vec<String> = v
                            .get("facts")
                            .and_then(Value::as_array)
                            .map(|a| {
                                a.iter()
                                    .filter_map(|x| x.as_str())
                                    .map(|s| s.chars().take(120).collect())
                                    .take(14)
                                    .collect()
                            })
                            .unwrap_or_default();
                        let _ = self.store.with(|c| {
                            c.execute(
                                "INSERT OR IGNORE INTO tree_page_cards(page_key, page, facts, created_at) VALUES (?1,?2,?3,?4)",
                                params![key, page, serde_json::to_string(&facts).unwrap_or_default(), now_ms()],
                            )
                        });
                        stats["page_cards"] = json!(stats["page_cards"].as_i64().unwrap_or(0) + 1);
                    }
                    Err(e) => {
                        // Park the page (empty card) so the same leaf is not re-sent on every
                        // pass; a later identical output reuses the empty card and costs nothing.
                        tracing::warn!(error = %e, leaf = %leaf.id, "page card failed; parked");
                        let _ = self.store.with(|c| {
                            c.execute(
                                "INSERT OR IGNORE INTO tree_page_cards(page_key, page, facts, created_at) VALUES (?1, '', '[]', ?2)",
                                params![key, now_ms()],
                            )
                        });
                        errors += 1;
                        calls += 1;
                        if errors >= 3 {
                            break;
                        }
                    }
                }
            }
            let _ = self.store.with(|c| {
                c.execute(
                    "INSERT OR REPLACE INTO tree_leaf_pages(leaf_id, page_key) VALUES (?1, ?2)",
                    params![leaf.id, key],
                )
            });
            pages_last = leaf.id.clone();
        }
        if pages_last != pages_through {
            self.state_set("pages_through", &pages_last);
        }
        // 3. episode cards: closed episodes, or open ones that settled
        let settle_before = now_ms() - SETTLE_MS;
        let retry_before = now_ms() - CARD_FAIL_BACKOFF_MS;
        let todo = self.nodes_where(
            "level = 'episode' AND (status = 'closed' OR (status = 'open' AND t1 < ?1) OR (status = 'card_failed' AND updated_at < ?2)) ORDER BY t1 DESC LIMIT 60",
            &[&settle_before, &retry_before],
        );
        for mut ep in todo {
            if calls >= MAX_CARDS_PER_PASS {
                break;
            }
            let leaves = self.leaves_by_ids(&ep.children);
            let body: Vec<String> = leaves
                .iter()
                .map(|l| {
                    self.leaf_line(
                        l,
                        if l.kind == "user" {
                            4000
                        } else {
                            TOOL_CLIP_FOR_CARDS
                        },
                    )
                })
                .collect();
            let thread = ep
                .thread_id
                .as_ref()
                .and_then(|t| self.node(&format!("thread:{t}")));
            let hint = thread
                .map(|t| format!("thread '{}' ({})", t.title, t.kind.as_str()))
                .unwrap_or_default();
            match self
                .write_card(&model, "episode", &hint, &body.join("\n"))
                .await
            {
                Ok(card) => {
                    calls += 1;
                    ep.heading = card.0;
                    ep.summary = card.1;
                    ep.entities = card.2;
                    ep.status = "ready".into();
                    ep.updated_at = now_ms();
                    self.upsert_node(&ep);
                    self.index_card(&ep);
                    if let Some(t) = &ep.thread_id {
                        self.set_status(&format!("thread:{t}"), "stale");
                    }
                    stats["cards"] = json!(stats["cards"].as_i64().unwrap_or(0) + 1);
                }
                Err(e) => {
                    tracing::warn!(error = %e, node = %ep.id, "episode card failed; retry in an hour");
                    errors += 1;
                    calls += 1;
                    self.set_status(&ep.id, "card_failed");
                    if errors >= 3 {
                        break;
                    }
                }
            }
        }
        // 4. thread cards (debounced) from episode cards
        let debounce = now_ms() - THREAD_CARD_DEBOUNCE_MS;
        let threads = self.nodes_where(
            "level = 'thread' AND ((status = 'stale' AND (updated_at < ?1 OR heading = '')) OR (status = 'card_failed' AND updated_at < ?2)) ORDER BY t1 DESC LIMIT 20",
            &[&debounce, &retry_before],
        );
        for mut th in threads {
            if calls >= MAX_CARDS_PER_PASS {
                break;
            }
            let kids: Vec<Node> = th
                .children
                .iter()
                .filter_map(|c| self.node(c))
                .filter(|n| !n.heading.is_empty())
                .collect();
            if kids.is_empty() {
                continue;
            }
            let body: Vec<String> = kids
                .iter()
                .map(|k| {
                    format!(
                        "- [{} – {}] {} — {}",
                        fmt_ts(k.t0),
                        fmt_ts(k.t1),
                        k.heading,
                        k.summary
                    )
                })
                .collect();
            let hint = format!("{} '{}'", th.kind.as_str(), th.title);
            match self
                .write_card(&model, "thread", &hint, &body.join("\n"))
                .await
            {
                Ok(card) => {
                    calls += 1;
                    th.heading = card.0;
                    th.summary = card.1;
                    th.entities = card.2;
                    th.t0 = kids.iter().map(|k| k.t0).min().unwrap_or(th.t0);
                    th.status = "ready".into();
                    th.updated_at = now_ms();
                    self.upsert_node(&th);
                    self.index_card(&th);
                    self.set_status(ROOT_ID, "stale");
                    stats["cards"] = json!(stats["cards"].as_i64().unwrap_or(0) + 1);
                }
                Err(e) => {
                    tracing::warn!(error = %e, node = %th.id, "thread card failed; retry in an hour");
                    errors += 1;
                    calls += 1;
                    // `stale` is restored by the next episode card, so new content still wins.
                    self.set_status(&th.id, "card_failed");
                    if errors >= 3 {
                        break;
                    }
                }
            }
        }
        // 5. the root card (debounced harder)
        if calls < MAX_CARDS_PER_PASS {
            if let Some(mut root) = self.node(ROOT_ID) {
                let due = (root.status == "stale"
                    && root.updated_at < now_ms() - ROOT_CARD_DEBOUNCE_MS)
                    || (root.status == "card_failed" && root.updated_at < retry_before);
                if due {
                    let kids: Vec<Node> = root
                        .children
                        .iter()
                        .filter_map(|c| self.node(c))
                        .filter(|n| !n.heading.is_empty())
                        .collect();
                    if !kids.is_empty() {
                        let body: Vec<String> = kids
                            .iter()
                            .map(|k| {
                                format!(
                                    "- [{} – {}] {} — {}",
                                    fmt_ts(k.t0),
                                    fmt_ts(k.t1),
                                    k.heading,
                                    k.summary
                                )
                            })
                            .collect();
                        match self
                            .write_card(
                                &model,
                                "project",
                                &self.project_name.clone(),
                                &body.join("\n"),
                            )
                            .await
                        {
                            Ok(card) => {
                                root.heading = card.0;
                                root.summary = card.1;
                                root.entities = card.2;
                                root.t0 = kids.iter().map(|k| k.t0).min().unwrap_or(0);
                                root.t1 = kids.iter().map(|k| k.t1).max().unwrap_or(0);
                                root.status = "ready".into();
                                root.updated_at = now_ms();
                                self.upsert_node(&root);
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "project card failed; retry in an hour");
                                // A thread card sets the root back to `stale`, so new content
                                // still refreshes it before the backoff ends.
                                self.set_status(ROOT_ID, "card_failed");
                            }
                        }
                    }
                }
            }
        }
        stats["ms"] = json!(t0.elapsed().as_millis() as i64);
        Ok(stats)
    }

    fn card_model(&self) -> String {
        self.card_model.clone()
    }

    async fn write_card(
        &self,
        model: &str,
        kind: &str,
        hint: &str,
        body: &str,
    ) -> Result<(String, String, Vec<String>)> {
        let body: String = body.chars().take(24_000).collect();
        let (text, _, usage) = self
            .llm
            .complete_with_usage(
                model,
                &format!("{CARD_SYSTEM}\nAnswer with one JSON object and nothing else."),
                &format!("Node kind: {kind}. Context: {hint}\n\nContent:\n{body}"),
                1200,
            )
            .await?;
        self.add_cost("cards", &usage);
        parse_card(&text).ok_or_else(|| anyhow!("card without heading: {}", clip_chars(&text, 120)))
    }

    /// Node counts by level and status (for the admin/debug view and the roll-out check).
    pub fn stats(&self) -> Value {
        let rows: Vec<(String, String, i64)> = self
            .store
            .with(|c| {
                let mut st = c.prepare(
                    "SELECT level, status, COUNT(*) FROM tree_nodes GROUP BY level, status",
                )?;
                let it = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
                it.collect()
            })
            .unwrap_or_default();
        let pages: i64 = self
            .store
            .with(|c| c.query_row("SELECT COUNT(*) FROM tree_page_cards", [], |r| r.get(0)))
            .unwrap_or(0);
        json!({
            "nodes": rows.iter().map(|(l, s, n)| json!({"level": l, "status": s, "n": n})).collect::<Vec<_>>(),
            "page_cards": pages,
            "built_through": self.state_get("built_through"),
            "spend": self.spend_report(),
        })
    }

    // ---- walk ---------------------------------------------------------------------------------

    /// Threads visible from `chat_id`: a people/group chat sees only itself and the subagents it
    /// spawned; a project chat sees the whole Project.
    fn allowed_threads(&self, chat_id: &str, restricted: bool) -> Option<HashSet<String>> {
        if !restricted {
            return None;
        }
        let mut set = HashSet::new();
        set.insert(chat_id.to_owned());
        if let Ok(subs) = self.store.subagents() {
            for s in subs {
                if s.chat_id.as_deref() == Some(chat_id) {
                    set.insert(s.id);
                }
            }
        }
        Some(set)
    }

    fn card_json(&self, n: &Node) -> Value {
        let mut c = json!({
            "kind": if n.level == "thread" { format!("{} thread", n.kind.as_str()) } else { n.level.clone() },
            "heading": if n.heading.is_empty() { n.title.clone() } else { n.heading.clone() },
            "summary": n.summary,
            "when": format!("{} – {}", fmt_ts(n.t0), fmt_ts(n.t1)),
        });
        if !n.entities.is_empty() {
            c["entities"] = json!(n.entities);
        }
        if n.level == "thread" && !n.title.is_empty() {
            c["title"] = json!(n.title);
        }
        c
    }

    fn leaf_card_json(&self, leaf: &T3Row) -> Value {
        json!({
            "kind": format!("raw {} event", leaf.kind),
            "when": fmt_ts(leaf.at),
            "text": self.leaf_line(leaf, LEAF_CLIP_FOR_JEV),
        })
    }

    /// Score sibling candidates with the decision model; one request per chunk, chunks in parallel.
    async fn score(
        &self,
        scorer: &str,
        query: &Value,
        cards: &[(String, Value)],
        tally: &mut (i64, f64, i64),
    ) -> HashMap<String, f64> {
        let futs = cards.chunks(JEV_CHUNK).map(|chunk| {
            let cands: serde_json::Map<String, Value> = chunk
                .iter()
                .enumerate()
                .map(|(i, (_, card))| (format!("c{i}"), card.clone()))
                .collect();
            let questions: serde_json::Map<String, Value> = (0..chunk.len())
                .map(|i| {
                    let cid = format!("c{i}");
                    (cid.clone(), json!({
                        "type": "noul",
                        "instructions": format!("Could `candidates.{cid}` contain information needed to answer `query`? Judge this candidate on its own, from its heading, summary, time and entities. When in doubt, say yes: missing a needed record is worse than reading an extra one."),
                        "criteria": {
                            "true": "The candidate covers the same subject, person, file, project, decision or event as the query, or an episode where the answer was likely said; or the query asks about a time span the candidate falls in.",
                            "false": "The candidate is about an unrelated subject and nothing in it would help answer the query."
                        }
                    }))
                })
                .collect();
            let state = json!({ "query": query, "candidates": Value::Object(cands) });
            async move {
                let r = self
                    .llm
                    .decide(
                        scorer,
                        &state,
                        &Value::Object(questions),
                        Duration::from_millis(JEV_TIMEOUT_MS),
                    )
                    .await;
                (chunk, r)
            }
        });
        let mut out = HashMap::new();
        for (chunk, r) in join_all(futs).await {
            match r {
                Ok(d) => {
                    tally.0 += 1;
                    tally.1 += d.cost_usd.unwrap_or(0.0);
                    tally.2 = tally.2.max(d.latency_ms);
                    self.add_cost(
                        "walk",
                        &json!({"cost": d.cost_usd.unwrap_or(0.0), "input_tokens": d.input_tokens, "output_tokens": d.output_tokens}),
                    );
                    for (i, (id, _)) in chunk.iter().enumerate() {
                        let p = d
                            .answers
                            .get(&format!("c{i}"))
                            .map(|a| a.probability)
                            .unwrap_or(0.0);
                        out.insert(id.clone(), p);
                    }
                }
                Err(e) => {
                    tracing::debug!(error = %e, "tree walk: decision call failed");
                }
            }
        }
        out
    }

    /// `score` behind the score cache: a candidate whose card was already judged for the same
    /// question words today (same asker, same scope) reuses that probability; only the misses go
    /// to Jev, and their answers are stored. Ids are unique across levels (`thread:`, `ep:`,
    /// leaf ULIDs), so one table serves all three.
    #[allow(clippy::too_many_arguments)]
    async fn score_cached(
        &self,
        scorer: &str,
        query: &Value,
        cards: &[(String, Value)],
        ctx: &str,
        tally: &mut (i64, f64, i64),
        cache_hits: &mut i64,
    ) -> HashMap<String, f64> {
        let keys: Vec<String> = cards
            .iter()
            .map(|(id, card)| sha16(&format!("{id}|{card}|{ctx}")))
            .collect();
        let hits = self.cached_scores(&keys);
        let mut out = HashMap::new();
        let mut todo: Vec<(String, Value)> = Vec::new();
        let mut todo_keys: Vec<String> = Vec::new();
        for ((id, card), key) in cards.iter().zip(&keys) {
            if let Some(p) = hits.get(key) {
                out.insert(id.clone(), *p);
                *cache_hits += 1;
            } else {
                todo.push((id.clone(), card.clone()));
                todo_keys.push(key.clone());
            }
        }
        if !todo.is_empty() {
            let fresh = self.score(scorer, query, &todo, tally).await;
            let puts: Vec<(String, f64)> = todo
                .iter()
                .zip(&todo_keys)
                .filter_map(|((id, _), key)| fresh.get(id).map(|p| (key.clone(), *p)))
                .collect();
            self.cache_put(&puts);
            out.extend(fresh);
        }
        out
    }

    /// Episodes of `threads`: ready ones as cards to score, the rest as leaves to scan directly
    /// (`direct` = leaf id, thread node id).
    fn gather_episodes(&self, threads: &[&Node], scope: &Scope<'_>) -> Gathered {
        let mut g = Gathered::default();
        for t in threads {
            let tid = t.thread_id.as_deref().unwrap_or("");
            for c in &t.children {
                if let Some(mut ep) = self.node(c) {
                    if ep.level != "episode" {
                        continue;
                    }
                    if tid == scope.chat_id {
                        // the asking chat: only what the session no longer holds
                        ep.children.retain(|l| scope.leaf_ok(tid, l));
                        if ep.children.is_empty() {
                            continue;
                        }
                    }
                    if ep.status == "ready" && !ep.heading.is_empty() {
                        g.ep_cards.push((ep.id.clone(), self.card_json(&ep)));
                        g.ep_nodes.insert(ep.id.clone(), ep);
                    } else {
                        g.direct
                            .extend(ep.children.iter().map(|l| (l.clone(), t.id.clone())));
                    }
                }
            }
        }
        g
    }

    /// Keyword hits (bm25) inside the scope: other allowed threads, plus the asking chat's own
    /// compacted-away leaves.
    fn bm25_hits(&self, words: &[String], scope: &Scope<'_>) -> Vec<String> {
        let mut out = Vec::new();
        if let Ok(hits) = self.store.t3_search_any(words, BM25_EXTRA) {
            for (r, _, _) in hits {
                if r.kind == "route" || !scope.leaf_ok(&r.thread_id, &r.id) {
                    continue;
                }
                out.push(r.id);
            }
        }
        out
    }

    /// After a session compaction (D91): the chat's leaves older than the kept tail left the
    /// model's window. Record the oldest kept leaf; everything before it becomes a walk
    /// candidate for questions asked in that chat. The tail is estimated at half of pi's
    /// `keepRecentTokens` (T3 holds tool outputs clipped, so its token counts run low): the cut
    /// errs towards offering a few leaves the window still has — Jev drops what is not needed —
    /// never towards hiding one that is gone.
    pub fn note_session_cut(&self, chat_id: &str, keep_recent_tokens: usize) -> Option<String> {
        let rows = self.t3_rows(
            "WHERE thread_id = ?1 AND kind <> 'route' ORDER BY id DESC LIMIT 2000",
            &[&chat_id],
        );
        let budget = keep_recent_tokens / 2;
        let mut acc = 0usize;
        let mut cut: Option<String> = None;
        for r in &rows {
            acc += est_tokens(&r.text);
            cut = Some(r.id.clone());
            if acc >= budget {
                break;
            }
        }
        let key = format!("session_cut:{chat_id}");
        match (&cut, acc >= budget) {
            (Some(id), true) => {
                self.state_set(&key, id);
                cut
            }
            _ => {
                // the whole chat fits in the kept tail: nothing was summarised away
                let _ = self
                    .store
                    .with(|c| c.execute("DELETE FROM tree_state WHERE key = ?1", params![key]));
                None
            }
        }
    }

    fn session_cut(&self, chat_id: &str) -> Option<String> {
        self.state_get(&format!("session_cut:{chat_id}"))
    }

    /// Threads to expand, best first. A thread with keyword evidence is kept above the threshold
    /// within the beam, as the group's own thread, or as one of the top two (recall over
    /// precision, as in D77). A thread without evidence rode along in the same request and is
    /// kept only when Jev itself says so.
    #[allow(clippy::too_many_arguments)]
    fn keep_threads<'a>(
        &self,
        evidence: &'a [Node],
        ride: &'a [Node],
        probs: &HashMap<String, f64>,
        chat_id: &str,
        restricted: bool,
        pack: &mut Pack,
    ) -> Vec<&'a Node> {
        let mut ranked: Vec<(&Node, bool)> = evidence
            .iter()
            .map(|n| (n, true))
            .chain(ride.iter().map(|n| (n, false)))
            .collect();
        ranked.sort_by(|(a, _), (b, _)| {
            probs
                .get(&b.id)
                .unwrap_or(&0.0)
                .partial_cmp(probs.get(&a.id).unwrap_or(&0.0))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut kept: Vec<&Node> = Vec::new();
        let mut kept_evidence = 0usize;
        let mut per_kind: HashMap<&str, usize> = HashMap::new();
        for (n, has_evidence) in ranked {
            let p = *probs.get(&n.id).unwrap_or(&0.0);
            let c = per_kind.entry(n.kind.as_str()).or_insert(0);
            let scored = p >= n.kind.threshold("thread") && *c < n.kind.beam("thread");
            let keep = scored
                || (has_evidence
                    && ((restricted && n.thread_id.as_deref() == Some(chat_id))
                        || kept_evidence < 2));
            pack.decisions.push(json!({ "level": "thread", "node_id": n.id, "p": (p * 1000.0).round() / 1000.0, "thr": n.kind.threshold("thread"), "expanded": keep, "evidence": has_evidence, "heading": clip_chars(&n.heading, 120) }));
            if keep {
                *c += 1;
                if has_evidence {
                    kept_evidence += 1;
                }
                kept.push(n);
            }
        }
        kept
    }

    /// Episodes to expand, best first: above the threshold within the beam, and always the top
    /// two. Ties break on the id so the order is the same in every run.
    fn keep_episodes(
        &self,
        ep_nodes: &HashMap<String, Node>,
        probs: &HashMap<String, f64>,
        pack: &mut Pack,
    ) -> Vec<Node> {
        let mut ranked: Vec<&Node> = ep_nodes.values().collect();
        ranked.sort_by(|a, b| {
            probs
                .get(&b.id)
                .unwrap_or(&0.0)
                .partial_cmp(probs.get(&a.id).unwrap_or(&0.0))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.id.cmp(&b.id))
        });
        let mut kept: Vec<Node> = Vec::new();
        let mut per_kind: HashMap<&str, usize> = HashMap::new();
        for n in ranked {
            let p = *probs.get(&n.id).unwrap_or(&0.0);
            let c = per_kind.entry(n.kind.as_str()).or_insert(0);
            let keep =
                (p >= n.kind.threshold("episode") && *c < n.kind.beam("episode")) || kept.len() < 2;
            pack.decisions.push(json!({ "level": "episode", "node_id": n.id, "p": (p * 1000.0).round() / 1000.0, "thr": n.kind.threshold("episode"), "expanded": keep, "heading": clip_chars(&n.heading, 120) }));
            if keep {
                *c += 1;
                kept.push(n.clone());
            }
        }
        kept
    }

    /// One scoring round over the candidates not sent to Jev yet: through the cache when
    /// `fast`, straight to Jev otherwise. Nothing new → no request.
    #[allow(clippy::too_many_arguments)]
    async fn score_round(
        &self,
        fast: bool,
        scorer: &str,
        query: &Value,
        cards: Vec<(String, Value)>,
        ctx: &str,
        asked: &mut HashSet<String>,
        probs: &mut HashMap<String, f64>,
        tally: &mut (i64, f64, i64),
        cache_hits: &mut i64,
    ) {
        let todo: Vec<(String, Value)> = cards
            .into_iter()
            .filter(|(id, _)| asked.insert(id.clone()))
            .collect();
        if todo.is_empty() {
            return;
        }
        let got = if fast {
            self.score_cached(scorer, query, &todo, ctx, tally, cache_hits)
                .await
        } else {
            self.score(scorer, query, &todo, tally).await
        };
        probs.extend(got);
    }

    /// The walk proper. `chat_id` is the chat the question is asked in (its recent window is
    /// already in the model's session, so it is excluded); `restricted` limits the walk to that
    /// chat's subtree.
    ///
    /// With `opts.fast` off this is the D77 walk: every other thread scored, then the ready
    /// episodes of the kept threads, then their leaves plus keyword hits — three Jev rounds,
    /// every score fresh. With it on, three things change and nothing else:
    /// 1. **Prefilter.** A thread with no stemmed content word in common with the question (in a
    ///    leaf or an index card) is never expanded on its own: when no thread has evidence the
    ///    walk ends here with no Jev request at all; otherwise such threads ride along in the
    ///    first request (no extra round trip) and are expanded only when Jev says yes.
    /// 2. **Cache.** Every score goes through `tree_score_cache` (card + question words + day).
    /// 3. **One round when small.** With few evidence threads, their episodes and leaves are
    ///    scored in the same request as the thread cards; Jev judges each candidate on its own,
    ///    so the keep rules give the same result as three rounds would.
    #[allow(clippy::too_many_arguments)]
    async fn walk_inner(
        &self,
        scorer: &str,
        chat_id: &str,
        restricted: bool,
        query_text: &str,
        asker: &str,
        prompt_memory: &str,
        opts: WalkOptions,
    ) -> Pack {
        let t0 = Instant::now();
        let asked_at = now_ms();
        let mut pack = Pack::default();
        let scope = Scope {
            chat_id,
            allowed: self.allowed_threads(chat_id, restricted),
            own_cut: self.session_cut(chat_id),
        };
        let words = content_words(query_text);
        let query = json!({ "text": query_text, "asked_at": fmt_ts(asked_at), "asked_in": if restricted { "a group or people chat" } else { "a chat with the project owner" }, "asked_by": asker });
        let ctx = cache_ctx(&words, asked_at, asker, restricted);
        let mut tally = (0i64, 0f64, 0i64);
        let mut cache_hits = 0i64;
        let fast = opts.fast;
        let compact = opts.compact;
        // level 1 candidates: every other visible thread — and the asking chat itself once a
        // compaction has moved part of it out of the model's window — split (fast) into those
        // with keyword evidence for the question and those without
        let mut evidence: Vec<Node> = self
            .nodes_where("level = 'thread' ORDER BY t1 DESC LIMIT 400", &[])
            .into_iter()
            .filter(|n| match n.thread_id.as_deref() {
                Some(t) if t == chat_id => scope.own_cut.is_some(),
                Some(t) => scope.thread_ok(t),
                None => false,
            })
            .collect();
        let mut ride: Vec<Node> = Vec::new();
        pack.prefilter = if fast { "no-words" } else { "off" };
        if fast {
            let thread_count = evidence.len();
            if let Some(found) = self.threads_with_evidence(&words, thread_count) {
                let (yes, no): (Vec<Node>, Vec<Node>) = evidence
                    .into_iter()
                    .partition(|n| n.thread_id.as_ref().is_some_and(|t| found.contains(t)));
                evidence = yes;
                ride = no;
                pack.prefilter = "evidence";
                if evidence.is_empty() && !ride.is_empty() && first_person(query_text) {
                    evidence = std::mem::take(&mut ride);
                    pack.prefilter = "first-person";
                }
            }
        }
        if restricted
            && !evidence
                .iter()
                .any(|n| n.thread_id.as_deref() == Some(chat_id))
        {
            // the group's own thread is the subtree root; keep its episodes
            if let Some(n) = self.node(&format!("thread:{chat_id}")) {
                evidence.push(n);
            }
        }
        pack.prefiltered = ride.len();
        if evidence.is_empty() {
            pack.outcome = "empty".into();
            pack.reason = Some(if ride.is_empty() {
                "no threads".into()
            } else {
                "no keyword match".into()
            });
            pack.walk_ms = t0.elapsed().as_millis() as i64;
            return pack;
        }
        let mut probs: HashMap<String, f64> = HashMap::new();
        let mut asked: HashSet<String> = HashSet::new();
        let mut cards: Vec<(String, Value)> = evidence
            .iter()
            .chain(ride.iter())
            .map(|n| (n.id.clone(), self.card_json(n)))
            .collect();
        // Shape: with few evidence threads, gather their episodes and leaves now; if the whole
        // set is small, round 1 scores all three levels at once.
        let mut pre: Option<Gathered> = None;
        let mut hits: Option<Vec<String>> = None;
        let mut loaded: HashMap<String, T3Row> = HashMap::new();
        let mut leaf_via: HashMap<String, Via> = HashMap::new();
        let mut leaf_kind: HashMap<String, Kind> = HashMap::new();
        if fast && evidence.len() <= FLAT_MAX_THREADS {
            let all: Vec<&Node> = evidence.iter().collect();
            let g = self.gather_episodes(&all, &scope);
            let h = self.bm25_hits(&words, &scope);
            let mut ids: Vec<String> = Vec::new();
            for ep in g.ep_nodes.values() {
                for l in &ep.children {
                    ids.push(l.clone());
                    leaf_via.insert(l.clone(), Via::Episode(ep.id.clone()));
                    leaf_kind.insert(l.clone(), ep.kind);
                }
            }
            for (l, t) in &g.direct {
                ids.push(l.clone());
                leaf_via.entry(l.clone()).or_insert(Via::Thread(t.clone()));
            }
            ids.extend(h.iter().cloned());
            let mut seen = HashSet::new();
            ids.retain(|l| seen.insert(l.clone()));
            if evidence.len() + g.ep_cards.len() + ids.len() <= FLAT_MAX_CANDIDATES {
                pack.flat = true;
                for l in self.leaves_by_ids(&ids) {
                    loaded.insert(l.id.clone(), l);
                }
                if loaded.is_empty() {
                    // nothing to pack, so nothing to score: not one Jev request
                    pack.outcome = "empty".into();
                    pack.reason = Some("no candidate leaves".into());
                    pack.walk_ms = t0.elapsed().as_millis() as i64;
                    return pack;
                }
                cards.extend(g.ep_cards.iter().cloned());
                cards.extend(
                    ids.iter()
                        .filter_map(|id| loaded.get(id))
                        .map(|l| (l.id.clone(), self.leaf_card_json(l))),
                );
            }
            pre = Some(g);
            hits = Some(h);
        }
        // round 1: threads (and, when small, everything under them)
        self.score_round(
            fast,
            scorer,
            &query,
            cards,
            &ctx,
            &mut asked,
            &mut probs,
            &mut tally,
            &mut cache_hits,
        )
        .await;
        let keep_threads =
            self.keep_threads(&evidence, &ride, &probs, chat_id, restricted, &mut pack);
        // level 2: ready episodes of the kept threads are scored; open/closed ones go straight
        // to the leaf level
        let g = match pre.take() {
            Some(g) => {
                let kept_ids: HashSet<&String> = keep_threads.iter().map(|t| &t.id).collect();
                let ep_nodes: HashMap<String, Node> = g
                    .ep_nodes
                    .into_iter()
                    .filter(|(_, n)| n.parents.first().is_some_and(|p| kept_ids.contains(p)))
                    .collect();
                let mut g2 = Gathered {
                    ep_cards: g
                        .ep_cards
                        .into_iter()
                        .filter(|(id, _)| ep_nodes.contains_key(id))
                        .collect(),
                    ep_nodes,
                    direct: g
                        .direct
                        .into_iter()
                        .filter(|(_, t)| kept_ids.contains(t))
                        .collect(),
                };
                // ride-along threads Jev kept were not pre-gathered
                let late: Vec<&Node> = keep_threads
                    .iter()
                    .copied()
                    .filter(|t| ride.iter().any(|r| r.id == t.id))
                    .collect();
                if !late.is_empty() {
                    let gl = self.gather_episodes(&late, &scope);
                    g2.ep_cards.extend(gl.ep_cards);
                    g2.ep_nodes.extend(gl.ep_nodes);
                    g2.direct.extend(gl.direct);
                }
                g2
            }
            None => self.gather_episodes(&keep_threads, &scope),
        };
        // round 2: episode cards not scored yet (none when round 1 covered them)
        self.score_round(
            fast,
            scorer,
            &query,
            g.ep_cards.clone(),
            &ctx,
            &mut asked,
            &mut probs,
            &mut tally,
            &mut cache_hits,
        )
        .await;
        let keep_eps = self.keep_episodes(&g.ep_nodes, &probs, &mut pack);
        // level 3: leaves of kept episodes + unsummarized leaves of kept threads + keyword hits
        let mut leaf_ids: Vec<String> = Vec::new();
        for ep in &keep_eps {
            for l in &ep.children {
                leaf_ids.push(l.clone());
                leaf_via.insert(l.clone(), Via::Episode(ep.id.clone()));
                leaf_kind.insert(l.clone(), ep.kind);
            }
        }
        for (l, t) in &g.direct {
            leaf_ids.push(l.clone());
            leaf_via.entry(l.clone()).or_insert(Via::Thread(t.clone()));
        }
        let hits = match hits.take() {
            Some(h) => h,
            None => self.bm25_hits(&words, &scope),
        };
        leaf_ids.extend(hits.iter().cloned());
        let mut seen = HashSet::new();
        leaf_ids.retain(|l| seen.insert(l.clone()));
        let missing: Vec<String> = leaf_ids
            .iter()
            .filter(|l| !loaded.contains_key(*l))
            .cloned()
            .collect();
        for l in self.leaves_by_ids(&missing) {
            loaded.insert(l.id.clone(), l);
        }
        let mut leaves: Vec<&T3Row> = leaf_ids.iter().filter_map(|l| loaded.get(l)).collect();
        leaves.sort_by(|a, b| (a.at, &a.id).cmp(&(b.at, &b.id)));
        if leaves.is_empty() {
            pack.outcome = "empty".into();
            pack.reason = Some("no candidate leaves".into());
            pack.jev_calls = tally.0;
            pack.jev_cost = tally.1;
            pack.jev_ms = tally.2;
            pack.cache_hits = cache_hits;
            pack.walk_ms = t0.elapsed().as_millis() as i64;
            return pack;
        }
        // round 3: leaf cards not scored yet (none when round 1 covered them)
        let leaf_cards: Vec<(String, Value)> = leaves
            .iter()
            .map(|l| (l.id.clone(), self.leaf_card_json(l)))
            .collect();
        self.score_round(
            fast,
            scorer,
            &query,
            leaf_cards,
            &ctx,
            &mut asked,
            &mut probs,
            &mut tally,
            &mut cache_hits,
        )
        .await;
        // leaf stage: a leaf reached through the tree needs its parent kept; a keyword hit
        // stands on its own (as before: keyword hits were extra candidates in every walk)
        let kept_thread_ids: HashSet<&String> = keep_threads.iter().map(|t| &t.id).collect();
        let kept_ep_ids: HashSet<&String> = keep_eps.iter().map(|e| &e.id).collect();
        let bm25_set: HashSet<&String> = hits.iter().collect();
        let mut kept: Vec<&T3Row> = Vec::new();
        for l in &leaves {
            let via_tree = match leaf_via.get(&l.id) {
                Some(Via::Episode(ep)) => kept_ep_ids.contains(ep),
                Some(Via::Thread(t)) => kept_thread_ids.contains(t),
                None => false,
            };
            if !via_tree && !bm25_set.contains(&l.id) {
                continue;
            }
            let kind = match leaf_kind.get(&l.id) {
                Some(k) if via_tree => *k,
                _ => {
                    let k = self.thread_meta(&l.thread_id).1;
                    leaf_kind.insert(l.id.clone(), k);
                    k
                }
            };
            let p = *probs.get(&l.id).unwrap_or(&0.0);
            let keep = p >= kind.threshold("leaf");
            pack.decisions.push(json!({ "level": "leaf", "node_id": l.id, "p": (p * 1000.0).round() / 1000.0, "thr": kind.threshold("leaf"), "expanded": keep, "source": if via_tree { "tree" } else { "bm25" }, "heading": clip_chars(&l.text, 100) }));
            if keep {
                kept.push(l);
            }
        }
        let mut blocks: Vec<(i64, String, usize, String)> = Vec::new(); // (at, text, tokens, leaf id)
        let mut total: usize = 0;
        if compact {
            self.pack_compact(
                &mut kept,
                &leaf_kind,
                prompt_memory,
                &mut pack,
                &mut blocks,
                &mut total,
            );
        } else {
            // full: time order, one budget by content kind, oldest dropped when over budget
            let any_tool = kept
                .iter()
                .any(|l| leaf_kind.get(&l.id).copied().unwrap_or(Kind::Chat) == Kind::Tool);
            let budget = if any_tool {
                BUDGET_TOOL_FULL
            } else {
                BUDGET_CHAT_FULL
            };
            kept.sort_by_key(|l| (l.at, l.id.clone()));
            for l in &kept {
                let kind = leaf_kind.get(&l.id).copied().unwrap_or(Kind::Chat);
                let line = self.leaf_line(l, kind.clip_full());
                let tok = est_tokens(&line);
                blocks.push((l.at, line, tok, l.id.clone()));
            }
            total = blocks.iter().map(|b| b.2).sum();
            let mut i = 0;
            while total > budget && i < blocks.len() {
                total -= blocks[i].2;
                blocks[i].1.clear();
                i += 1;
            }
        }
        let mut out = String::new();
        let mut cur_thread = String::new();
        let mut thread_headings: HashMap<String, String> = HashMap::new();
        for t in &keep_threads {
            if let Some(tid) = &t.thread_id {
                thread_headings.insert(
                    tid.clone(),
                    if t.heading.is_empty() {
                        t.title.clone()
                    } else {
                        format!("{} — {}", t.title, t.heading)
                    },
                );
            }
        }
        let leaf_thread: HashMap<&str, &str> = kept
            .iter()
            .map(|l| (l.id.as_str(), l.thread_id.as_str()))
            .collect();
        for (_, line, _, id) in &blocks {
            if line.is_empty() {
                continue;
            }
            let tid = leaf_thread.get(id.as_str()).copied().unwrap_or("");
            if tid != cur_thread {
                cur_thread = tid.to_owned();
                let (title, _, _) = self.thread_meta(tid);
                let h = thread_headings.get(tid).cloned().unwrap_or(title);
                // #1722: records from another chat are labelled so "Done?" is never answered
                // about a different chat's folder.
                if tid == chat_id {
                    out.push_str(&format!("\n## {h} [thread {tid} — this chat]\n"));
                } else {
                    out.push_str(&format!("\n## {h} [thread {tid} — another chat]\n"));
                }
            }
            out.push_str(line);
            out.push('\n');
            pack.leaf_ids.push(id.clone());
        }
        pack.node_ids = keep_threads
            .iter()
            .map(|n| n.id.clone())
            .chain(keep_eps.iter().map(|n| n.id.clone()))
            .collect();
        pack.tokens = total;
        pack.text = out.trim().to_owned();
        pack.jev_calls = tally.0;
        pack.jev_cost = tally.1;
        pack.jev_ms = tally.2;
        pack.cache_hits = cache_hits;
        pack.walk_ms = t0.elapsed().as_millis() as i64;
        pack.outcome = if pack.text.is_empty() {
            "empty".into()
        } else {
            "ok".into()
        };
        pack
    }

    /// Run the walk for one turn with a deadline, log it, and return the text to inject (None =
    /// current behaviour). `mode` decides whether an ok pack is injected or only logged.
    #[allow(clippy::too_many_arguments)]
    pub async fn pack_for_turn(
        &self,
        mode: TreeMode,
        scorer: &str,
        chat_id: &str,
        message_id: Option<&str>,
        purpose: &str,
        restricted: bool,
        query_text: &str,
        asker: &str,
        recent_window: &Value,
        prompt_memory: &str,
        opts: WalkOptions,
    ) -> Option<String> {
        if mode == TreeMode::Off {
            return None;
        }
        let compact = opts.compact;
        let built = self.state_get("built_through").is_some();
        let mut pack = if !built {
            Pack {
                outcome: "fallback".into(),
                reason: Some("tree not built yet".into()),
                ..Default::default()
            }
        } else if self.capped() {
            Pack {
                outcome: "fallback".into(),
                reason: Some("daily cap".into()),
                ..Default::default()
            }
        } else {
            match tokio::time::timeout(
                Duration::from_millis(WALK_DEADLINE_MS + 600),
                self.walk_inner(
                    scorer,
                    chat_id,
                    restricted,
                    query_text,
                    asker,
                    prompt_memory,
                    opts,
                ),
            )
            .await
            {
                Ok(p) => p,
                Err(_) => Pack {
                    outcome: "fallback".into(),
                    reason: Some(format!("walk timed out after {}ms", WALK_DEADLINE_MS + 600)),
                    ..Default::default()
                },
            }
        };
        let inject = mode == TreeMode::On && pack.outcome == "ok";
        if pack.outcome == "ok" && !inject {
            pack.outcome = "logged".into();
        } else if inject {
            pack.outcome = "injected".into();
        }
        // one summary row per walk so the log answers "how often did the fast path fire" without
        // a schema change
        pack.decisions.push(json!({ "level": "walk", "fast": opts.fast, "prefilter": pack.prefilter, "flat": pack.flat, "prefiltered": pack.prefiltered, "cache_hits": pack.cache_hits, "jev_calls": pack.jev_calls }));
        let _ = self.store.with(|c| {
            c.execute(
                "INSERT INTO tree_walk_log(id, chat_id, message_id, purpose, mode, outcome, reason, query, pack, decisions, jev_calls, jev_cost, jev_ms, walk_ms, pack_tokens, recent_window, created_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)",
                params![
                    ulid(), chat_id, message_id, purpose, mode.as_str(), pack.outcome, pack.reason, clip_chars(query_text, 600),
                    json!({ "node_ids": pack.node_ids, "leaf_ids": pack.leaf_ids, "text": pack.text }).to_string(),
                    Value::Array(pack.decisions.clone()).to_string(), pack.jev_calls, pack.jev_cost, pack.jev_ms, pack.walk_ms, pack.tokens as i64,
                    recent_window.to_string(), now_ms()
                ],
            )
        });
        let _ = self.store.log_decision_with_usage(
            "read",
            message_id,
            Some(&json!({ "purpose": purpose, "outcome": pack.outcome, "reason": pack.reason, "nodes": pack.node_ids.len(), "leaves": pack.leaf_ids.len(), "tokens": pack.tokens })),
            Some(if inject { "INJECT" } else { "FALLBACK" }),
            &pack.leaf_ids,
            pack.reason.as_deref(),
            None,
            Some(scorer),
            Some(pack.walk_ms),
            Some(&json!({ "provider": "jev", "calls": pack.jev_calls, "cost_usd": pack.jev_cost, "max_latency_ms": pack.jev_ms })),
        );
        tracing::info!(chat = chat_id, purpose, outcome = %pack.outcome, leaves = pack.leaf_ids.len(), tokens = pack.tokens, jev_calls = pack.jev_calls, cache_hits = pack.cache_hits, prefilter = pack.prefilter, prefiltered = pack.prefiltered, flat = pack.flat, cost_usd = pack.jev_cost, walk_ms = pack.walk_ms, "memory tree walk");
        if !inject {
            return None;
        }
        let rules = if compact {
            "[Memory retrieved for this message from earlier chats, tool results and subagent sessions; raw records, oldest first; each heading says whether its thread is this chat or another chat. Background only: this chat's own messages and the current state of your folder and jobs outrank every record here, and a question with no named object (\"Done?\", \"the last number\") is about this chat's work — check it, never answer it from another chat's record. What the user told you about themselves (name, preferences, facts) is true and outranks anything you said before or any handle you see; between two user statements the newer wins; if the records show the question's premise is wrong, say so. Say briefly what you checked; if neither this chat nor the records answer it, say you have no record rather than guessing. More via context_search / context_open / context_around.]"
        } else {
            "[Memory retrieved from your earlier chats, tool results and subagent sessions for this message. Raw records, oldest first; each heading says whether its thread is this chat or another chat. Background only: this chat's own messages and the current state of your folder and jobs outrank every record here, and a question with no named object (\"Done?\", \"the last number\") is about this chat's work — check it, never answer it from another chat's record. Rules: what the user told you about themselves (their name, preferences, facts) is true and outranks anything you said before and any handle or label you see; between two user statements the newer wins; if the records show the question's premise is wrong, say so. Say in a few words what you checked. If neither this chat nor the records answer the question, say you have no record of it rather than guessing. You can read more with context_search / context_open / context_around.]"
        };
        Some(format!("{rules}\n{}\n[End of retrieved memory]", pack.text))
    }

    /// Offline check (`arbos-agentd eval-tree`): build until nothing is left to card, then walk
    /// one query from `chat_id` and print the pack.
    pub async fn eval(
        &self,
        scorer: &str,
        chat_id: &str,
        query: &str,
        opts: WalkOptions,
    ) -> Result<Value> {
        let mut passes = Vec::new();
        for _ in 0..30 {
            let st = self.build_pass().await?;
            let did = st.get("cards").and_then(Value::as_i64).unwrap_or(0)
                + st.get("page_cards").and_then(Value::as_i64).unwrap_or(0)
                + st.get("leaves").and_then(Value::as_i64).unwrap_or(0);
            passes.push(st.clone());
            if did == 0
                || st
                    .get("skipped_cap")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            {
                break;
            }
        }
        // the same T0/T1 text the prompt's project-memory block holds (threads omitted)
        let mut prompt_memory = String::new();
        for tier in ["T0", "T1"] {
            for m in self.store.memory_live(tier, None, 80).unwrap_or_default() {
                prompt_memory.push_str(&format!("- [{}] {}\n", m.kind, m.text));
            }
        }
        let pack = self
            .walk_inner(
                scorer,
                chat_id,
                false,
                query,
                "the project owner",
                &prompt_memory,
                opts,
            )
            .await;
        Ok(json!({
            "passes": passes,
            "stats": self.stats(),
            "pack": { "outcome": pack.outcome, "reason": pack.reason, "tokens": pack.tokens, "leaves": pack.leaf_ids, "nodes": pack.node_ids, "jev_calls": pack.jev_calls, "jev_cost": pack.jev_cost, "jev_ms": pack.jev_ms, "walk_ms": pack.walk_ms, "cache_hits": pack.cache_hits, "prefilter": pack.prefilter, "prefiltered": pack.prefiltered, "flat": pack.flat, "text": pack.text },
            "decisions": pack.decisions,
        }))
    }

    // ---- pull tools (MCP) ---------------------------------------------------------------------

    pub fn tool_search(&self, chat_id: &str, restricted: bool, query: &str, limit: i64) -> Value {
        let allowed = self.allowed_threads(chat_id, restricted);
        let ok = |tid: &str| allowed.as_ref().map(|a| a.contains(tid)).unwrap_or(true);
        let hits: Vec<Value> = self
            .store
            .t3_search(query, None, None, limit.clamp(1, 25), 0)
            .unwrap_or_default()
            .into_iter()
            .filter(|(r, _, _)| r.kind != "route" && ok(&r.thread_id))
            .map(|(r, score, snip)| json!({ "id": r.id, "kind": format!("leaf/{}", r.kind), "thread_id": r.thread_id, "when": fmt_ts(r.at), "score": -score, "snippet": clip_chars(&snip, 300) }))
            .collect();
        let words: HashSet<String> = query
            .to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.len() > 2)
            .map(str::to_owned)
            .collect();
        let mut cards: Vec<(usize, Value)> = self
            .nodes_where("level IN ('thread','episode') AND heading <> ''", &[])
            .into_iter()
            .filter(|n| n.thread_id.as_deref().map(ok).unwrap_or(true))
            .filter_map(|n| {
                let hay = format!("{} {} {} {}", n.title, n.heading, n.summary, n.entities.join(" ")).to_lowercase();
                let ov = words.iter().filter(|w| hay.contains(w.as_str())).count();
                (ov > 0).then(|| (ov, json!({ "id": n.id, "kind": n.level, "when": format!("{} – {}", fmt_ts(n.t0), fmt_ts(n.t1)), "heading": n.heading, "summary": n.summary, "children": n.children.len() })))
            })
            .collect();
        cards.sort_by_key(|c| std::cmp::Reverse(c.0));
        json!({ "events": hits, "index_cards": cards.into_iter().take(8).map(|c| c.1).collect::<Vec<_>>() })
    }

    pub fn tool_open(&self, chat_id: &str, restricted: bool, node_id: &str) -> Value {
        let allowed = self.allowed_threads(chat_id, restricted);
        let ok = |tid: &str| allowed.as_ref().map(|a| a.contains(tid)).unwrap_or(true);
        if let Some(n) = self.node(node_id) {
            if n.thread_id.as_deref().map(|t| !ok(t)).unwrap_or(false) {
                return json!({ "error": "not visible from this chat" });
            }
            if n.level == "episode" {
                let leaves = self.leaves_by_ids(&n.children);
                let mut lines = Vec::new();
                let mut used = 0usize;
                for l in &leaves {
                    let line = self.leaf_line(l, 4000);
                    used += line.chars().count();
                    if used > 40_000 {
                        lines.push(format!(
                            "…[{} more events; use context_around]",
                            leaves.len() - lines.len()
                        ));
                        break;
                    }
                    lines.push(line);
                }
                return json!({ "id": n.id, "heading": n.heading, "summary": n.summary, "events": lines });
            }
            let kids: Vec<Value> = n
                .children
                .iter()
                .filter_map(|c| self.node(c))
                .filter(|k| k.thread_id.as_deref().map(ok).unwrap_or(true))
                .map(|k| json!({ "id": k.id, "kind": k.level, "when": format!("{} – {}", fmt_ts(k.t0), fmt_ts(k.t1)), "heading": if k.heading.is_empty() { k.title.clone() } else { k.heading.clone() }, "summary": k.summary, "status": k.status, "children": k.children.len() }))
                .collect();
            return json!({ "id": n.id, "kind": n.level, "heading": n.heading, "summary": n.summary, "children": kids });
        }
        match self.store.t3_get(node_id) {
            Ok(Some(l)) if ok(&l.thread_id) => {
                json!({ "id": l.id, "thread_id": l.thread_id, "text": self.leaf_line(&l, 12_000) })
            }
            Ok(Some(_)) => json!({ "error": "not visible from this chat" }),
            _ => json!({ "error": "no such node or event" }),
        }
    }

    pub fn tool_around(
        &self,
        chat_id: &str,
        restricted: bool,
        leaf_id: &str,
        before: i64,
        after: i64,
    ) -> Value {
        let allowed = self.allowed_threads(chat_id, restricted);
        let Ok(Some(l)) = self.store.t3_get(leaf_id) else {
            return json!({ "error": "no such event" });
        };
        if let Some(a) = &allowed {
            if !a.contains(&l.thread_id) {
                return json!({ "error": "not visible from this chat" });
            }
        }
        let before = before.clamp(0, 20);
        let after = after.clamp(0, 20);
        let mut rows = self.t3_rows(
            "WHERE thread_id = ?1 AND kind <> 'route' AND id < ?2 ORDER BY id DESC LIMIT ?3",
            &[&l.thread_id, &leaf_id, &before],
        );
        rows.reverse();
        rows.push(l.clone());
        rows.extend(self.t3_rows(
            "WHERE thread_id = ?1 AND kind <> 'route' AND id > ?2 ORDER BY id ASC LIMIT ?3",
            &[&l.thread_id, &leaf_id, &after],
        ));
        let (title, _, _) = self.thread_meta(&l.thread_id);
        json!({ "thread": title, "thread_id": l.thread_id, "events": rows.iter().map(|r| self.leaf_line(r, 2000)).collect::<Vec<_>>() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_parses() {
        assert_eq!(TreeMode::parse("on"), Some(TreeMode::On));
        assert_eq!(TreeMode::parse("LOG"), Some(TreeMode::Log));
        assert_eq!(TreeMode::parse("off"), Some(TreeMode::Off));
        assert_eq!(TreeMode::parse("maybe"), None);
    }

    #[test]
    fn thresholds_per_kind() {
        assert_eq!(Kind::Chat.threshold("thread"), 0.3);
        assert_eq!(Kind::Chat.threshold("leaf"), 0.4);
        assert_eq!(Kind::Tool.threshold("thread"), 0.2);
        assert_eq!(Kind::Tool.threshold("leaf"), 0.4);
    }

    #[test]
    fn card_parses_truncated_json() {
        let full = parse_card(r#"{"heading":"H","summary":"S","entities":["a"]}"#).unwrap();
        assert_eq!(full.0, "H");
        let cut = parse_card(r#"{"heading": "Only head", "summary": "cut off mid"#).unwrap();
        assert_eq!(cut.0, "Only head");
        assert_eq!(cut.1, "cut off mid");
        assert!(parse_card("no card here").is_none());
    }

    #[test]
    fn clip_counts_chars() {
        assert_eq!(clip_chars("héllo", 10), "héllo");
        assert!(clip_chars("héllo wörld", 5).starts_with("héllo …["));
    }

    #[test]
    fn first_person_markers() {
        assert!(first_person("What breed is my pet?"));
        assert!(first_person("Did we decide on a port?"));
        assert!(first_person("I'm allergic to what again?"));
        assert!(!first_person("Write me a haiku about autumn rain."));
        assert!(!first_person("Explain quicksort in two sentences."));
    }

    #[test]
    fn fts_or_quotes_terms() {
        assert_eq!(
            fts_or(&["name".into(), "tree.rs".into()]).as_deref(),
            Some("\"name\" OR \"tree.rs\"")
        );
        assert_eq!(fts_or(&[]), None);
    }

    // ---- cross-chat recall: the fast walk must find what the D77 walk finds ---------------------
    //
    // A local stand-in for the relay proxy serves the two model routes the tree uses. Cards come
    // back as JSON built from the text (heading = first line, summary = the lines, entities =
    // capitalised words), so they carry the words a real Haiku card would. The decision route
    // scores a candidate 0.9 when it shares a word stem (or a listed synonym) with the question,
    // else 0.05 — deterministic, so the two arms see identical judgements and any difference in
    // the packed leaves is the walk's own doing.

    const SYNONYMS: &[(&str, &[&str])] = &[
        ("pet", &["dog", "cat", "retriever", "tabby"]),
        ("breed", &["retriever", "tabby"]),
        ("called", &["name"]),
    ];

    fn stem5(w: &str) -> String {
        w.chars().take(5).collect()
    }

    fn mock_score(query: &str, candidate: &str) -> f64 {
        let hay: Vec<String> = candidate
            .to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.len() > 2)
            .map(stem5)
            .collect();
        let mut needles: Vec<String> = content_words(query).iter().map(|w| stem5(w)).collect();
        for w in content_words(query) {
            if let Some((_, syns)) = SYNONYMS.iter().find(|(k, _)| *k == w) {
                needles.extend(syns.iter().map(|s| stem5(s)));
            }
        }
        if needles.iter().any(|n| hay.iter().any(|h| h == n)) {
            0.9
        } else {
            0.05
        }
    }

    fn strip_leaf_prefix(line: &str) -> &str {
        // "[2026-10-21 12:00 UTC] user (user): text" or "- [a – b] heading — summary"
        match line.find("): ") {
            Some(i) => &line[i + 3..],
            None => line.trim_start_matches("- "),
        }
    }

    async fn mock_cards(axum::Json(body): axum::Json<Value>) -> axum::Json<Value> {
        let system = body["messages"][0]["content"].as_str().unwrap_or("");
        let user = body["messages"][1]["content"].as_str().unwrap_or("");
        let content = if system.contains("index tool results") {
            let out = user.split("Output:\n").nth(1).unwrap_or(user);
            let facts: Vec<String> = out
                .lines()
                .filter(|l| l.contains(['=', ':']) && !l.starts_with("log line"))
                .take(14)
                .map(|l| l.trim().to_owned())
                .collect();
            json!({ "page": out.lines().next().unwrap_or(""), "facts": facts }).to_string()
        } else {
            let text = user.split("Content:\n").nth(1).unwrap_or(user);
            let lines: Vec<&str> = text.lines().map(strip_leaf_prefix).collect();
            let heading: String = lines.first().unwrap_or(&"").chars().take(110).collect();
            let summary: String = lines.join(" ").chars().take(600).collect();
            let entities: Vec<String> = text
                .split(|c: char| !c.is_alphanumeric())
                .filter(|w| w.len() > 2 && w.chars().next().is_some_and(char::is_uppercase))
                .map(str::to_owned)
                .collect::<HashSet<_>>()
                .into_iter()
                .take(10)
                .collect();
            json!({ "heading": heading, "summary": summary, "entities": entities }).to_string()
        };
        axum::Json(json!({
            "choices": [{ "message": { "content": content } }],
            "usage": { "prompt_tokens": user.len() / 4, "completion_tokens": 60, "cost": 0.0001 }
        }))
    }

    async fn mock_decide(axum::Json(body): axum::Json<Value>) -> axum::Json<Value> {
        let query = body["state"]["query"]["text"].as_str().unwrap_or("");
        let mut answers = serde_json::Map::new();
        if let Some(c) = body["state"]["candidates"].as_object() {
            for (id, card) in c {
                answers.insert(
                    id.clone(),
                    json!({ "type": "noul", "noul": mock_score(query, &card.to_string()) }),
                );
            }
        }
        axum::Json(json!({
            "answers": answers,
            "model": "mock-jev",
            "usage": { "input_tokens": body.to_string().len() / 4, "output_tokens": 0, "cost": 0.0001 }
        }))
    }

    async fn spawn_mock_relay() -> u16 {
        let app = axum::Router::new()
            .route("/v1/chat/completions", axum::routing::post(mock_cards))
            .route("/decisions", axum::routing::post(mock_decide));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        port
    }

    /// Load the fixture into a fresh store at fixed timestamps (`t` minutes ago), in time order
    /// so the ULIDs sort like the clock.
    fn load_fixture(store: &Store, fixture: &Value, now: i64) {
        let mut rows: Vec<(i64, String, Option<String>, String, String, String)> = Vec::new();
        for chat in fixture["chats"].as_array().unwrap() {
            let id = chat["id"].as_str().unwrap();
            store
                .upsert_chat(
                    id,
                    chat["title"].as_str().unwrap(),
                    false,
                    now - 10 * 86_400_000,
                )
                .unwrap();
            for l in chat["leaves"].as_array().unwrap() {
                rows.push((
                    now - l["t"].as_i64().unwrap() * 60_000,
                    id.to_owned(),
                    None,
                    l["kind"].as_str().unwrap().to_owned(),
                    l["role"].as_str().unwrap().to_owned(),
                    l["text"].as_str().unwrap().to_owned(),
                ));
            }
        }
        for sub in fixture["subagents"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
        {
            let id = sub["id"].as_str().unwrap();
            store
                .insert_subagent(
                    id,
                    sub["name"].as_str().unwrap(),
                    sub["purpose"].as_str().unwrap(),
                    "prime",
                    sub["chat_id"].as_str().unwrap(),
                )
                .unwrap();
            for l in sub["leaves"].as_array().unwrap() {
                rows.push((
                    now - l["t"].as_i64().unwrap() * 60_000,
                    id.to_owned(),
                    Some(id.to_owned()),
                    l["kind"].as_str().unwrap().to_owned(),
                    l["role"].as_str().unwrap().to_owned(),
                    l["text"].as_str().unwrap().to_owned(),
                ));
            }
        }
        rows.sort_by_key(|r| r.0);
        store
            .with(|c| {
                for (i, (at, thread, sub, kind, role, text)) in rows.iter().enumerate() {
                    // ids sort like the clock, as they do when rows arrive one turn at a time
                    let id = ulid::Ulid::from_parts(*at as u64, i as u128).to_string();
                    c.execute(
                        "INSERT INTO t3(id, thread_id, subagent_id, session_id, kind, role, at, text, source, size) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,NULL,?9)",
                        params![id, thread, sub, thread, kind, role, at, text, text.len() as i64],
                    )?;
                    c.execute(
                        "INSERT INTO t3_fts(text, id, thread_id, kind) VALUES (?1,?2,?3,?4)",
                        params![text, id, thread, kind],
                    )?;
                }
                Ok(())
            })
            .unwrap();
    }

    fn id_set(p: &Pack) -> std::collections::BTreeSet<String> {
        p.leaf_ids.iter().cloned().collect()
    }

    #[tokio::test]
    async fn fast_walk_keeps_every_cross_chat_hit() {
        let port = spawn_mock_relay().await;
        let dir = std::env::temp_dir().join(format!("arbos-walk-recall-{}", ulid()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(&dir.join("agentd.db")).unwrap();
        let fixture: Value =
            serde_json::from_str(include_str!("../tests/fixtures/memory-walk-recall.json"))
                .unwrap();
        load_fixture(&store, &fixture, now_ms());
        let llm = Llm::with_endpoints(
            &format!("http://127.0.0.1:{port}/v1"),
            &format!("http://127.0.0.1:{port}/decisions"),
            "test-token",
        );
        let tree = Tree::with_ids("p-test", "Bob", "mock-card-model", store, Arc::new(llm));
        for _ in 0..10 {
            let st = tree.build_pass().await.unwrap();
            if st["cards"].as_i64().unwrap_or(0) == 0 && st["leaves"].as_i64().unwrap_or(0) == 0 {
                break;
            }
        }
        let ready = tree.nodes_where("level = 'episode' AND status = 'ready'", &[]);
        assert!(ready.len() >= 6, "episode cards written: {}", ready.len());
        assert!(
            !tree
                .nodes_where("level = 'thread' AND heading <> ''", &[])
                .is_empty(),
            "thread cards written"
        );
        assert!(
            tree.store
                .with(
                    |c| c.query_row("SELECT COUNT(*) FROM tree_cards_fts", [], |r| r
                        .get::<_, i64>(0))
                )
                .unwrap()
                > 0,
            "card index populated"
        );

        let slow = WalkOptions {
            compact: true,
            fast: false,
        };
        let fast = WalkOptions {
            compact: true,
            fast: true,
        };
        let (mut calls_slow, mut calls_fast, mut calls_fast_again) = (0i64, 0i64, 0i64);
        for case in fixture["cases"].as_array().unwrap() {
            let id = case["id"].as_str().unwrap();
            let from = case["from"].as_str().unwrap();
            let q = case["query"].as_str().unwrap();
            let a = tree
                .walk_inner("mock-jev", from, false, q, "the project owner", "", slow)
                .await;
            let b = tree
                .walk_inner("mock-jev", from, false, q, "the project owner", "", fast)
                .await;
            let c = tree
                .walk_inner("mock-jev", from, false, q, "the project owner", "", fast)
                .await;
            calls_slow += a.jev_calls;
            calls_fast += b.jev_calls;
            calls_fast_again += c.jev_calls;
            eprintln!(
                "{id:>10}: D77 {} leaves / {} requests · fast {} leaves / {} requests ({}, flat={}, ride-along={}) · cached {} requests / {} hits",
                a.leaf_ids.len(),
                a.jev_calls,
                b.leaf_ids.len(),
                b.jev_calls,
                b.prefilter,
                b.flat,
                b.prefiltered,
                c.jev_calls,
                c.cache_hits
            );
            assert_eq!(
                id_set(&a),
                id_set(&b),
                "{id}: fast walk packs the same leaves as the D77 walk ({} / {})",
                a.prefilter,
                b.prefilter
            );
            assert_eq!(
                id_set(&b),
                id_set(&c),
                "{id}: a cached walk packs the same leaves"
            );
            assert_eq!(
                c.jev_calls, 0,
                "{id}: the second fast walk is answered from the cache"
            );
            for gold in case["gold"].as_array().unwrap() {
                let g = gold.as_str().unwrap();
                assert!(a.text.contains(g), "{id}: D77 walk packs {g:?}\n{}", a.text);
                assert!(
                    b.text.contains(g),
                    "{id}: fast walk packs {g:?}\n{}",
                    b.text
                );
            }
            if case["expect_empty"].as_bool().unwrap_or(false) {
                assert_eq!(a.outcome, "empty", "{id}: D77 walk injects nothing");
                assert_eq!(b.outcome, "empty", "{id}: fast walk injects nothing");
                assert_eq!(b.jev_calls, 0, "{id}: no Jev request when no chat matches");
                assert_eq!(b.prefilter, "evidence");
            }
            if id == "pet-breed" {
                assert_eq!(b.prefilter, "first-person");
            }
        }
        assert!(
            calls_fast < calls_slow,
            "fewer Jev requests: {calls_fast} vs {calls_slow}"
        );
        assert_eq!(calls_fast_again, 0);
        eprintln!("jev requests over the set: D77 walk {calls_slow}, fast walk {calls_fast}, fast walk again {calls_fast_again}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Build a tree over a fixture with the mock relay; returns the tree and its temp dir.
    async fn build_fixture_tree(path_json: &str) -> (Tree, std::path::PathBuf, Value) {
        let port = spawn_mock_relay().await;
        let dir = std::env::temp_dir().join(format!("arbos-walk-{}", ulid()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(&dir.join("agentd.db")).unwrap();
        let fixture: Value = serde_json::from_str(path_json).unwrap();
        load_fixture(&store, &fixture, now_ms());
        let llm = Llm::with_endpoints(
            &format!("http://127.0.0.1:{port}/v1"),
            &format!("http://127.0.0.1:{port}/decisions"),
            "test-token",
        );
        let tree = Tree::with_ids("p-test", "Bob", "mock-card-model", store, Arc::new(llm));
        for _ in 0..10 {
            let st = tree.build_pass().await.unwrap();
            if st["cards"].as_i64().unwrap_or(0) == 0 && st["leaves"].as_i64().unwrap_or(0) == 0 {
                break;
            }
        }
        (tree, dir, fixture)
    }

    fn leaf_id_containing(tree: &Tree, needle: &str) -> String {
        tree.t3_rows(
            "WHERE text LIKE ?1 ORDER BY id LIMIT 1",
            &[&format!("%{needle}%")],
        )
        .into_iter()
        .next()
        .map(|r| r.id)
        .unwrap_or_else(|| panic!("no leaf containing {needle:?}"))
    }

    /// D91: after a session compaction the asking chat's own early turns are walk candidates;
    /// what the window still holds is not. Both walk shapes agree, and the gold facts come back.
    #[tokio::test]
    async fn long_chat_recall_after_compaction() {
        let (tree, dir, fixture) =
            build_fixture_tree(include_str!("../tests/fixtures/memory-walk-long-chat.json")).await;
        let slow = WalkOptions {
            compact: true,
            fast: false,
        };
        let fast = WalkOptions {
            compact: true,
            fast: true,
        };
        let chat = "chat-launch";
        assert!(
            tree.nodes_where(
                "level = 'episode' AND status = 'ready' AND thread_id = 'chat-launch'",
                &[]
            )
            .len()
                >= 4,
            "the long chat split into carded episodes"
        );

        // Before any compaction the chat's own turns are the model's and are not walked.
        let before = tree
            .walk_inner(
                "mock-jev",
                chat,
                false,
                "What's my daughter's name?",
                "the project owner",
                "",
                fast,
            )
            .await;
        assert!(
            !before.text.contains("Mira"),
            "uncompacted chat: own turns are not packed\n{}",
            before.text
        );

        // A whole chat that fits in the kept tail records no cut.
        assert_eq!(tree.note_session_cut(chat, 1_000_000), None);
        assert_eq!(tree.session_cut(chat), None);

        // Compaction keeps the recent tail (the ledger-sync turns); everything older left the window.
        let cut = tree.note_session_cut(chat, 300).expect("a cut");
        assert_eq!(tree.session_cut(chat).as_deref(), Some(cut.as_str()));
        let pricing = leaf_id_containing(&tree, "$15 per seat");
        let ledger = leaf_id_containing(&tree, "ledger-sync");
        assert!(pricing < cut, "pricing turn is older than the cut");
        assert!(ledger >= cut, "ledger-sync turn is still in the window");

        let (mut calls_slow, mut calls_fast) = (0i64, 0i64);
        for case in fixture["cases"].as_array().unwrap() {
            let id = case["id"].as_str().unwrap();
            let q = case["query"].as_str().unwrap();
            let a = tree
                .walk_inner("mock-jev", chat, false, q, "the project owner", "", slow)
                .await;
            let b = tree
                .walk_inner("mock-jev", chat, false, q, "the project owner", "", fast)
                .await;
            calls_slow += a.jev_calls;
            calls_fast += b.jev_calls;
            eprintln!(
                "{id:>10}: D77 {} leaves / {} requests · fast {} leaves / {} requests ({}, flat={})",
                a.leaf_ids.len(), a.jev_calls, b.leaf_ids.len(), b.jev_calls, b.prefilter, b.flat
            );
            assert_eq!(
                id_set(&a),
                id_set(&b),
                "{id}: both walk shapes pack the same leaves"
            );
            for gold in case["gold"].as_array().unwrap() {
                let g = gold.as_str().unwrap();
                assert!(a.text.contains(g), "{id}: D77 walk packs {g:?}\n{}", a.text);
                assert!(
                    b.text.contains(g),
                    "{id}: fast walk packs {g:?}\n{}",
                    b.text
                );
            }
            for held in case["still_in_window"]
                .as_array()
                .cloned()
                .unwrap_or_default()
            {
                let h = held.as_str().unwrap();
                assert!(
                    !b.text.contains(h),
                    "{id}: {h:?} is still in the window and must not be packed\n{}",
                    b.text
                );
            }
            // nothing newer than the cut from the asking chat
            for l in &b.leaf_ids {
                let row = tree.store.t3_get(l).unwrap().unwrap();
                assert!(
                    row.thread_id != chat || l.as_str() < cut.as_str(),
                    "{id}: leaf {l} is still in the window"
                );
            }
        }
        assert!(
            calls_fast <= calls_slow,
            "fast walk needs no more requests: {calls_fast} vs {calls_slow}"
        );
        eprintln!("long chat: D77 walk {calls_slow} requests, fast walk {calls_fast}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
