//! Request attribution shared by the model stub and relay. Authoritative totals live on
//! the relay; this context is captured before a call, never inferred when it finishes.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageContext {
    pub session_id: Option<String>,
    pub session_run_id: Option<String>,
    pub parent_session_id: Option<String>,
    pub chat_id: Option<String>,
    pub turn_id: Option<String>,
    pub job_id: Option<String>,
}

impl UsageContext {
    pub fn for_chat(chat_id: &str) -> Self {
        Self {
            session_id: Some(chat_id.into()),
            chat_id: Some(chat_id.into()),
            ..Self::default()
        }
    }

    pub fn headers(&self) -> [(&'static str, Option<&str>); 6] {
        [
            ("x-arbos-session", self.session_id.as_deref()),
            ("x-arbos-session-run", self.session_run_id.as_deref()),
            ("x-arbos-parent-session", self.parent_session_id.as_deref()),
            ("x-arbos-chat", self.chat_id.as_deref()),
            ("x-arbos-turn", self.turn_id.as_deref()),
            ("x-arbos-job", self.job_id.as_deref()),
        ]
    }
}
