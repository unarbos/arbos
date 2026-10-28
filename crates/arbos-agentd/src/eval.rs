//! `arbos-agentd eval-responders`: replay a labelled group-chat fixture through the respond
//! judge (`responders.rs`) and report exact-set accuracy, per-agent precision/recall, how many
//! silence cases stayed silent, how many "exactly one agent" cases got exactly one, a threshold
//! sweep over the recorded probabilities (no extra calls), latency percentiles, tokens and cost.
//!
//! Fixture: `{ rosters: { name: { kind, title, people[], agents[{id, name, role}] } },
//! cases: [{ id, roster, from, text, mentions[], recent[{from, text}], reply_to?, expected[]
//! | expected_one_of[], note }] }`. `expected` is the exact set of agent ids (empty = silence);
//! `expected_one_of` means exactly one of those ids. The real-Arbonites set lives at
//! `crates/arbos-agentd/tests/fixtures/respond-gate-arbonites.json`.
//!
//! Talks to OpenRouter directly when `OPENROUTER_API_KEY` is set, else to the relay proxy
//! (`ARBOS_RELAY_URL`, `ARBOS_BOX_TOKEN`).

use std::path::Path;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};

use crate::config::{DEFAULT_DECISION_SCORER, DEFAULT_DECISION_THRESHOLD};
use crate::llm::Llm;
use crate::people::{mention_role, MentionRole};
use crate::responders::{self, Candidate, ResponderDecision, Room};
use crate::settings::{DecisionConfig, DecisionProvider};

pub struct EvalArgs {
    pub fixture: String,
    pub scorer: Option<String>,
    pub threshold: Option<f64>,
    pub repeat: usize,
    pub out: Option<String>,
}

/// What one case expects.
enum Expected {
    /// Exactly this set (empty = nobody replies).
    Set(Vec<String>),
    /// Exactly one agent, from this list.
    OneOf(Vec<String>),
}

struct Case {
    id: String,
    note: String,
    room: Room,
    expected: Expected,
}

struct Run {
    case_idx: usize,
    decision: Option<ResponderDecision>,
    error: Option<String>,
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn load_cases(path: &Path) -> Result<Vec<Case>> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let v: Value = serde_json::from_str(&text)?;
    let rosters = v
        .get("rosters")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("fixture has no rosters"))?;
    let mut out = Vec::new();
    for c in v
        .get("cases")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("fixture has no cases"))?
    {
        let roster_name = c["roster"].as_str().unwrap_or("");
        let roster = rosters
            .get(roster_name)
            .ok_or_else(|| anyhow!("case {} names unknown roster {roster_name}", c["id"]))?;
        let text = c["text"].as_str().unwrap_or("");
        let chips = strings(&c["mentions"]);
        let mut candidates: Vec<Candidate> = roster["agents"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|x| {
                        let id = x["id"].as_str().unwrap_or("").to_owned();
                        let name = x["name"].as_str().unwrap_or("").to_owned();
                        // Same classifier the gate uses: how `text` @mentions this agent; a
                        // typed chip with no `@handle` in the prose reads as `referenced`.
                        let mut role = mention_role(text, &name);
                        if role == MentionRole::None
                            && chips.iter().any(|m| *m == format!("agent:{id}"))
                        {
                            role = MentionRole::Referenced;
                        }
                        Candidate {
                            id,
                            name,
                            role: x["role"].as_str().unwrap_or("").to_owned(),
                            host: x["host"].as_bool().unwrap_or(false),
                            mentioned_as: (role != MentionRole::None).then(|| role.as_str()),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        candidates.sort_by(|a, b| a.id.cmp(&b.id));
        let kind: &'static str = if roster["kind"].as_str() == Some("people_group") {
            "people_group"
        } else {
            "project_group"
        };
        let expected = if c.get("expected_one_of").is_some() {
            Expected::OneOf(strings(&c["expected_one_of"]))
        } else {
            Expected::Set(strings(&c["expected"]))
        };
        out.push(Case {
            id: c["id"].as_str().unwrap_or("?").to_owned(),
            note: c["note"].as_str().unwrap_or("").to_owned(),
            room: Room {
                kind,
                chat_id: format!("eval-{roster_name}"),
                title: roster["title"].as_str().unwrap_or("group").to_owned(),
                from: c["from"].as_str().unwrap_or("the user").to_owned(),
                text: text.to_owned(),
                recent: c["recent"].as_array().cloned().unwrap_or_default(),
                candidates,
                people: strings(&roster["people"]),
                reply_to: c
                    .get("reply_to")
                    .filter(|r| r.is_object())
                    .map(|r| json!({ "from": r["from"], "text": r["text"] })),
            },
            expected,
        });
    }
    Ok(out)
}

fn percentile(sorted: &[i64], p: f64) -> i64 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn same_set(a: &[String], b: &[String]) -> bool {
    a.len() == b.len() && a.iter().all(|x| b.contains(x))
}

/// Accuracy numbers for the runs at one threshold (re-picked from the recorded probabilities
/// when `threshold` is given).
fn summarize(cases: &[Case], runs: &[Run], threshold: Option<f64>) -> Value {
    let mut exact = 0usize;
    let mut total = 0usize;
    let (mut tp, mut fp, mut fn_) = (0usize, 0usize, 0usize);
    let (mut silence_right, mut silence_total) = (0usize, 0usize);
    let (mut one_right, mut one_total) = (0usize, 0usize);
    let mut latencies: Vec<i64> = Vec::new();
    let mut tokens_in = 0i64;
    let mut tokens_out = 0i64;
    let mut cost = 0.0f64;
    let mut costed = 0usize;
    let mut failures = 0usize;
    let mut wrong: Vec<Value> = Vec::new();
    for r in runs {
        let c = &cases[r.case_idx];
        total += 1;
        let Some(d) = &r.decision else {
            failures += 1;
            match &c.expected {
                Expected::Set(e) => fn_ += e.len(),
                Expected::OneOf(_) => fn_ += 1,
            }
            continue;
        };
        latencies.push(d.latency_ms);
        tokens_in += d.input_tokens;
        tokens_out += d.output_tokens;
        if let Some(u) = d.cost_usd {
            cost += u;
            costed += 1;
        }
        let chosen = match threshold {
            Some(t) => responders::pick(&d.addressee_probs, &d.scores, t).3,
            None => d.chosen.clone(),
        };
        let ok = match &c.expected {
            Expected::Set(e) => {
                if e.is_empty() {
                    silence_total += 1;
                    if chosen.is_empty() {
                        silence_right += 1;
                    }
                }
                for a in &c.room.candidates {
                    match (e.contains(&a.id), chosen.contains(&a.id)) {
                        (true, true) => tp += 1,
                        (false, true) => fp += 1,
                        (true, false) => fn_ += 1,
                        (false, false) => {}
                    }
                }
                same_set(&chosen, e)
            }
            Expected::OneOf(ids) => {
                one_total += 1;
                let hits = chosen.iter().filter(|id| ids.contains(id)).count();
                let ok = chosen.len() == 1 && hits == 1;
                if ok {
                    one_right += 1;
                }
                tp += hits.min(1);
                fn_ += usize::from(hits == 0);
                fp += chosen.len() - hits.min(1);
                ok
            }
        };
        if ok {
            exact += 1;
        } else {
            wrong.push(json!({
                "case": c.id, "note": c.note,
                "expected": match &c.expected { Expected::Set(e) => json!(e), Expected::OneOf(e) => json!({ "one_of": e }) },
                "got": chosen, "addressee": d.addressee, "addressee_p": (d.addressee_p * 100.0).round() / 100.0,
                "scores": d.scores.iter().map(|(id, p)| json!({ "id": id, "p": (p * 100.0).round() / 100.0 })).collect::<Vec<_>>(),
            }));
        }
    }
    latencies.sort_unstable();
    let precision = if tp + fp > 0 {
        tp as f64 / (tp + fp) as f64
    } else {
        1.0
    };
    let recall = if tp + fn_ > 0 {
        tp as f64 / (tp + fn_) as f64
    } else {
        1.0
    };
    let f1 = if precision + recall > 0.0 {
        2.0 * precision * recall / (precision + recall)
    } else {
        0.0
    };
    let n = latencies.len().max(1) as f64;
    json!({
        "threshold": threshold,
        "runs": total,
        "failures": failures,
        "exact_set_accuracy": exact as f64 / total.max(1) as f64,
        "exact": exact,
        "per_agent": { "precision": precision, "recall": recall, "f1": f1, "tp": tp, "fp": fp, "fn": fn_ },
        "silence_cases": { "right": silence_right, "total": silence_total },
        "exactly_one_cases": { "right": one_right, "total": one_total },
        "latency_ms": {
            "p50": percentile(&latencies, 0.5), "p95": percentile(&latencies, 0.95),
            "max": latencies.last().copied().unwrap_or(0),
            "mean": latencies.iter().sum::<i64>() as f64 / n,
        },
        "mean_input_tokens": tokens_in as f64 / n,
        "mean_output_tokens": tokens_out as f64 / n,
        "mean_cost_usd": if costed > 0 { Some(cost / costed as f64) } else { None },
        "wrong": wrong,
    })
}

async fn run_cases(llm: &Llm, cfg: &DecisionConfig, cases: &[Case], repeat: usize) -> Vec<Run> {
    let mut runs = Vec::new();
    for _ in 0..repeat {
        for (i, c) in cases.iter().enumerate() {
            match responders::decide(llm, cfg, &c.room).await {
                Ok(d) => {
                    let want = match &c.expected {
                        Expected::Set(e) => format!("{e:?}"),
                        Expected::OneOf(e) => format!("one of {e:?}"),
                    };
                    let ok = match &c.expected {
                        Expected::Set(e) => same_set(&d.chosen, e),
                        Expected::OneOf(e) => d.chosen.len() == 1 && e.contains(&d.chosen[0]),
                    };
                    eprintln!(
                        "{} {:<7} {:>5} ms  {}  want {want}",
                        if ok { "ok  " } else { "MISS" },
                        c.id,
                        d.latency_ms,
                        d.reason,
                    );
                    runs.push(Run {
                        case_idx: i,
                        decision: Some(d),
                        error: None,
                    });
                }
                Err(e) => {
                    eprintln!("FAIL {:<7} {e}", c.id);
                    runs.push(Run {
                        case_idx: i,
                        decision: None,
                        error: Some(e.to_string()),
                    });
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    runs
}

pub async fn run(args: EvalArgs) -> Result<()> {
    let cases = load_cases(Path::new(&args.fixture))?;
    let llm = match std::env::var("OPENROUTER_API_KEY")
        .ok()
        .filter(|s| !s.is_empty())
    {
        Some(key) => {
            eprintln!("endpoint: OpenRouter directly");
            Llm::with_endpoints(
                "https://openrouter.ai/api/v1",
                "https://openrouter.ai/api/alpha/decisions",
                &key,
            )
        }
        None => {
            let relay = std::env::var("ARBOS_RELAY_URL")
                .ok()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    anyhow!("set OPENROUTER_API_KEY, or ARBOS_RELAY_URL + ARBOS_BOX_TOKEN")
                })?;
            let token = std::env::var("ARBOS_BOX_TOKEN").unwrap_or_default();
            eprintln!("endpoint: relay proxy at {relay}");
            let relay = relay.trim_end_matches('/');
            Llm::with_endpoints(
                &format!("{relay}/proxy/v1"),
                &format!("{relay}/proxy/decisions"),
                &token,
            )
        }
    };
    let cfg = DecisionConfig {
        provider: DecisionProvider::Jev,
        scorer: args
            .scorer
            .clone()
            .unwrap_or_else(|| DEFAULT_DECISION_SCORER.to_owned()),
        threshold: args.threshold.unwrap_or(DEFAULT_DECISION_THRESHOLD),
        memory_tree: crate::tree::TreeMode::Off,
    };
    let runs = run_cases(&llm, &cfg, &cases, args.repeat).await;
    let mut report = summarize(&cases, &runs, None);
    report["fixture"] = json!(args.fixture);
    report["cases"] = json!(cases.len());
    report["repeat"] = json!(args.repeat);
    report["scorer"] = json!(cfg.scorer);
    report["model_builds"] = json!(runs
        .iter()
        .filter_map(|r| r.decision.as_ref().map(|d| d.model.clone()))
        .collect::<std::collections::BTreeSet<_>>());
    report["errors"] = json!(runs
        .iter()
        .filter_map(|r| r.error.clone())
        .collect::<Vec<_>>());
    // Threshold sweep over the recorded probabilities: no new calls needed.
    report["threshold_sweep"] = json!([0.3, 0.35, 0.4, 0.45, 0.5, 0.55, 0.6, 0.65, 0.7, 0.75, 0.8, 0.85, 0.9]
        .iter()
        .map(|t| {
            let s = summarize(&cases, &runs, Some(*t));
            json!({ "threshold": t, "exact_set_accuracy": s["exact_set_accuracy"], "precision": s["per_agent"]["precision"], "recall": s["per_agent"]["recall"], "f1": s["per_agent"]["f1"], "silence": s["silence_cases"], "exactly_one": s["exactly_one_cases"] })
        })
        .collect::<Vec<_>>());
    // Per-case verdicts, so an external scorer can compare runs.
    report["runs"] = json!(runs
        .iter()
        .map(|r| {
            let c = &cases[r.case_idx];
            match &r.decision {
                Some(d) => json!({
                    "id": c.id, "chosen": d.chosen, "addressee": d.addressee,
                    "addressee_probs": d.addressee_probs.iter().map(|(k, p)| json!({ "option": k, "p": (p * 1000.0).round() / 1000.0 })).collect::<Vec<_>>(),
                    "scores": d.scores.iter().map(|(k, p)| json!({ "id": k, "p": (p * 1000.0).round() / 1000.0 })).collect::<Vec<_>>(),
                    "reason": d.reason, "latency_ms": d.latency_ms, "cost_usd": d.cost_usd,
                    "input_tokens": d.input_tokens, "output_tokens": d.output_tokens, "input_hash": d.input_hash,
                }),
                None => json!({ "id": c.id, "chosen": [], "error": r.error }),
            }
        })
        .collect::<Vec<_>>());
    eprintln!(
        "judge: exact-set {:.1}% ({}/{}), per-agent P {:.2} R {:.2} F1 {:.2}, silence {}/{}, exactly-one {}/{}, p50 {} ms, p95 {} ms, mean tokens in {:.0} / out {:.0}, mean cost {}",
        report["exact_set_accuracy"].as_f64().unwrap_or(0.0) * 100.0,
        report["exact"],
        report["runs"].as_array().map(|a| a.len()).unwrap_or(0),
        report["per_agent"]["precision"].as_f64().unwrap_or(0.0),
        report["per_agent"]["recall"].as_f64().unwrap_or(0.0),
        report["per_agent"]["f1"].as_f64().unwrap_or(0.0),
        report["silence_cases"]["right"],
        report["silence_cases"]["total"],
        report["exactly_one_cases"]["right"],
        report["exactly_one_cases"]["total"],
        report["latency_ms"]["p50"],
        report["latency_ms"]["p95"],
        report["mean_input_tokens"].as_f64().unwrap_or(0.0),
        report["mean_output_tokens"].as_f64().unwrap_or(0.0),
        report["mean_cost_usd"],
    );
    let text = serde_json::to_string_pretty(&report)?;
    match &args.out {
        Some(path) => {
            std::fs::write(path, &text)?;
            eprintln!("wrote {path}");
        }
        None => println!("{text}"),
    }
    Ok(())
}
