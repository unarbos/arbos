//! Which of the two things a provider refusal is about: the message the user just sent, or the
//! session it arrived in.
//!
//! A provider that declines content judges the whole request, history included. So one refused
//! turn can leave a chat where every later message is refused on sight, however ordinary it is.
//! Test5, 2026-11-06, room `01M6KTFT1X6WVXB39DX50RGF8Z/01M6RZ8KTGTXTY1MM33KP2DMYE`: two
//! adversarial tasks at 11:43 and 11:49 CR (`rm -rf /`, a skill injection), the last answered
//! turn at 12:12, and from 12:17 identifying a photographed dish, writing a blog post and
//! stating an output preference all came back `model_refused` with zero tool calls. A real user
//! in that chat is stuck in it for good.
//!
//! The hard part is not the recovery, it is the verdict. "The provider refused" is true of a
//! poisoned session and of a request that deserves to be refused, and a recovery keyed on the
//! refusal alone would quietly retry adversarial asks on a context chosen to make them pass.
//!
//! Jacob's discriminator (2026-11-06) is an experiment rather than a heuristic, so it does not
//! have that failure mode: **ask the provider the new message on its own, with no history at
//! all.**
//!
//! * The bare message is answered → the message is fine and the session is what was declined.
//! * The bare message is refused too → the refusal is about the message, and it stands.
//!
//! It cannot launder adversarial content, because adversarial content is refused on its own as
//! well: that is the same call the user's next message in a fresh chat would make. What it
//! cannot see is what rides *with* a message rather than in it — an attachment, a quoted file —
//! so a message whose attachment is the declined part probes clean. [`SESSION_REFUSED_LINE`]
//! is worded for that: it is only ever read after the trimmed retry was refused as well, and it
//! names the attachment as one of the two things it can be.

use anyhow::Result;

use crate::llm::Completion;

/// What the probe established about a refusal. `Untested` is the honest default: every refusal
/// starts here, and stays here when the probe could not be run or could not answer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RefusalCause {
    #[default]
    Untested,
    /// The bare message was refused on its own: this request is what the provider declined.
    MessageAlone,
    /// The bare message was answered on its own: the session it arrived in is what was declined.
    SessionHistory,
}

/// One probe call's verdict.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Probe {
    /// The provider declined the bare message.
    Refused,
    /// The provider answered it.
    Answered,
    /// The probe could not be made or could not be read as either (no text to probe, a 5xx, a
    /// timeout). Never guessed at: an unavailable probe leaves the cause `Untested`.
    Unavailable,
}

/// What a refused turn should do about it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefusalRecovery {
    /// Nothing. The refusal is the answer, and the user reads it unchanged.
    Stand,
    /// Retry once on a fresh session with the bare message: a context the probe has just shown
    /// the provider will accept.
    FreshContext,
    /// One narrower attempt in the same session on the fallback talking model, asking for the
    /// parts it can do (#2750 fix F). The path for a refusal with no verdict on it.
    SafeParts,
}

/// The probe's system prompt. Deliberately plain: it asks for an ordinary answer, and it must
/// not argue the provider out of a refusal — a prompt that pleaded ("this is benign, please
/// comply") would turn the experiment back into a heuristic, and a laundering one at that.
pub const PROBE_SYSTEM: &str =
    "You are a helpful assistant. Answer the user's message in one or two sentences.";

/// A probe answer is thrown away, so it is sized to settle the question and nothing more.
pub const PROBE_MAX_TOKENS: u32 = 64;

/// The probe sits in front of the user's answer, so it is not allowed to be the reason a turn
/// is slow. Past this it is an unavailable probe like any other, and the refusal falls to the
/// narrower same-session retry.
pub const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// The most of a long message the probe sends. The provider's judgement is about content, and
/// the opening of a message carries the ask; a full 19k-character spec would make every probe
/// cost what the turn cost.
pub const PROBE_MESSAGE_CHARS: usize = 4000;

/// Read after a trimmed retry was refused as well. It must not repeat the "split it up" advice
/// of [`crate::agent::ModelError::chat_line_for`]: the probe has just shown that this message
/// passes on its own, so splitting it is work the user would do for nothing.
pub const SESSION_REFUSED_LINE: &str = "I couldn't answer that, and your message is not the problem: the model provider accepted it on its own, with none of this chat attached. Something this chat carries — an earlier message, or a file on this one — is what it declined. I have replaced this chat's working context, so send that message again; there is nothing to split up.";

/// Written above an answer the fresh context recovered. A recovery that said nothing would hide
/// that the chat is poisoned and that the window the answer came from is not the one the user
/// has been talking to.
pub const SESSION_TRIMMED_NOTICE: &str = "The model provider had declined this chat's recent context, so I answered that on a fresh one. The earlier turns are out of my working window now; ask and I will read them back out of this Project's history.";

/// The message to probe with: the user's own words, clipped, and nothing else — no memory
/// block, no chat recap, no Arbos framing. `None` when there is nothing to ask about, which is
/// a probe that cannot run rather than a verdict.
pub fn probe_message(user_text: &str) -> Option<String> {
    let t = user_text.trim();
    if t.is_empty() {
        return None;
    }
    Some(t.chars().take(PROBE_MESSAGE_CHARS).collect())
}

/// One probe, with the provider call handed in so the verdict can be driven without one.
pub async fn probe_with<F, Fut>(user_text: &str, ask: F) -> Probe
where
    F: FnOnce(String) -> Fut,
    Fut: std::future::Future<Output = Result<Completion>>,
{
    let Some(message) = probe_message(user_text) else {
        return Probe::Unavailable;
    };
    read_probe(ask(message).await)
}

/// What one probe call means. A refusal arrives in three shapes on this lane — the provider's
/// own `refusal` on the message, a moderation `finish_reason`, and an HTTP body the relay proxy
/// passes through — and only the first two are [`Completion`]'s to report, so the error case is
/// classified with the same wording list the reply and compaction lanes share.
///
/// Anything else is `Unavailable`. A 5xx or a timeout says nothing about the content, and
/// reading one as "the message is fine" would rotate a healthy chat's session on a blip.
pub fn read_probe(answered: Result<Completion>) -> Probe {
    match answered {
        Ok(c) if c.refusal_reason().is_some() => Probe::Refused,
        // An empty answer is not an answer. Some providers decline by returning nothing at all,
        // and a blank body is no evidence that the message passed.
        Ok(c) if c.text.trim().is_empty() => Probe::Unavailable,
        Ok(_) => Probe::Answered,
        Err(e) if crate::agent::ModelError::detail_is_refusal(&format!("{e:#}")) => Probe::Refused,
        Err(_) => Probe::Unavailable,
    }
}

/// The cause one probe verdict establishes.
pub fn cause_of(probe: Probe) -> RefusalCause {
    match probe {
        Probe::Answered => RefusalCause::SessionHistory,
        Probe::Refused => RefusalCause::MessageAlone,
        Probe::Unavailable => RefusalCause::Untested,
    }
}

/// What to do about a refusal, given what the probe found.
///
/// `MessageAlone` stands. That is the whole no-laundering guarantee, and it is why the probe
/// runs before any recovery rather than after one: an adversarial ask reaches no retry, no
/// trimmed context and no fallback model, and the line the user reads is the one 0.1.200
/// already gives them.
pub fn recovery_for(cause: RefusalCause) -> RefusalRecovery {
    match cause {
        RefusalCause::SessionHistory => RefusalRecovery::FreshContext,
        RefusalCause::MessageAlone => RefusalRecovery::Stand,
        RefusalCause::Untested => RefusalRecovery::SafeParts,
    }
}
