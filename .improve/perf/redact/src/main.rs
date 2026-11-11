use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::hint::black_box;
use std::time::Instant;

use redact_perf::{find_key_like, redact_patterns, redact_placeholders, scrub, scrub_value};
use serde_json::{json, Value};

fn corpus() -> Vec<String> {
    let fill = "aB1cD2eF3gH4iJ5kL6mN7oP8qR9sT0uV";
    let samples = [
        format!("sk_{}_{fill}", "live"),
        format!("rk_{}_{fill}", "test"),
        format!("sk-or-v1-{fill}"),
        format!("sk-ant-{fill}"),
        format!("sk-proj-{fill}"),
        format!("sk-{fill}"),
        format!("ghp_{fill}"),
        format!("github_pat_{fill}"),
        format!("xoxb-{fill}"),
        format!("AKIA{}", "A1".repeat(8)),
        format!("ops_{fill}"),
        format!("dp.st.{fill}"),
        format!("dtn_{fill}"),
        format!("AIza{fill}"),
        format!("SG.{fill}.{fill}"),
        format!("key-{}", "a1".repeat(16)),
        format!("eyJ{fill}.{fill}.{fill}"),
    ];
    let mut cases = vec![
        "plain prose and Unicode λ: key-value, key-binding, primary-key-lookup".to_owned(),
        "2026-11-09 INFO relay GET /projects/synthetic/inspect 200 in 14ms".to_owned(),
        "see https://example.invalid/query?after=a8f3k2j9d0s7f6g5h4j3k2l1z0x9c8v7".to_owned(),
        "path /Users/synthetic/Projects/arbos/crates/arbos-relay/src/secrets.rs".to_owned(),
        "op://Synthetic/item/field arbos-secret://synthetic $ARBOS_SECRET_EXAMPLE".to_owned(),
        "export SYNTHETIC_API_KEY=not-a-real-value\nNAME=ordinary".to_owned(),
        "Authorization: Bearer synthetic.bearer-value\nordinary body".to_owned(),
    ];
    for sample in samples {
        for (before, after) in [
            ("", ""),
            ("λ token: ", "."),
            ("prefix_", "_suffix"),
            ("/", "/path"),
            ("\"", "\""),
            ("text\n", "\nend"),
        ] {
            cases.push(format!("{before}{sample}{after}"));
        }
    }
    cases
}

fn value(text: &str) -> Value {
    json!({ "unchanged_key": [text, { "nested": text }], "count": 7, "enabled": true, "nothing": null })
}

fn fingerprint(cases: &[String]) -> u64 {
    let mut hash = DefaultHasher::new();
    for text in cases {
        redact_patterns(text).hash(&mut hash);
        redact_placeholders(text).hash(&mut hash);
        scrub(text).hash(&mut hash);
        for found in find_key_like(text) {
            (
                found.start,
                found.end,
                found.suggested_name,
                found.confident,
            )
                .hash(&mut hash);
        }
        let mut v = value(text);
        scrub_value(&mut v);
        v.to_string().hash(&mut hash);
    }
    hash.finish()
}

fn run(workload: &str, iterations: usize, cases: &[String], values: &[Value]) -> u128 {
    let start = Instant::now();
    for i in 0..iterations {
        let index = i % cases.len();
        let text = black_box(cases[index].as_str());
        match workload {
            "redact_patterns" => {
                black_box(redact_patterns(text));
            }
            "find_key_like" => {
                black_box(find_key_like(text));
            }
            "redact_placeholders" => {
                black_box(redact_placeholders(text));
            }
            "scrub_value" => {
                let mut v = black_box(values[index].clone());
                scrub_value(&mut v);
                black_box(v);
            }
            _ => panic!("unknown workload"),
        }
    }
    start.elapsed().as_nanos()
}

fn main() {
    let mut args = std::env::args().skip(1);
    let iterations: usize = args
        .next()
        .unwrap_or_else(|| "2000".into())
        .parse()
        .unwrap();
    let rounds: usize = args.next().unwrap_or_else(|| "15".into()).parse().unwrap();
    assert!(iterations > 0 && rounds > 0);
    let cases = corpus();
    let values: Vec<Value> = cases.iter().map(|s| value(s)).collect();
    let cold = Instant::now();
    black_box(scrub(&cases[0]));
    black_box(find_key_like(&cases[0]));
    let first_scrub_and_classify_ns = cold.elapsed().as_nanos();
    let digest = fingerprint(&cases);
    let workloads = [
        "redact_patterns",
        "find_key_like",
        "redact_placeholders",
        "scrub_value",
    ];
    for workload in workloads {
        black_box(run(workload, cases.len(), &cases, &values));
    }
    let mut samples = Vec::new();
    for round in 0..rounds {
        for offset in 0..workloads.len() {
            let workload = workloads[(round + offset) % workloads.len()];
            samples.push(
                json!({ "round": round, "workload": workload, "iterations": iterations,
                "total_ns": run(workload, iterations, &cases, &values) }),
            );
        }
    }
    println!(
        "{}",
        json!({ "corpus_cases": cases.len(), "fingerprint": format!("{digest:016x}"),
        "first_scrub_and_classify_ns": first_scrub_and_classify_ns, "samples": samples })
    );
}
