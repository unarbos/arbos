//! The last word on a job: a job that ended owes its chat a row, whatever killed it.
//!
//! Bug AJ (Jacob, Test5 round 16, 2026-11-05, agentd 0.1.197 / ef90488f8, scenario `r16burst`).
//! Eight heavy compute asks went in together, the workspace cgroup went over `memory.max`, and
//! the kernel's OOM killer took the **main** pi child five times in fifty seconds. Two of the
//! eight answers posted. Six posted nothing — and five of those six left a `jobs` row reading
//! `failed`, with the right error in it, that no part of the chat ever showed.
//!
//! The box knew. The user did not.
//!
//! **Why the silence was on the path with the retry protection.** A job started by `start_job`
//! runs through `spawn_subagent_task` with a `JobSpec`, and that path is the careful one: a
//! failed worker gets one fresh start, and `wait_for_live_pi` holds it until the box has a main
//! child again (bug V). What it does not have is a last-resort row. It settles its worker rows
//! deliberately quietly and hands every user-facing word to `run_job_fallback_turn` — which runs
//! through `drive_turn`, so it needs the main pi child. That is the process most likely to be
//! gone at the moment it is needed, because whatever killed the worker usually killed it too.
//! When the fallback failed, the error went to the log and the task returned.
//!
//! The other path, `start_job_for_send` (an ask run beside a busy turn), gets no retry at all —
//! and ends by writing a `job_failed` row whenever nothing else answered the ask. It always
//! speaks. This module gives the careful path the same guarantee, through
//! [`Agent::write_job_result`], which writes its row with a direct model call straight into the
//! doc and so does not need a pi child to say anything.
//!
//! **`delivered_key` is the condition, and it was already there.** Every delivery path —
//! `write_job_result` for a result, `run_job_fallback_turn` for inline work — already records
//! that an answer reached the chat on the job's own row. Nothing in the repo had ever read it.
//! That unread field is exactly the bookkeeping this needs, so the backstop consults it rather
//! than adding a second flag beside it.

use std::sync::Arc;

use arbos_core::doc::MessageView;
use serde_json::Value;

use crate::agent::{Agent, JobSpec};
use crate::sync::SharedHost;

/// A row `start_job` wrote as a receipt for this job rather than as an answer to the ask: the
/// "running in the background now" acknowledgement and the "queued behind …" line, both posted
/// through `post_chat_line` under a `deliveryKey`.
///
/// Counting either as the answer would reinstate the bug: every forced ask in `r16burst` was
/// acknowledged, and then six of them died without another word.
fn is_job_receipt(m: &MessageView, job_id: &str) -> bool {
    m.rest
        .get("deliveryKey")
        .and_then(Value::as_str)
        .is_some_and(|k| k == format!("job:{job_id}:ack") || k == format!("job:{job_id}:queued"))
}

impl Agent {
    /// Whether `ask_msg_id` already holds an answer a client would draw — any visible assistant
    /// row with words in it that is not one of this job's own receipts.
    ///
    /// `hidden` rows are skipped by both apps, and a row settled without a line is wordless, so
    /// neither is something the user saw.
    async fn chat_answered_ask(&self, host: &SharedHost, ask_msg_id: &str, job_id: &str) -> bool {
        let h = host.lock().await;
        h.doc.messages().into_iter().any(|m| {
            m.role == "assistant"
                && m.rest.get("inReplyTo").and_then(Value::as_str) == Some(ask_msg_id)
                && m.rest.get("hidden").and_then(Value::as_bool) != Some(true)
                && !m.text().trim().is_empty()
                && !is_job_receipt(&m, job_id)
        })
    }

    /// Every way of doing this job's work has been tried and so has the turn that was meant to
    /// report it. Post the one row the chat is owed, and record on the job row that it landed.
    ///
    /// Silent in the three cases where a row would be wrong or duplicated: a job the user
    /// stopped (the stop line already said so), a job whose answer is already recorded as
    /// delivered, and a job whose ask already carries a visible answer — which is what a
    /// fallback turn that got far enough to write prose before failing leaves behind.
    pub(crate) async fn report_lost_job(
        self: &Arc<Self>,
        chat_id: &str,
        host: &SharedHost,
        ask_msg_id: &str,
        job: &JobSpec,
        error: &str,
    ) {
        let Ok(Some(row)) = self.store.job(&job.id) else {
            return;
        };
        if row.state == "cancelled" {
            return;
        }
        if let Some(key) = &row.delivered_key {
            tracing::info!(chat_id, job = %job.id, key, "the job's answer already reached the chat; no failure row");
            return;
        }
        if self.chat_answered_ask(host, ask_msg_id, &job.id).await {
            // Something answered the ask without recording it. Say so on the row, so a later
            // reader of `delivered_key` does not take this job for one that went silent.
            let _ = self
                .store
                .set_job_delivered(&job.id, &format!("job:{}:answered", job.id));
            return;
        }
        tracing::warn!(
            chat_id,
            job = %job.id,
            title = %job.title,
            error,
            "every worker and the inline fallback failed; the chat is told the job died"
        );
        let notes = format!(
            "This work did not run. The job \"{}\" was started for the user's message and every \
             attempt to do it failed before it produced anything: both of its workers ended \
             early, and so did the attempt to do the work in the main session. The error each \
             time was: {error}\n\nWhat the job was asked to do:\n{}",
            job.title,
            row.brief.trim(),
        );
        // Returns `Err` by design once `error` is set, after the row is written, so the result
        // cannot stand for whether anything was posted — the doc is asked instead, below.
        if let Err(e) = self
            .write_job_result(
                chat_id,
                host,
                ask_msg_id,
                &job.title,
                "failed",
                &notes,
                Some(error),
            )
            .await
        {
            tracing::info!(chat_id, job = %job.id, error = %e, "the job's failure row was written");
        }
        if self.chat_answered_ask(host, ask_msg_id, &job.id).await {
            let _ = self
                .store
                .set_job_delivered(&job.id, &format!("job:{}:lost", job.id));
        } else {
            // Nothing is left to try. Loud, because a job row that ends `failed` with no
            // `delivered_key` is precisely the state bug AJ left six asks in.
            tracing::error!(
                chat_id,
                job = %job.id,
                error,
                "the job died and the chat could not be told"
            );
        }
    }
}
