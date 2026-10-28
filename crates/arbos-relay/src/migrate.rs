//! Migration from one-sandbox-per-project to one-box-per-user (D70). For every legacy sandbox of a
//! user's project: copy its `/work` (repo + `.arbos` state: SQLite, pi and Prime sessions, skills,
//! blobs) into the user box as `/work/<project>`, then stop the old sandbox and mark it
//! `migrated_to`. Idempotent: rows already marked are skipped; a failed project does not stop the
//! others. Old sandboxes are never deleted here.

use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};

use arbos_core::ids::now_ms;

use crate::boxes::{self, Provider};
use crate::daytona::SandboxState;
use crate::state::AppState;

/// One project's outcome.
#[derive(Debug, serde::Serialize)]
pub struct MigrationReport {
    pub project_id: String,
    pub legacy_sandbox: Option<String>,
    pub outcome: String,
    pub work_mib: Option<i64>,
    pub ms: i64,
}

/// Migrate every unmigrated legacy sandbox whose project belongs to `user_id`.
pub async fn migrate_user(
    state: &AppState,
    user_id: &str,
    only_project: Option<&str>,
    force: bool,
) -> Result<Vec<MigrationReport>> {
    let projects = state.db.projects_of_user(user_id)?;
    let legacy = state.db.legacy_sandboxes()?;
    let mut reports = Vec::new();
    // The target box must exist and run before any copy.
    let provider_default = boxes::provider_for_user(state, user_id);
    let target = boxes::ensure_user_box(state, user_id, provider_default).await?;
    let target_provider =
        Provider::parse(&target.provider).ok_or_else(|| anyhow!("bad target provider"))?;
    if target_provider == Provider::Daytona {
        let dt = state
            .daytona
            .as_ref()
            .ok_or_else(|| anyhow!("DAYTONA_API_KEY not configured"))?;
        let id = target
            .sandbox_id
            .clone()
            .ok_or_else(|| anyhow!("target box has no sandbox id"))?;
        let cur = dt.get(&id).await?;
        if cur.state != SandboxState::Started {
            boxes::ensure_awake(state, user_id, "migration").await;
            dt.wait_for(&id, Duration::from_secs(180), |s| {
                s.state == SandboxState::Started
            })
            .await?;
        }
    }
    for (project_id, legacy_sb, migrated_to) in legacy {
        let t0 = std::time::Instant::now();
        let Some(project) = projects.iter().find(|p| p.id == project_id) else {
            continue; // another user's project (or deleted)
        };
        if only_project.is_some_and(|o| o != project_id) {
            continue;
        }
        if migrated_to.is_some() && !force {
            reports.push(MigrationReport {
                project_id: project_id.clone(),
                legacy_sandbox: legacy_sb.sandbox_id.clone(),
                outcome: format!("already migrated to {}", migrated_to.unwrap_or_default()),
                work_mib: None,
                ms: 0,
            });
            continue;
        }
        let res = migrate_one(state, user_id, &project.id, &legacy_sb, &target).await;
        let (outcome, work_mib) = match res {
            Ok(Step::Copied(mib)) => {
                let _ = state.db.mark_legacy_migrated(&project_id, user_id);
                if let Ok(Some(mut b)) = state.db.user_box(user_id) {
                    if !b.migrated_from.contains(&project_id) {
                        b.migrated_from.push(project_id.clone());
                        let _ = state.db.upsert_box(&b);
                    }
                }
                ("migrated".to_owned(), Some(mib))
            }
            Ok(Step::Skipped(why)) => {
                // Nothing to carry over; marked so the next run stays quiet.
                let _ = state
                    .db
                    .mark_legacy_migrated(&project_id, &format!("skipped: {why}"));
                (format!("skipped: {why}"), None)
            }
            Err(e) => (format!("failed: {e:#}"), None),
        };
        tracing::info!(project = %project_id, outcome = %outcome, "migration step");
        reports.push(MigrationReport {
            project_id,
            legacy_sandbox: legacy_sb.sandbox_id.clone(),
            outcome,
            work_mib,
            ms: t0.elapsed().as_millis() as i64,
        });
    }
    Ok(reports)
}

/// One project's migration outcome.
pub enum Step {
    /// `/work` copied; size in MiB.
    Copied(i64),
    /// Nothing to copy (legacy sandbox gone, or a local sandbox for a Daytona box).
    Skipped(String),
}

/// Copy one legacy sandbox's `/work` into the user box's `/work/<project>`.
async fn migrate_one(
    state: &AppState,
    user_id: &str,
    project_id: &str,
    legacy: &crate::db::Sandbox,
    target: &crate::db::Sandbox,
) -> Result<Step> {
    let legacy_provider =
        Provider::parse(&legacy.provider).ok_or_else(|| anyhow!("bad legacy provider"))?;
    let target_provider =
        Provider::parse(&target.provider).ok_or_else(|| anyhow!("bad target provider"))?;
    match (legacy_provider, target_provider) {
        (Provider::Process, Provider::Process) => {
            // Local: stop the old agentd if it still runs, then copy the directory.
            if let Some(pid) = legacy.local_pid {
                let _ = nix::sys::signal::kill(
                    nix::unistd::Pid::from_raw(pid as i32),
                    nix::sys::signal::Signal::SIGTERM,
                );
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            let src = state
                .cfg
                .data_dir
                .join("boxes")
                .join(project_id)
                .join("work");
            let dst = state
                .cfg
                .data_dir
                .join("boxes")
                .join(user_id)
                .join("work")
                .join(project_id);
            if !src.exists() {
                return Err(anyhow!("legacy work dir {} is missing", src.display()));
            }
            hold_workspace(state, target, project_id, true).await?;
            let _ = std::fs::remove_dir_all(&dst);
            std::fs::create_dir_all(dst.parent().unwrap())?;
            let st = tokio::process::Command::new("cp")
                .args(["-a", &src.display().to_string(), &dst.display().to_string()])
                .status()
                .await?;
            hold_workspace(state, target, project_id, false).await?;
            if !st.success() {
                return Err(anyhow!("cp -a failed"));
            }
            Ok(Step::Copied(dir_size_mib(&dst)))
        }
        (Provider::Daytona, Provider::Daytona) => {
            let dt = state
                .daytona
                .as_ref()
                .ok_or_else(|| anyhow!("DAYTONA_API_KEY not configured"))?;
            let old_id = legacy
                .sandbox_id
                .clone()
                .ok_or_else(|| anyhow!("legacy sandbox has no id"))?;
            let new_id = target
                .sandbox_id
                .clone()
                .ok_or_else(|| anyhow!("target box has no sandbox id"))?;
            // 1. The old sandbox must run for the toolbox to read it.
            let cur = match dt.get(&old_id).await {
                Ok(c) => c,
                Err(e) if e.to_string().contains("404") => {
                    return Ok(Step::Skipped(format!(
                        "legacy sandbox {old_id} no longer exists"
                    )))
                }
                Err(e) => return Err(e),
            };
            if matches!(
                cur.state,
                SandboxState::Destroyed | SandboxState::Destroying
            ) {
                return Ok(Step::Skipped(format!(
                    "legacy sandbox {old_id} is destroyed"
                )));
            }
            if cur.state != SandboxState::Started {
                dt.start(&old_id).await?;
                dt.wait_for(&old_id, Duration::from_secs(180), |s| {
                    s.state == SandboxState::Started
                })
                .await?;
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
            // 2. Quiesce: the legacy agentd restarts with the sandbox and would write to the
            //    SQLite files while they are packed; stop it so the WAL is checkpointed on close.
            let _ = dt
                .toolbox_exec(
                    &old_id,
                    "sudo -n pkill -TERM -x arbos-agentd 2>/dev/null; sleep 3; sudo -n pkill -KILL -x arbos-agentd 2>/dev/null; sudo -n pkill -KILL -f 'pi --mode rpc' 2>/dev/null; sudo -n pkill -KILL -x prime-agent 2>/dev/null; true",
                    60,
                )
                .await;
            // 3. Size, then one tarball of /work (WAL/SHM included: a quiescent WAL is consistent).
            let du = dt
                .toolbox_exec(&old_id, "sudo -n du -sm /work | cut -f1", 120)
                .await?;
            let mib: i64 = du
                .get("result")
                .and_then(Value::as_str)
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(0);
            if mib > 3500 {
                return Err(anyhow!(
                    "/work is {mib} MiB; too large to copy through the relay in one piece"
                ));
            }
            let tar = dt
                .toolbox_exec(
                    &old_id,
                    // Not /tmp: fs.protected_regular stops root from reopening another user's file there.
                    "sudo -n rm -f /tmp/arbos-migrate.tgz /home/daytona/arbos-migrate.tgz; cd /work && sudo -n tar czf /home/daytona/arbos-migrate.tgz --exclude='./.arbos/logs' . && sudo -n chmod 644 /home/daytona/arbos-migrate.tgz && ls -l /home/daytona/arbos-migrate.tgz",
                    900,
                )
                .await?;
            if tar.get("exitCode").and_then(Value::as_i64).unwrap_or(1) != 0 {
                return Err(anyhow!(
                    "tar on the legacy box failed: {}",
                    tar.get("result").and_then(Value::as_str).unwrap_or("")
                ));
            }
            let bytes = dt
                .toolbox_download(&old_id, "/home/daytona/arbos-migrate.tgz")
                .await
                .context("download tarball")?;
            tracing::info!(project = %project_id, mib, bytes = bytes.len(), "legacy /work packed");
            // 4. Stop the workspace on the user box, replace its directory, release it.
            hold_workspace(state, target, project_id, true).await?;
            // The held workspace's children have just exited; give their last writes a moment.
            tokio::time::sleep(Duration::from_secs(2)).await;
            let remote_tgz = format!("/tmp/arbos-migrate-{project_id}.tgz");
            let upload = dt.toolbox_upload(&new_id, &remote_tgz, bytes).await;
            let extract = match upload {
                Ok(()) => {
                    dt.toolbox_exec(
                        &new_id,
                        &format!(
                            "for i in 1 2 3 4 5; do sudo -n rm -rf /work/{p} && break; sleep 2; done; sudo -n mkdir -p /work/{p} && sudo -n tar xzf {t} -C /work/{p} && sudo -n rm -f /work/{p}/.arbos/.owner; rm -f {t} && echo ok",
                            p = project_id,
                            t = remote_tgz
                        ),
                        900,
                    )
                    .await
                }
                Err(e) => Err(e),
            };
            hold_workspace(state, target, project_id, false).await?;
            let out = extract?;
            if out.get("exitCode").and_then(Value::as_i64).unwrap_or(1) != 0 {
                return Err(anyhow!(
                    "extract on the user box failed: {}",
                    out.get("result").and_then(Value::as_str).unwrap_or("")
                ));
            }
            let _ = dt
                .toolbox_exec(&old_id, "sudo -n rm -f /home/daytona/arbos-migrate.tgz", 60)
                .await;
            // 5. Hibernate the old sandbox; it stays as a fallback (never deleted here).
            let _ = dt.stop(&old_id, false).await;
            Ok(Step::Copied(mib))
        }
        (Provider::Process, Provider::Daytona) => Ok(Step::Skipped(
            "local process sandbox; its files stay under the relay data dir".into(),
        )),
        (a, b) => Err(anyhow!(
            "cannot migrate a {} sandbox into a {} box",
            a.as_str(),
            b.as_str()
        )),
    }
}

/// Ask the user box's agentd to stop (hold) or restart (release) one workspace.
async fn hold_workspace(
    state: &AppState,
    target: &crate::db::Sandbox,
    project_id: &str,
    hold: bool,
) -> Result<()> {
    let Some(url) = &target.box_url else {
        return Err(anyhow!("user box has no url yet"));
    };
    let verb = if hold { "hold" } else { "release" };
    let mut req = state
        .http
        .post(format!("{url}/w/{project_id}/{verb}"))
        .bearer_auth(&target.mcp_token)
        .timeout(Duration::from_secs(60));
    if let Some(t) = &target.preview_token {
        req = req.header("x-daytona-preview-token", t);
    }
    let r = req
        .send()
        .await
        .with_context(|| format!("{verb} workspace"))?;
    if !r.status().is_success() {
        return Err(anyhow!("{verb} workspace: HTTP {}", r.status()));
    }
    Ok(())
}

fn dir_size_mib(p: &std::path::Path) -> i64 {
    fn walk(p: &std::path::Path) -> u64 {
        let mut n = 0;
        if let Ok(rd) = std::fs::read_dir(p) {
            for e in rd.flatten() {
                let path = e.path();
                if path.is_dir() {
                    n += walk(&path);
                } else if let Ok(m) = path.metadata() {
                    n += m.len();
                }
            }
        }
        n
    }
    (walk(p) / (1024 * 1024)) as i64
}

/// JSON for the API: the reports plus the box they went to.
pub fn report_json(user_id: &str, reports: &[MigrationReport]) -> Value {
    json!({ "user_id": user_id, "at": now_ms(), "reports": reports })
}
