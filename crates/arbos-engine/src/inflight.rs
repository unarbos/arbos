//! A tool call's record before it runs. The transcript gets its `tool`
//! line when the call returns; a kernel that dies in between leaves no
//! trace, so the continued turn's model issues the command again and a
//! side effect happens twice (qal-j02: `echo >> side-effects.log` ran
//! twice across a `kill -9`). Each running call is a file here from the
//! moment it starts until its result is written; the next kernel turns
//! whatever is left into a `tool` line that says the call was cut, so
//! the model checks before it repeats.
//!
//! `agents/<id>/inflight/<call_id>.json`, the `ToolRec` as it was emitted
//! at the start (name, args, started, no result).

use arbos_core::{AgentId, Place, ToolRec};
use std::path::PathBuf;

fn dir(place: &Place, agent: &AgentId) -> PathBuf {
    place.agent_dir(agent.as_str()).join("inflight")
}

fn file(place: &Place, agent: &AgentId, call_id: &str) -> PathBuf {
    // A call id is the provider's; keep it a plain file name.
    let safe: String = call_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    dir(place, agent).join(format!("{safe}.json"))
}

/// The call is about to run. Not best effort: the record is what keeps a
/// command from running twice across a kernel death (qal-j02), and a
/// record that could not be written is a guarantee silently gone. The
/// caller refuses the call and says why (the qal-j08 family; see
/// `arbos_core::record`).
pub fn start(place: &Place, agent: &AgentId, rec: &ToolRec) -> anyhow::Result<()> {
    let path = file(place, agent, &rec.call_id);
    let json = serde_json::to_vec(rec)?;
    arbos_core::record::write_atomic(&path, &json)
}

/// The call returned (its `tool` line follows on the transcript).
pub fn end(place: &Place, agent: &AgentId, call_id: &str) {
    let _ = std::fs::remove_file(file(place, agent, call_id));
}

/// The call is waiting for the user's allow/deny: a sibling marker, so a
/// kernel that dies now writes the call up as never run, not as one that
/// may have completed.
pub fn waiting_for_approval(place: &Place, agent: &AgentId, call_id: &str) {
    let _ = std::fs::write(approval_marker(place, agent, call_id), b"");
}

pub fn approval_settled(place: &Place, agent: &AgentId, call_id: &str) {
    let _ = std::fs::remove_file(approval_marker(place, agent, call_id));
}

fn approval_marker(place: &Place, agent: &AgentId, call_id: &str) -> PathBuf {
    let mut p = file(place, agent, call_id);
    p.set_extension("approval");
    p
}

/// Whether the call was waiting for the user when the kernel died.
fn was_waiting_for_approval(place: &Place, agent: &AgentId, call_id: &str) -> bool {
    approval_marker(place, agent, call_id).exists()
}

/// What is running now, oldest first, the files left as they are. For a
/// kernel asking what a silent turn is waiting on.
pub fn peek(place: &Place, agent: &AgentId) -> Vec<ToolRec> {
    let Ok(rd) = std::fs::read_dir(dir(place, agent)) else {
        return vec![];
    };
    let mut recs: Vec<ToolRec> = rd
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| serde_json::from_slice::<ToolRec>(&std::fs::read(e.path()).ok()?).ok())
        .collect();
    recs.sort_by_key(|r| r.started.unwrap_or(0));
    recs
}

/// What was running when the last kernel died: every record, oldest
/// first, and the files gone.
pub fn take(place: &Place, agent: &AgentId) -> Vec<ToolRec> {
    let d = dir(place, agent);
    let Ok(rd) = std::fs::read_dir(&d) else {
        return vec![];
    };
    let mut recs: Vec<ToolRec> = rd
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| {
            let text = std::fs::read(e.path()).ok()?;
            let mut rec = serde_json::from_slice::<ToolRec>(&text).ok()?;
            if was_waiting_for_approval(place, agent, &rec.call_id) {
                // Carried on the record for `cut_record`: the body says
                // it never ran.
                rec.result_size = Some(0);
                rec.error = Some(WAITING.into());
            }
            let _ = std::fs::remove_file(e.path());
            let _ = std::fs::remove_file(approval_marker(place, agent, &rec.call_id));
            Some(rec)
        })
        .collect();
    recs.sort_by_key(|r| r.started.unwrap_or(0));
    let _ = std::fs::remove_dir(&d);
    recs
}

/// Marker `take` sets on a record that was waiting for approval.
const WAITING: &str = "waiting for approval";

/// The `tool` line the next kernel writes for a call it found cut: the
/// same call (name, args, call id, when it started) with the result the
/// model reads instead of an output.
pub fn cut_record(mut rec: ToolRec, now_ms: i64) -> ToolRec {
    if rec.error.as_deref() == Some(WAITING) {
        let what = rec
            .args
            .as_ref()
            .and_then(|a| {
                a.get("command")
                    .or_else(|| a.get("path"))
                    .or_else(|| a.get("task"))
            })
            .and_then(|v| v.as_str())
            .map(|s| format!(" ({})", arbos_core::text::clip(s, 120)))
            .unwrap_or_default();
        rec.ended = Some(now_ms);
        rec.error = Some(
            "not run: the kernel restarted while this waited for the user's allow/deny".into(),
        );
        rec.body = Some(format!(
            "[kernel] this {} call{what} was waiting for the user to allow or deny it when the kernel restarted; it never ran, and the question is gone with the turn. Nothing changed. If the user still wants it, call it again and they will be asked afresh.",
            rec.name
        ));
        return rec.with_output();
    }
    let ran_for = rec
        .started
        .map(|s| ((now_ms - s).max(0) / 1000).to_string() + "s")
        .unwrap_or_else(|| "an unknown time".into());
    let what = rec
        .args
        .as_ref()
        .and_then(|a| {
            a.get("command")
                .or_else(|| a.get("path"))
                .or_else(|| a.get("task"))
        })
        .and_then(|v| v.as_str())
        .map(|s| format!(" ({})", arbos_core::text::clip(s, 120)))
        .unwrap_or_default();
    let text = format!(
        "[kernel] the kernel restarted while this {} call was running{what}; it had been running for {ran_for} and may have completed in part or in full. Do not run it again unchecked: look at what it changed first (the file, the log, the branch), then continue from there or tell the user it was interrupted.",
        rec.name
    );
    rec.ended = Some(now_ms);
    rec.error = Some("interrupted: the kernel restarted while this ran".into());
    rec.body = Some(text);
    rec.with_output()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(call_id: &str, cmd: &str, started: i64) -> ToolRec {
        ToolRec {
            name: "bash".into(),
            call_id: call_id.into(),
            step: 1,
            paths: vec![],
            started: Some(started),
            ended: None,
            result_size: None,
            error: None,
            body: None,
            args: Some(serde_json::json!({"command": cmd})),
            child: None,
            images: vec![],
            diff: None,
            label: None,
            output: None,
        }
    }

    #[test]
    fn a_started_call_is_a_file_until_it_ends_and_a_leftover_is_taken_once() {
        let dir = tempfile::tempdir().unwrap();
        let place = Place::new(dir.path());
        let agent = AgentId::new("root");
        start(&place, &agent, &rec("call/1", "echo a >> log", 1_000));
        start(&place, &agent, &rec("call_2", "sleep 30", 2_000));
        end(&place, &agent, "call_2");
        let left = take(&place, &agent);
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].call_id, "call/1");
        assert!(take(&place, &agent).is_empty(), "taken once");
        let cut = cut_record(left.into_iter().next().unwrap(), 31_000);
        assert_eq!(cut.ended, Some(31_000));
        assert!(cut.error.as_deref().unwrap().starts_with("interrupted"));
        let body = cut.body.unwrap();
        assert!(
            body.contains("(echo a >> log)") && body.contains("30s"),
            "{body}"
        );
        assert!(body.contains("Do not run it again unchecked"), "{body}");
    }
}
