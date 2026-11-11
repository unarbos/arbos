//! Bounded local observations of actual billed-settlement helpers; synthetic in-memory state only.
extern crate self as arbos_core;
mod generated;
use db::Db;
pub use generated::{credits, db, db_accounts, db_voice, ids, proxy};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    existing: bool,
    metered: bool,
    grant_micros: u32,
    cap_micros: Option<u32>,
    allow_negative: bool,
}
#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Event {
    Report {
        total_micros: u32,
        vision_micros: u32,
    },
    Fault {
        kind: u8,
    },
    Clear,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    id: u32,
    config: Config,
    events: Vec<Event>,
}
fn dollars(micros: u32) -> f64 {
    f64::from(micros) / 1_000_000.0
}
fn snapshot(db: &Db) -> anyhow::Result<Value> {
    db.with(|c| {
        let account = c.query_row("SELECT spend_cap_usd,cap_unlimited,allow_negative FROM users WHERE id='synthetic-payer'", [], |r| Ok(json!({"cap":r.get::<_,f64>(0)?,"unlimited":r.get::<_,i64>(1)?!=0,"allow_negative":r.get::<_,i64>(2)?!=0}))).optional()?;
        let mut statement=c.prepare("SELECT kind,usd,ref FROM credit_ledger WHERE user_id='synthetic-payer' ORDER BY id")?;
        let ledger=statement.query_map([],|r|Ok(json!({"kind":r.get::<_,String>(0)?,"usd":r.get::<_,f64>(1)?,"reference":r.get::<_,Option<String>>(2)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
        let usage=c.query_row("SELECT usd,vision_usd,usd_charged,vision_charged FROM voice_calls WHERE call_id='synthetic-call' AND project_id='synthetic-project'",[],|r|Ok(json!({"total":r.get::<_,f64>(0)?,"vision":r.get::<_,f64>(1)?,"settled":r.get::<_,f64>(2)?,"vision_settled":r.get::<_,f64>(3)?}))).optional()?;
        Ok(json!({"account":account,"ledger":ledger,"usage":usage}))
    })
}
fn fault(db: &Db, kind: u8) -> anyhow::Result<()> {
    db.with(|c| {
        c.execute_batch("DROP TRIGGER IF EXISTS synthetic_fault")?;
        let statement=match kind {
            0=>"CREATE TRIGGER synthetic_fault BEFORE UPDATE OF usd_charged ON voice_calls BEGIN SELECT RAISE(ABORT,'synthetic fault'); END",
            1=>"CREATE TRIGGER synthetic_fault BEFORE UPDATE OF usd_charged ON voice_calls BEGIN SELECT RAISE(IGNORE); END",
            2=>"CREATE TRIGGER synthetic_fault BEFORE INSERT ON credit_ledger WHEN NEW.kind='voice' BEGIN SELECT RAISE(ABORT,'synthetic fault'); END",
            3=>"CREATE TRIGGER synthetic_fault BEFORE INSERT ON credit_ledger WHEN NEW.kind='voice' BEGIN SELECT RAISE(IGNORE); END",
            4=>"CREATE TRIGGER synthetic_fault BEFORE INSERT ON users BEGIN SELECT RAISE(IGNORE); END",
            5=>"CREATE TRIGGER synthetic_fault BEFORE UPDATE OF cap_unlimited,spend_cap_usd ON users BEGIN SELECT RAISE(IGNORE); END",
            _=>"CREATE TRIGGER synthetic_fault BEFORE INSERT ON credit_ledger WHEN NEW.kind='grant' BEGIN SELECT RAISE(IGNORE); END",
        };c.execute_batch(statement)
    })
}
fn observe(request: &Request) -> anyhow::Result<Value> {
    let db = Db::fresh()?;
    let config = &request.config;
    if config.existing {
        db.with(|c| {
            c.execute("INSERT INTO users(id,created_at,spend_cap_usd,cap_unlimited,allow_negative) VALUES ('synthetic-payer',0,?1,?2,?3)",params![config.cap_micros.map_or(0.0,dollars),i64::from(config.cap_micros.is_none()),i64::from(config.allow_negative)])?;
            if config.grant_micros>0 {c.execute("INSERT INTO credit_ledger(user_id,at,kind,usd,actor) VALUES ('synthetic-payer',0,'grant',?1,'synthetic-fixture')",params![dollars(config.grant_micros)])?;}Ok(())
        })?;
    }
    let billing = db_voice::VoiceBilling {
        metered: config.metered,
        legacy: db_accounts::LegacyCreditDefaults {
            limit: config
                .cap_micros
                .map_or(credits::SpendLimit::Unlimited, |v| {
                    credits::SpendLimit::Daily(dollars(v))
                }),
            grant_usd: dollars(config.grant_micros),
        },
    };
    let initial = snapshot(&db)?;
    let mut steps = Vec::new();
    for event in &request.events {
        let result = match event {
            Event::Clear => {
                db.with(|c| c.execute_batch("DROP TRIGGER IF EXISTS synthetic_fault"))?;
                json!({"tag":"Fixture"})
            }
            Event::Fault { kind } => {
                fault(&db, *kind)?;
                json!({"tag":"Fixture"})
            }
            Event::Report {
                total_micros,
                vision_micros,
            } => {
                let usage = db_voice::VoiceUsage {
                    usd: dollars(*total_micros),
                    vision_usd: dollars(*vision_micros),
                    ..Default::default()
                };
                match db.settle_voice_usage(
                    "synthetic-call",
                    "synthetic-project",
                    "synthetic-payer",
                    1,
                    &usage,
                    &billing,
                ) {
                    Err(_) => json!({"tag":"Error"}),
                    Ok(receipt) => {
                        let charges=receipt.charges.into_iter().map(|(part,charge)|{
                            let legacy=match charge.legacy{db_accounts::LegacyCreditInit::Existing=>"Existing",db_accounts::LegacyCreditInit::Granted=>"Granted",db_accounts::LegacyCreditInit::Unfunded=>"Unfunded"};
                            let debit=match charge.debit{db_accounts::PostfactDebit::Paid=>json!({"tag":"Paid"}),db_accounts::PostfactDebit::Clamped{balance_usd}=>json!({"tag":"Clamped","balance":balance_usd}),db_accounts::PostfactDebit::Refused(reason)=>json!({"tag":"Refused","reason":match reason{db_accounts::DebitRefused::Insufficient{..}=>"Insufficient",db_accounts::DebitRefused::DailyCap{..}=>"DailyCap",db_accounts::DebitRefused::ReadOnly{..}=>"ReadOnly"}})};
                            json!({"reported_part":part,"legacy":legacy,"debit":debit})
                        }).collect::<Vec<_>>();
                        json!({"tag":"Committed","reported_total":receipt.reported_total,"reported_delta":receipt.reported_delta,"charges":charges})
                    }
                }
            }
        };
        steps.push(json!({"result":result,"snapshot":snapshot(&db)?}));
    }
    Ok(
        json!({"id":request.id,"sqlite_version":rusqlite::version(),"initial":initial,"steps":steps}),
    )
}
fn reply(raw: &[u8]) -> Option<Value> {
    let request: Request = serde_json::from_slice(raw).ok()?;
    let config = &request.config;
    if request.events.len() > 64
        || config.grant_micros > 1_000_000_000
        || config.cap_micros.is_some_and(|v| v > 1_000_000_000)
        || (!config.existing && (config.allow_negative || config.cap_micros == Some(0)))
    {
        return None;
    }
    if request.events.iter().any(|event| match event {
        Event::Fault { kind } => *kind > 6,
        Event::Report {
            total_micros,
            vision_micros,
        } => *total_micros > 1_000_000_000 || vision_micros > total_micros,
        Event::Clear => false,
    }) {
        return None;
    }
    observe(&request).ok()
}
fn main() -> io::Result<()> {
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let mut line = Vec::new();
    loop {
        line.clear();
        let mut oversized = false;
        loop {
            let bytes = input.fill_buf()?;
            if bytes.is_empty() {
                break;
            }
            let used = bytes
                .iter()
                .position(|&byte| byte == b'\n')
                .map_or(bytes.len(), |index| index + 1);
            let ended = bytes[used - 1] == b'\n';
            if line.len() + used <= 8192 && !oversized {
                line.extend_from_slice(&bytes[..used]);
            } else {
                oversized = true;
            }
            input.consume(used);
            if ended {
                break;
            }
        }
        if line.is_empty() && !oversized {
            break;
        }
        let response = if oversized { None } else { reply(&line) }
            .unwrap_or_else(|| json!({"id":null,"error":"Invalid trace"}));
        writeln!(output, "{response}")?;
        output.flush()?;
    }
    Ok(())
}
