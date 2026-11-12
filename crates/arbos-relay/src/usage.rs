//! Model request accounting. Extend the existing usage ledger rather than maintaining a second
//! session counter. Legacy rows retain unknown attribution and unknown cost provenance.

use anyhow::Result;
use arbos_core::usage::UsageContext;
use rusqlite::{params, Connection};
use serde::Serialize;

use crate::db::{Db, UsageTotals};

pub fn migrate(c: &Connection) -> Result<()> {
    let columns: Vec<String> = c
        .prepare("PRAGMA table_info(usage)")?
        .query_map([], |r| r.get(1))?
        .collect::<rusqlite::Result<_>>()?;
    for (name, ty) in [
        ("request_id", "TEXT"),
        ("session_id", "TEXT"),
        ("session_run_id", "TEXT"),
        ("parent_session_id", "TEXT"),
        ("chat_id", "TEXT"),
        ("turn_id", "TEXT"),
        ("job_id", "TEXT"),
        ("upstream_id", "TEXT"),
        ("provider", "TEXT"),
        ("actual_usd", "REAL"),
        ("cost_source", "TEXT NOT NULL DEFAULT 'unknown'"),
        ("metering_status", "TEXT NOT NULL DEFAULT 'unknown'"),
    ] {
        if !columns.iter().any(|c| c == name) {
            c.execute(&format!("ALTER TABLE usage ADD COLUMN {name} {ty}"), [])?;
        }
    }
    c.execute_batch(
        "CREATE UNIQUE INDEX IF NOT EXISTS usage_request ON usage(request_id) WHERE request_id IS NOT NULL;
         CREATE INDEX IF NOT EXISTS usage_session_at ON usage(project_id, session_id, at);
         CREATE INDEX IF NOT EXISTS usage_parent_at ON usage(project_id, parent_session_id, at);"
    )?;
    Ok(())
}

#[derive(Default)]
pub struct UsageRecord<'a> {
    pub project_id: &'a str,
    pub api: &'a str,
    pub model: &'a str,
    /// Disjoint buckets: uncached input, output, cache read, cache write.
    pub tokens: [i64; 4],
    pub usd: f64,
    pub latency_ms: i64,
    pub ttfb_ms: Option<i64>,
    pub context: UsageContext,
    pub request_id: Option<&'a str>,
    pub upstream_id: Option<&'a str>,
    pub provider: Option<&'a str>,
    pub actual_usd: Option<f64>,
    pub cost_source: Option<&'a str>,
    pub metering_status: Option<&'a str>,
}

#[derive(Debug, Default, Serialize)]
pub struct SessionUsage {
    pub session_id: Option<String>,
    #[serde(flatten)]
    pub totals: UsageTotals,
    pub total_tokens: i64,
    /// NULL when no upstream-reported cost is available. Never substitutes local estimates.
    pub actual_usd: Option<f64>,
    pub estimated_usd: f64,
    pub reported_cost_calls: i64,
    pub estimated_cost_calls: i64,
    pub unknown_cost_calls: i64,
    pub incomplete_calls: i64,
}

impl Db {
    /// Returns whether the request was newly recorded; duplicate finalizations never charge twice.
    pub fn record_model_usage(&self, u: UsageRecord<'_>) -> Result<bool> {
        self.with(|c| {
            c.execute(
                "INSERT INTO usage(project_id, at, api, model, input, output, cache_read, cache_write,
                 usd, latency_ms, ttfb_ms, request_id, session_id, session_run_id, parent_session_id,
                 chat_id, turn_id, job_id, upstream_id, provider, actual_usd, cost_source, metering_status)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23)
                 ON CONFLICT DO NOTHING",
                params![u.project_id, arbos_core::ids::now_ms(), u.api, u.model,
                    u.tokens[0], u.tokens[1], u.tokens[2], u.tokens[3], u.usd, u.latency_ms,
                    u.ttfb_ms, u.request_id, u.context.session_id, u.context.session_run_id,
                    u.context.parent_session_id, u.context.chat_id, u.context.turn_id, u.context.job_id,
                    u.upstream_id, u.provider, u.actual_usd, u.cost_source.unwrap_or("unknown"),
                    u.metering_status.unwrap_or("unknown")],
            ).map(|n| n == 1)
        })
    }

    /// [from, to), always scoped to the authorized project. A NULL session bucket retains
    /// historical/background calls. Including children uses OR, so each request is counted once.
    pub fn session_usage(
        &self,
        project: &str,
        session: Option<&str>,
        from: i64,
        to: i64,
        include_children: bool,
    ) -> Result<Vec<SessionUsage>> {
        self.with(|c| {
            // Choose only fixed SQL fragments so single-session queries can use the session
            // indexes instead of scanning every request of a long-lived project.
            let scope = if session.is_some() { "?2" } else { "session_id" };
            let filter = match (session.is_some(), include_children) {
                (false, _) => "",
                (true, false) => "AND session_id = ?2",
                (true, true) => "AND (session_id = ?2 OR parent_session_id = ?2)",
            };
            let mut st = c.prepare(&format!(
                "SELECT {scope} AS scope,
                 COALESCE(SUM(input),0), COALESCE(SUM(output),0), COALESCE(SUM(cache_read),0),
                 COALESCE(SUM(cache_write),0), COALESCE(SUM(usd),0.0), COUNT(*), SUM(actual_usd),
                 COALESCE(SUM(CASE WHEN cost_source = 'estimated' THEN usd ELSE 0 END),0.0),
                 COALESCE(SUM(cost_source = 'reported'),0), COALESCE(SUM(cost_source = 'estimated'),0),
                 COALESCE(SUM(cost_source = 'unknown'),0), COALESCE(SUM(metering_status != 'complete'),0)
                 FROM usage WHERE project_id = ?1 AND at >= ?3 AND at < ?4
                 {filter}
                 GROUP BY scope ORDER BY scope"
            ))?;
            let rows = st.query_map(params![project, session, from, to], |r| {
                let totals = UsageTotals { input: r.get(1)?, output: r.get(2)?, cache_read: r.get(3)?,
                    cache_write: r.get(4)?, usd: r.get(5)?, calls: r.get(6)? };
                Ok(SessionUsage { session_id: r.get(0)?,
                    total_tokens: totals.input + totals.output + totals.cache_read + totals.cache_write,
                    totals, actual_usd: r.get(7)?, estimated_usd: r.get(8)?, reported_cost_calls: r.get(9)?,
                    estimated_cost_calls: r.get(10)?, unknown_cost_calls: r.get(11)?, incomplete_calls: r.get(12)? })
            })?;
            rows.collect()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{test_support::test_state, AuthMode};

    #[test]
    fn migration_keeps_old_costs_unknown_and_is_repeatable() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE usage (
            id INTEGER PRIMARY KEY, project_id TEXT, at INTEGER, api TEXT, model TEXT,
            input INTEGER, output INTEGER, cache_read INTEGER, cache_write INTEGER,
            usd REAL, latency_ms INTEGER, ttfb_ms INTEGER);
            INSERT INTO usage VALUES(1,'p',10,'openai','m',100,20,0,0,0.25,50,NULL);",
        )
        .unwrap();
        migrate(&c).unwrap();
        migrate(&c).unwrap();
        let old: (f64, Option<f64>, Option<String>, String) = c
            .query_row(
                "SELECT usd, actual_usd, session_id, cost_source FROM usage",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(old, (0.25, None, None, "unknown".into()));
    }

    fn record(
        db: &Db,
        project: &str,
        session: &str,
        parent: Option<&str>,
        req: &str,
        actual: Option<f64>,
    ) -> bool {
        db.record_model_usage(UsageRecord {
            project_id: project,
            api: "openai",
            model: "m",
            tokens: [10, 20, 30, 40],
            usd: actual.unwrap_or(0.1),
            actual_usd: actual,
            request_id: Some(req),
            context: UsageContext {
                session_id: Some(session.into()),
                parent_session_id: parent.map(str::to_owned),
                ..Default::default()
            },
            cost_source: Some(if actual.is_some() {
                "reported"
            } else {
                "estimated"
            }),
            metering_status: Some("complete"),
            ..Default::default()
        })
        .unwrap()
    }

    #[test]
    fn sessions_sum_calls_across_resumes_and_count_children_once() {
        let state = test_state(AuthMode::Paircode);
        let db = &state.db;
        assert!(record(db, "p", "chat", None, "r1", Some(0.1)));
        assert!(record(db, "p", "chat", Some("chat"), "r2", Some(0.0)));
        assert!(record(db, "p", "worker", Some("chat"), "r3", None));
        assert!(record(db, "other", "chat", None, "r4", Some(0.1)));
        assert!(!record(db, "p", "chat", None, "r1", Some(99.0)));
        let own = db
            .session_usage("p", Some("chat"), 0, i64::MAX, false)
            .unwrap();
        assert_eq!(own[0].totals.calls, 2);
        assert_eq!(own[0].total_tokens, 200);
        assert_eq!(own[0].actual_usd, Some(0.1));
        assert!((own[0].totals.usd - 0.1).abs() < 1e-9);
        let all = db
            .session_usage("p", Some("chat"), 0, i64::MAX, true)
            .unwrap();
        assert_eq!(all[0].totals.calls, 3);
        assert_eq!(all[0].reported_cost_calls, 2);
        assert_eq!(all[0].estimated_cost_calls, 1);
        assert!((all[0].estimated_usd - 0.1).abs() < 1e-9);
        assert!((all[0].totals.usd - 0.2).abs() < 1e-9);
        assert_eq!(
            db.session_usage("p", None, 0, i64::MAX, false)
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn time_windows_are_half_open_and_unknown_cost_is_not_free() {
        let state = test_state(AuthMode::Paircode);
        let db = &state.db;
        db.record_usage("p", "anthropic", "m", 1, 2, 3, 4, 0.5, 0, None)
            .unwrap();
        db.with(|c| c.execute("UPDATE usage SET at = 100", []))
            .unwrap();
        assert!(db
            .session_usage("p", None, 0, 100, false)
            .unwrap()
            .is_empty());
        let rows = db.session_usage("p", None, 100, 101, false).unwrap();
        assert_eq!(rows[0].session_id, None);
        assert_eq!(rows[0].actual_usd, None);
        assert_eq!(rows[0].unknown_cost_calls, 1);
        assert_eq!(rows[0].incomplete_calls, 1);
        assert_eq!(rows[0].totals.usd, 0.5);
        assert_eq!(rows[0].total_tokens, 10);
    }

    #[tokio::test]
    async fn session_api_checks_project_access_and_validates_windows() {
        use crate::{
            api,
            auth::Principal,
            people::tests_support::{person, project_of},
        };
        use axum::extract::{Path, Query, State};
        let state = test_state(AuthMode::Github);
        let (owner, _) = person(&state, 9811);
        let (_, outsider) = person(&state, 9812);
        let project = project_of(&state, &owner, "usage-owner");
        let err = api::session_usage(
            State(state.clone()),
            outsider,
            Path((project.clone(), "chat".into())),
            Query(Default::default()),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::NOT_FOUND);
        let err = api::sessions_usage(
            State(state.clone()),
            Principal::Admin,
            Path(project.clone()),
            Query(api::SessionUsageQuery {
                from: Some(10),
                to: Some(10),
                ..Default::default()
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
        let response = api::session_usage(
            State(state),
            Principal::Admin,
            Path((project, "unused".into())),
            Query(Default::default()),
        )
        .await
        .unwrap();
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["usage"]["calls"], 0);
        assert!(value["usage"]["actual_usd"].is_null());
    }
}
