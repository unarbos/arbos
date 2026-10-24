//! Draft-and-approve (design §10, S18): the request/outcome types every feature shares, the
//! `approval` doc-part shape, and the risky-action list as data with one matcher. The box is still
//! the safety boundary (D16); this list only decides which actions get a card before they run.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Pending cards expire after this long (`expiresAt`).
pub const APPROVAL_TTL_MS: i64 = 24 * 60 * 60 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    /// Outgoing communication: email, chat post, calendar invite, GitHub comment/issue/PR, social post.
    Send,
    Destructive,
    Money,
    Credential,
    /// Any command or file read on the user's own computer.
    Computer,
    Other,
}

impl Risk {
    pub fn as_str(self) -> &'static str {
        match self {
            Risk::Send => "send",
            Risk::Destructive => "destructive",
            Risk::Money => "money",
            Risk::Credential => "credential",
            Risk::Computer => "computer",
            Risk::Other => "other",
        }
    }

    pub fn parse(s: &str) -> Risk {
        match s {
            "send" => Risk::Send,
            "destructive" => Risk::Destructive,
            "money" => Risk::Money,
            "credential" => Risk::Credential,
            "computer" => Risk::Computer,
            _ => Risk::Other,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalState {
    Pending,
    Approved,
    Rejected,
    Expired,
}

impl ApprovalState {
    pub fn as_str(self) -> &'static str {
        match self {
            ApprovalState::Pending => "pending",
            ApprovalState::Approved => "approved",
            ApprovalState::Rejected => "rejected",
            ApprovalState::Expired => "expired",
        }
    }
}

/// What the agent wants to do. `payload` is rendered by the card (a draft shows `to`,
/// `subject`, `body`; a command shows `command`) and is handed back to the caller on approval.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub title: String,
    pub summary: String,
    pub risk: Risk,
    #[serde(default)]
    pub payload: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ApprovalOutcome {
    Approved,
    Rejected {
        #[serde(default)]
        reason: Option<String>,
    },
    Expired,
}

impl ApprovalOutcome {
    pub fn state(&self) -> ApprovalState {
        match self {
            ApprovalOutcome::Approved => ApprovalState::Approved,
            ApprovalOutcome::Rejected { .. } => ApprovalState::Rejected,
            ApprovalOutcome::Expired => ApprovalState::Expired,
        }
    }

    pub fn is_approved(&self) -> bool {
        matches!(self, ApprovalOutcome::Approved)
    }
}

/// The dispatch rule every gate shares: a risky action runs only after an `Approved` outcome.
/// `None` (no card was ever decided) is a refusal, never a pass.
pub fn may_dispatch(outcome: Option<&ApprovalOutcome>) -> bool {
    matches!(outcome, Some(ApprovalOutcome::Approved))
}

/// Which part of a tool call a rule inspects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Subject {
    /// The tool name alone.
    ToolName,
    /// The connector action name (`args.action`) or the explicit `args.kind`.
    ConnectorAction,
    /// The shell command text of a `bash`-like tool.
    ShellCommand,
}

/// One entry of the risky-action list.
#[derive(Debug, Clone, Copy)]
pub struct RiskyRule {
    pub class: &'static str,
    pub risk: Risk,
    pub subject: Subject,
    /// Regex tested against the subject (case-insensitive).
    pub pattern: &'static str,
    pub why: &'static str,
}

/// Tools whose names look like sends but are box-internal or cross-project (D22): never gated.
/// `send_message` is agent-to-agent (or an in-group speak): the owner's agents may message each
/// other without a card. Risky work the *receiving* agent then does still raises a card in that
/// agent's chat. External sends stay gated via connector / draft tools.
pub const NOT_RISKY_TOOLS: &[&str] = &[
    "projects_send",
    "group_send",
    "send_message",
    "draft_message",
    "ask_user",
    "route",
    "remember",
    "recall",
    "peek",
    "deliver_file",
    "routines_list",
    "routines_create",
    "routines_update",
    "routines_delete",
];

/// Shell tools whose `command` argument is checked against the destructive patterns.
pub const SHELL_TOOLS: &[&str] = &["bash", "shell", "run_command", "execute_command"];

/// Connector actions of kind `send`/`write` when the caller passes no explicit `kind`.
const CONNECTOR_SEND_ACTIONS: &str = r"^(send|send_\w+|\w+_send|post|post_message|reply|comment|create_event|create_issue|create_pr|create_pull_request|publish|tweet|invite|share|forward)$";

pub const RISKY_RULES: &[RiskyRule] = &[
    // ---- outgoing communications --------------------------------------------------------------
    RiskyRule {
        class: "connector-send",
        risk: Risk::Send,
        subject: Subject::ConnectorAction,
        pattern: CONNECTOR_SEND_ACTIONS,
        why: "leaves the box as a message the user did not write",
    },
    RiskyRule {
        class: "connector-write",
        risk: Risk::Send,
        subject: Subject::ConnectorAction,
        pattern: r"^(write|send|delete|trash|archive|modify|update|move|label)$",
        why: "writes into a user account",
    },
    RiskyRule {
        class: "tool-send",
        risk: Risk::Send,
        subject: Subject::ToolName,
        pattern: r"(^|_)(send|post|publish|tweet|invite|reply)(_|$)",
        why: "outgoing communication",
    },
    // ---- money -----------------------------------------------------------------------------------
    RiskyRule {
        class: "money",
        risk: Risk::Money,
        subject: Subject::ToolName,
        pattern: r"(^|_)(pay|payment|purchase|checkout|transfer|refund|charge|buy|order|wire)(_|$)",
        why: "moves money",
    },
    RiskyRule {
        class: "connector-money",
        risk: Risk::Money,
        subject: Subject::ConnectorAction,
        pattern: r"(^|_)(pay|payment|purchase|checkout|transfer|refund|charge|buy|order|wire)(_|$)",
        why: "moves money",
    },
    // ---- credentials ------------------------------------------------------------------------------
    RiskyRule {
        class: "vault-request",
        risk: Risk::Credential,
        subject: Subject::ToolName,
        pattern: r"^(vault_request|vault_use|browser_fill)$",
        why: "uses a stored credential",
    },
    RiskyRule {
        class: "oauth-grant",
        risk: Risk::Credential,
        subject: Subject::ToolName,
        pattern: r"(oauth|_grant|grant_|token_export|export_token|connector_connect)",
        why: "grants or exports a credential",
    },
    // ---- user computer ----------------------------------------------------------------------------
    RiskyRule {
        class: "local-computer",
        risk: Risk::Computer,
        subject: Subject::ToolName,
        pattern: r"^local_(run|read|list|exec|write)$",
        why: "touches the user's own computer",
    },
    // ---- destructive shell --------------------------------------------------------------------------
    RiskyRule {
        class: "rm-recursive",
        risk: Risk::Destructive,
        subject: Subject::ShellCommand,
        pattern: r"(^|[;&|]\s*|\s)rm\s+(-\w*[rRf]\w*\s+)+",
        why: "recursive or forced delete outside /work/tmp",
    },
    RiskyRule {
        class: "git-force-push",
        risk: Risk::Destructive,
        subject: Subject::ShellCommand,
        pattern: r"git\s+push\b[^;&|]*(\s--force(-with-lease)?\b|\s-f\b|\s\+\S+)",
        why: "rewrites remote history",
    },
    RiskyRule {
        class: "git-branch-delete",
        risk: Risk::Destructive,
        subject: Subject::ShellCommand,
        pattern: r"git\s+(branch\s+(-D|-d|--delete)\b|push\b[^;&|]*(\s--delete\b|\s:\S+))",
        why: "deletes a branch",
    },
    RiskyRule {
        class: "sql-drop",
        risk: Risk::Destructive,
        subject: Subject::ShellCommand,
        pattern: r"\b(DROP\s+(TABLE|DATABASE|SCHEMA|INDEX|VIEW)|TRUNCATE(\s+TABLE)?)\b",
        why: "drops or truncates data",
    },
    RiskyRule {
        class: "cloud-delete",
        risk: Risk::Destructive,
        subject: Subject::ShellCommand,
        pattern: r"\b(aws|gcloud|az|doctl|flyctl|fly|heroku|vercel|terraform|pulumi|daytona|kubectl|helm)\s+[^;&|]*\b(delete|destroy|rm|remove|terminate|purge)\b",
        why: "deletes a cloud resource",
    },
    RiskyRule {
        class: "repo-delete",
        risk: Risk::Destructive,
        subject: Subject::ShellCommand,
        pattern: r"\bgh\s+(repo|release|gist)\s+delete\b",
        why: "deletes something in a user account",
    },
    RiskyRule {
        class: "container-prune",
        risk: Risk::Destructive,
        subject: Subject::ShellCommand,
        pattern: r"\bdocker\s+(system\s+prune|volume\s+(rm|prune)|rmi?)\b",
        why: "removes containers, images or volumes",
    },
    RiskyRule {
        class: "disk-wipe",
        risk: Risk::Destructive,
        subject: Subject::ShellCommand,
        pattern: r"(\bmkfs\b|\bdd\s+[^;&|]*of=/dev/|>\s*/dev/(sd|nvme|disk)|\bshred\b|\bwipefs\b)",
        why: "wipes a disk",
    },
];

/// Why a call needs a card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RiskMatch {
    pub class: &'static str,
    pub risk: Risk,
    pub why: &'static str,
}

fn regex(pattern: &str) -> Option<regex_lite::Regex> {
    regex_lite::RegexBuilder::new(pattern)
        .case_insensitive(true)
        .build()
        .ok()
}

/// Paths an `rm -r` may delete without a card: the box scratch area.
fn rm_targets_are_scratch(command: &str) -> bool {
    let Some(re) = regex(r"(^|[;&|]\s*|\s)rm\s+((-\w+\s+)+)(.*)$") else {
        return false;
    };
    let mut saw_any = false;
    for cap in re.captures_iter(command) {
        let rest = cap.get(4).map(|m| m.as_str()).unwrap_or("");
        for target in rest.split_whitespace() {
            if target.starts_with('-') {
                continue;
            }
            if matches!(target, "&&" | "||" | ";" | "|") {
                break;
            }
            saw_any = true;
            let t = target.trim_matches(|c| c == '"' || c == '\'');
            let scratch = t.starts_with("/work/tmp/")
                || t == "/work/tmp"
                || t.starts_with("tmp/")
                || t.starts_with("./tmp/")
                || t.starts_with("/tmp/");
            if !scratch {
                return false;
            }
        }
    }
    saw_any
}

/// Flag a shell command when it matches a destructive pattern.
pub fn classify_shell(command: &str) -> Option<RiskMatch> {
    for rule in RISKY_RULES {
        if rule.subject != Subject::ShellCommand {
            continue;
        }
        let Some(re) = regex(rule.pattern) else {
            continue;
        };
        if !re.is_match(command) {
            continue;
        }
        if rule.class == "rm-recursive" && rm_targets_are_scratch(command) {
            continue;
        }
        return Some(RiskMatch {
            class: rule.class,
            risk: rule.risk,
            why: rule.why,
        });
    }
    None
}

fn connector_subject(args: &Value) -> Option<String> {
    if let Some(kind) = args.get("kind").and_then(Value::as_str) {
        if kind == "send" || kind == "write" {
            return Some(kind.to_owned());
        }
        if kind == "read" {
            return None;
        }
    }
    args.get("action")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// Flag a tool call (name + JSON args) that is on the risky list. Reads, searches, drafts and
/// box-local edits return `None`.
pub fn classify_tool(tool: &str, args: &Value) -> Option<RiskMatch> {
    if NOT_RISKY_TOOLS.contains(&tool) {
        return None;
    }
    if SHELL_TOOLS.contains(&tool) {
        let cmd = args
            .get("command")
            .or_else(|| args.get("cmd"))
            .and_then(Value::as_str)
            .unwrap_or("");
        return classify_shell(cmd);
    }
    let is_connector = tool == "connector_call";
    let connector_subject = if is_connector {
        connector_subject(args)
    } else {
        None
    };
    for rule in RISKY_RULES {
        let hit = match rule.subject {
            Subject::ToolName => regex(rule.pattern).is_some_and(|re| re.is_match(tool)),
            Subject::ConnectorAction => connector_subject
                .as_deref()
                .is_some_and(|s| regex(rule.pattern).is_some_and(|re| re.is_match(s))),
            Subject::ShellCommand => false,
        };
        if hit {
            return Some(RiskMatch {
                class: rule.class,
                risk: rule.risk,
                why: rule.why,
            });
        }
    }
    None
}

/// Card title + summary for a gated tool call.
pub fn describe_tool(tool: &str, args: &Value, hit: &RiskMatch) -> (String, String) {
    let one_line = |v: &Value| -> String {
        let s = match v {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        let s: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
        if s.chars().count() > 160 {
            format!("{}…", s.chars().take(159).collect::<String>())
        } else {
            s
        }
    };
    if SHELL_TOOLS.contains(&tool) {
        let cmd = args
            .get("command")
            .or_else(|| args.get("cmd"))
            .cloned()
            .unwrap_or(Value::Null);
        return (
            format!("Run a destructive command ({})", hit.class),
            one_line(&cmd),
        );
    }
    if tool == "connector_call" {
        let provider = args.get("provider").and_then(Value::as_str).unwrap_or("?");
        let action = args.get("action").and_then(Value::as_str).unwrap_or("?");
        return (
            format!("{provider}: {action}"),
            one_line(args.get("args").unwrap_or(&Value::Null)),
        );
    }
    (format!("{tool} ({})", hit.class), one_line(args))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Invariant: the matcher flags every listed class and leaves plain reads alone.
    #[test]
    fn matcher_flags_each_risky_class() {
        let send = classify_tool(
            "connector_call",
            &json!({ "provider": "google", "action": "send", "args": {} }),
        )
        .expect("email send");
        assert_eq!(send.risk, Risk::Send);
        assert_eq!(
            classify_tool(
                "connector_call",
                &json!({ "provider": "slack", "action": "post_message" })
            )
            .map(|m| m.risk),
            Some(Risk::Send)
        );
        assert_eq!(
            classify_tool(
                "connector_call",
                &json!({ "provider": "google", "action": "create_event" })
            )
            .map(|m| m.risk),
            Some(Risk::Send)
        );
        assert_eq!(
            classify_tool(
                "connector_call",
                &json!({ "provider": "github", "action": "comment" })
            )
            .map(|m| m.risk),
            Some(Risk::Send)
        );
        assert_eq!(
            classify_tool(
                "connector_call",
                &json!({ "provider": "github", "action": "create_issue", "kind": "write" })
            )
            .map(|m| m.risk),
            Some(Risk::Send)
        );
        assert_eq!(
            classify_tool("social_post", &json!({})).map(|m| m.risk),
            Some(Risk::Send)
        );
        assert_eq!(
            classify_tool("stripe_payment", &json!({})).map(|m| m.risk),
            Some(Risk::Money)
        );
        assert_eq!(
            classify_tool(
                "connector_call",
                &json!({ "provider": "bank", "action": "transfer" })
            )
            .map(|m| m.risk),
            Some(Risk::Money)
        );
        assert_eq!(
            classify_tool("vault_request", &json!({ "name": "github" })).map(|m| m.risk),
            Some(Risk::Credential)
        );
        assert_eq!(
            classify_tool("oauth_grant", &json!({})).map(|m| m.risk),
            Some(Risk::Credential)
        );
        assert_eq!(
            classify_tool("local_run", &json!({ "cmd": "uname -a" })).map(|m| m.risk),
            Some(Risk::Computer)
        );
        assert_eq!(
            classify_tool("local_read", &json!({ "path": "~/notes.md" })).map(|m| m.risk),
            Some(Risk::Computer)
        );

        for cmd in [
            "rm -rf /work/project/src",
            "cd /work && rm -r build",
            "git push --force origin main",
            "git push -f",
            "git push origin +main",
            "git branch -D feature",
            "git push origin --delete feature",
            "psql -c 'DROP TABLE users'",
            "sqlite3 db.sqlite 'TRUNCATE TABLE logs'",
            "aws s3 rb s3://bucket --force && aws ec2 terminate-instances --instance-ids i-1",
            "gcloud compute instances delete vm-1",
            "kubectl delete deployment web",
            "gh repo delete unarbos/arbos --yes",
            "docker system prune -af",
            "dd if=/dev/zero of=/dev/sda",
        ] {
            let hit = classify_tool("bash", &json!({ "command": cmd }));
            assert_eq!(
                hit.as_ref().map(|m| m.risk),
                Some(Risk::Destructive),
                "{cmd} should be destructive"
            );
        }
    }

    #[test]
    fn matcher_ignores_reads_drafts_and_scratch() {
        for (tool, args) in [
            ("bash", json!({ "command": "ls -la" })),
            ("bash", json!({ "command": "cat README.md | head" })),
            ("bash", json!({ "command": "rm -rf /work/tmp/build" })),
            ("bash", json!({ "command": "rm -rf tmp/cache" })),
            ("bash", json!({ "command": "git push origin main" })),
            ("bash", json!({ "command": "cargo test --workspace" })),
            ("read", json!({ "path": "src/main.rs" })),
            ("edit", json!({ "path": "src/main.rs" })),
            ("write", json!({ "path": "/work/out/report.md" })),
            ("history_search", json!({ "query": "rm -rf" })),
            (
                "connector_call",
                json!({ "provider": "google", "action": "read_message" }),
            ),
            (
                "connector_call",
                json!({ "provider": "google", "action": "search", "kind": "read" }),
            ),
            (
                "connector_call",
                json!({ "provider": "google", "action": "create_draft" }),
            ),
            (
                "draft_message",
                json!({ "channel": "email", "to": "a@b.c" }),
            ),
            ("projects_send", json!({ "to": "Comms", "text": "hi" })),
            ("send_message", json!({ "to": "TestAgent", "text": "hi" })),
            ("send_message", json!({ "text": "still working" })),
            ("local_devices", json!({})),
        ] {
            assert_eq!(
                classify_tool(tool, &args),
                None,
                "{tool} {args} is not risky"
            );
        }
    }

    /// Invariant: a `send`-kind action with no `approved` outcome is never dispatched.
    #[test]
    fn send_without_approval_never_dispatches() {
        assert!(!may_dispatch(None));
        assert!(!may_dispatch(Some(&ApprovalOutcome::Rejected {
            reason: Some("no".into())
        })));
        assert!(!may_dispatch(Some(&ApprovalOutcome::Expired)));
        assert!(may_dispatch(Some(&ApprovalOutcome::Approved)));
    }
}
