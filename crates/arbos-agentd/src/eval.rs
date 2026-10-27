//! `arbos-agentd eval-responders`: run the labelled group-chat fixture
//! (`crates/arbos-agentd/tests/fixtures/group-responders.json`) through the responder decision
//! and report accuracy, per-agent precision/recall, latency percentiles, tokens and cost for the
//! `jev` decision model and the `llm` router. Talks to OpenRouter directly when
//! `OPENROUTER_API_KEY` is set, else to the relay proxy (`ARBOS_RELAY_URL`, `ARBOS_BOX_TOKEN`).

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
    pub provider: String,
    pub scorer: Option<String>,
    pub llm_model: String,
    pub threshold: Option<f64>,
    pub repeat: usize,
    pub out: Option<String>,
}

struct Case {
    id: String,
    note: String,
    room: Room,
    expected: Vec<String>,
}

struct Run {
    case_idx: usize,
    decision: Option<ResponderDecision>,
    error: Option<String>,
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
        let candidates = roster["agents"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|x| {
                        let name = x["name"].as_str().unwrap_or("").to_owned();
                        // Same classifier the people gate uses: how `text` @mentions this agent.
                        let role = mention_role(text, &name);
                        Candidate {
                            id: x["id"].as_str().unwrap_or("").to_owned(),
                            name,
                            role: x["role"].as_str().unwrap_or("").to_owned(),
                            host: x["host"].as_bool().unwrap_or(false),
                            mentioned_as: (role != MentionRole::None).then(|| role.as_str()),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        let kind: &'static str = if roster["kind"].as_str() == Some("people_group") {
            "people_group"
        } else {
            "project_group"
        };
        out.push(Case {
            id: c["id"].as_str().unwrap_or("?").to_owned(),
            note: c["note"].as_str().unwrap_or("").to_owned(),
            room: Room {
                kind,
                chat_id: format!("eval-{roster_name}"),
                title: roster["title"].as_str().unwrap_or("group").to_owned(),
                from: c["from"].as_str().unwrap_or("the user").to_owned(),
                text: c["text"].as_str().unwrap_or("").to_owned(),
                recent: c["recent"].as_array().cloned().unwrap_or_default(),
                candidates,
                people: roster["people"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default(),
            },
            expected: c["expected"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default(),
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

/// Accuracy numbers for one provider's runs, at one threshold (scores re-thresholded for `jev`).
fn summarize(cases: &[Case], runs: &[Run], threshold: Option<f64>) -> Value {
    let mut exact = 0usize;
    let mut total = 0usize;
    let (mut tp, mut fp, mut fn_) = (0usize, 0usize, 0usize);
    let mut none_right = 0usize;
    let mut none_total = 0usize;
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
            fn_ += c.expected.len();
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
            Some(t) if !d.scores.is_empty() => responders::threshold_pick(&d.scores, t),
            _ => d.chosen.clone(),
        };
        if same_set(&chosen, &c.expected) {
            exact += 1;
        } else {
            wrong.push(json!({
                "case": c.id, "note": c.note, "expected": c.expected, "got": chosen,
                "scores": d.scores.iter().map(|(id, p)| json!({ "id": id, "p": (p * 100.0).round() / 100.0 })).collect::<Vec<_>>(),
            }));
        }
        if c.expected.is_empty() {
            none_total += 1;
            if chosen.is_empty() {
                none_right += 1;
            }
        }
        for a in &c.room.candidates {
            let want = c.expected.contains(&a.id);
            let got = chosen.contains(&a.id);
            match (want, got) {
                (true, true) => tp += 1,
                (false, true) => fp += 1,
                (true, false) => fn_ += 1,
                (false, false) => {}
            }
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
        "nobody_cases": { "right": none_right, "total": none_total },
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

async fn run_provider(
    llm: &Llm,
    provider: DecisionProvider,
    cfg: &DecisionConfig,
    llm_model: &str,
    cases: &[Case],
    repeat: usize,
) -> Vec<Run> {
    let mut runs = Vec::new();
    for _ in 0..repeat {
        for (i, c) in cases.iter().enumerate() {
            let r = match provider {
                DecisionProvider::Jev => responders::decide_jev(llm, cfg, &c.room).await,
                DecisionProvider::Llm => {
                    responders::decide_llm(llm, llm_model, cfg.threshold, &c.room).await
                }
            };
            match r {
                Ok(d) => {
                    let mark = if same_set(&d.chosen, &c.expected) {
                        "ok  "
                    } else {
                        "MISS"
                    };
                    eprintln!(
                        "{mark} {:<8} {:<4} {:>5} ms  got {:?} want {:?}  {}",
                        provider.as_str(),
                        c.id,
                        d.latency_ms,
                        d.chosen,
                        c.expected,
                        d.scores
                            .iter()
                            .map(|(id, p)| format!("{id}={p:.2}"))
                            .collect::<Vec<_>>()
                            .join(" ")
                    );
                    runs.push(Run {
                        case_idx: i,
                        decision: Some(d),
                        error: None,
                    });
                }
                Err(e) => {
                    eprintln!("FAIL {:<8} {:<4} {e}", provider.as_str(), c.id);
                    runs.push(Run {
                        case_idx: i,
                        decision: None,
                        error: Some(e.to_string()),
                    });
                }
            }
            // Be gentle with rate limits on the chat path.
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
    let providers: Vec<DecisionProvider> = match args.provider.as_str() {
        "jev" => vec![DecisionProvider::Jev],
        "llm" => vec![DecisionProvider::Llm],
        _ => vec![DecisionProvider::Jev, DecisionProvider::Llm],
    };
    let mut report = json!({
        "fixture": args.fixture,
        "cases": cases.len(),
        "repeat": args.repeat,
        "scorer": cfg.scorer,
        "llm_model": args.llm_model,
        "threshold": cfg.threshold,
        "providers": {},
    });
    for p in providers {
        eprintln!("--- provider {} ---", p.as_str());
        let runs = run_provider(&llm, p, &cfg, &args.llm_model, &cases, args.repeat).await;
        let mut entry = summarize(&cases, &runs, None);
        entry["model_builds"] = json!(runs
            .iter()
            .filter_map(|r| r.decision.as_ref().map(|d| d.model.clone()))
            .collect::<std::collections::BTreeSet<_>>());
        entry["errors"] = json!(runs
            .iter()
            .filter_map(|r| r.error.clone())
            .collect::<Vec<_>>());
        if p == DecisionProvider::Jev {
            // Threshold sweep over the recorded probabilities: no new calls needed.
            let sweep: Vec<Value> = [0.3, 0.35, 0.4, 0.45, 0.5, 0.55, 0.6, 0.65, 0.7, 0.75, 0.8]
                .iter()
                .map(|t| {
                    let s = summarize(&cases, &runs, Some(*t));
                    json!({ "threshold": t, "exact_set_accuracy": s["exact_set_accuracy"], "precision": s["per_agent"]["precision"], "recall": s["per_agent"]["recall"], "f1": s["per_agent"]["f1"] })
                })
                .collect();
            entry["threshold_sweep"] = json!(sweep);
        }
        let s = &entry;
        eprintln!(
            "{}: exact-set {:.1}% ({}/{}), per-agent P {:.2} R {:.2} F1 {:.2}, nobody {}/{}, p50 {} ms, p95 {} ms, mean tokens in {:.0} / out {:.0}, mean cost {}",
            p.as_str(),
            s["exact_set_accuracy"].as_f64().unwrap_or(0.0) * 100.0,
            s["exact"],
            s["runs"],
            s["per_agent"]["precision"].as_f64().unwrap_or(0.0),
            s["per_agent"]["recall"].as_f64().unwrap_or(0.0),
            s["per_agent"]["f1"].as_f64().unwrap_or(0.0),
            s["nobody_cases"]["right"],
            s["nobody_cases"]["total"],
            s["latency_ms"]["p50"],
            s["latency_ms"]["p95"],
            s["mean_input_tokens"].as_f64().unwrap_or(0.0),
            s["mean_output_tokens"].as_f64().unwrap_or(0.0),
            s["mean_cost_usd"],
        );
        report["providers"][p.as_str()] = entry;
    }
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
