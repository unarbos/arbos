//! What this kernel holds: its jobs, its shells, its browser pages, with
//! their states — the answer to a client's `surfaces` frame. A window that
//! reattaches after a kernel died and a replacement answered has rows from
//! its memory (a job ticking, a terminal open) that the new kernel never
//! heard of; this list is the record it reconciles against. Every row the
//! kernel has, and nothing it does not.

use arbos_core::Place;
use arbos_core::wire::Surface;
use arbos_engine::{JobStatus as Status, JobsRoot};

use crate::hooks::KernelHooks;

/// The one browser page an agent has (`tools.rs` keys the row on it).
const BROWSER_PAGE: &str = "b1";

/// Every surface of the place, or of one owner when `agent` is given.
/// Jobs come from disk (the kernel's own record, which a replacement
/// kernel inherits); shells and pages from this process's hubs.
pub fn list(place: &Place, hooks: &KernelHooks, agent: Option<&str>) -> Vec<Surface> {
    let mut out = Vec::new();
    let agents: Vec<String> = match agent {
        Some(a) => vec![a.to_string()],
        None => arbos_core::list_agents(place)
            .unwrap_or_default()
            .into_iter()
            .map(|a| a.id.0)
            .collect(),
    };
    for id in &agents {
        let jobs = JobsRoot::for_agent(place, &arbos_core::AgentId::new(id.clone()));
        for job in jobs.list() {
            let journal = job.journal();
            let (running, exit) = match job.status {
                Status::Running => (true, None),
                Status::Exited(code) => (false, Some(code)),
                Status::Killed => (false, None),
            };
            out.push(Surface {
                owner: id.clone(),
                panel: "process".into(),
                id: job.id.clone(),
                cwd: Some(job.meta.cwd.display().to_string()),
                title: Some(job.meta.command.replace('\n', " ")),
                url: Some(journal.display().to_string()),
                by: "agent".into(),
                running,
                status: job.status_line(),
                pid: Some(job.meta.pid),
                started_ms: Some(job.meta.started_ms),
                ended_ms: job.ended_ms,
                exit,
                journal: Some(if journal.exists() { "present" } else { "gone" }.into()),
            });
        }
    }
    let shells = hooks.ptys.get().map(|p| p.list()).unwrap_or_default();
    for row in shells {
        // A shell docks under its owner; a page a client wrote to before any
        // shell was opened has none and docks under the pty's agent.
        let owner = if row.owner.is_empty() {
            row.agent.clone()
        } else {
            row.owner.clone()
        };
        if agent.is_some_and(|a| a != owner) {
            continue;
        }
        out.push(Surface {
            owner,
            panel: "terminal".into(),
            id: row.page.clone(),
            cwd: Some(row.cwd.display().to_string()),
            title: None,
            url: None,
            by: row.by.clone(),
            running: row.alive,
            status: if row.alive {
                format!("shell alive (pid {})", row.pid)
            } else {
                "shell gone".to_string()
            },
            pid: Some(row.pid),
            started_ms: Some(row.started_ms),
            ended_ms: None,
            exit: None,
            journal: None,
        });
    }
    for (owner, url) in hooks.browsers.pages() {
        if agent.is_some_and(|a| a != owner) {
            continue;
        }
        out.push(Surface {
            owner,
            panel: "browser".into(),
            id: BROWSER_PAGE.into(),
            cwd: None,
            title: None,
            url: Some(url),
            by: "agent".into(),
            running: true,
            status: "page open".into(),
            pid: None,
            started_ms: None,
            ended_ms: None,
            exit: None,
            journal: None,
        });
    }
    out
}
