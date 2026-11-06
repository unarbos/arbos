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
    // Posts a chart into the user's own chat like `deliver_file`; its name matches the
    // `tool-send` pattern (`post_`) and raised a card on the first live use
    // ([feedback chart-post-not-gated-2026-10-27]).
    "post_chart",
    // Agent-hosted sites (#2060): publishing the user's own static site to their own address
    // is the thing they asked for, like `deliver_file`; `publish_` matches the `tool-send`
    // pattern and raised a card on the first live use (the box's MCP client then timed out).
    "publish_site",
    "unpublish_site",
    "list_sites",
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
    // File deletion is not a regex rule: `delete_targets` below reads the command (rm, find
    // -delete, shred, truncate, git clean, rsync --delete, …) and classifies the paths it
    // touches (#1544, #1712). Its class is `FILE_DELETE`.
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

/// Why a call needs a card. `path_class` is set for shell rules whose targets are paths
/// (`rm-recursive`): the class a standing grant must cover to skip the card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RiskMatch {
    pub class: &'static str,
    pub risk: Risk,
    pub why: &'static str,
    pub path_class: Option<PathClass>,
}

/// Where a shell command's filesystem targets live, widest last. A standing grant for a class
/// covers every narrower class (#1544: "always allow in /tmp" never covers the workspace).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathClass {
    /// `/tmp` and `/work/tmp`: the box scratch area.
    Scratch,
    /// The agent's own workspace: the project folder it is given and everything under it
    /// (`/work/<project>`). Another project's folder is not this — it is `Anywhere`.
    Workspace,
    /// Anything else on the box.
    Anywhere,
}

/// The root every workspace sits under (`/work/<project>`), and the default a caller that names
/// no project gets.
pub const WORKSPACE_ROOT: &str = "/work";
/// Scratch roots: deletes under these never raise a card.
pub const SCRATCH_ROOTS: &[&str] = &["/tmp", "/work/tmp"];

impl PathClass {
    pub fn as_str(self) -> &'static str {
        match self {
            PathClass::Scratch => "scratch",
            PathClass::Workspace => "workspace",
            PathClass::Anywhere => "anywhere",
        }
    }

    pub fn parse(s: &str) -> Option<PathClass> {
        match s {
            "scratch" => Some(PathClass::Scratch),
            "workspace" => Some(PathClass::Workspace),
            "anywhere" => Some(PathClass::Anywhere),
            _ => None,
        }
    }

    /// Short label for the card option and the Settings row ("Always allow Doug in /tmp").
    pub fn label(self) -> &'static str {
        match self {
            PathClass::Scratch => "/tmp",
            PathClass::Workspace => "its workspace",
            PathClass::Anywhere => "anywhere",
        }
    }

    /// One plain line for the option hint / Settings subtitle.
    pub fn describe(self) -> &'static str {
        match self {
            PathClass::Scratch => "scratch only: /tmp and /work/tmp",
            PathClass::Workspace => "the agent's own /work folder, including scratch",
            PathClass::Anywhere => "any folder of its box",
        }
    }

    /// A grant for `self` lets a command whose targets fall in `other` run without a card.
    pub fn covers(self, other: PathClass) -> bool {
        self >= other
    }
}

/// One standing shell grant as the relay stores it (`shell_grants`): "<agent> may run
/// `rule_class` commands whose targets are within `path_class` without a card", optionally
/// only in one chat. Revoked rows are not listed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellGrant {
    pub id: String,
    pub project_id: String,
    pub rule_class: String,
    pub path_class: PathClass,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_id: Option<String>,
    pub granted_by: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_id: Option<String>,
    pub created_at: i64,
}

impl ShellGrant {
    /// Whether this grant lets `hit` (raised in `chat_id`) run without a card.
    pub fn covers(&self, hit: &RiskMatch, chat_id: Option<&str>) -> bool {
        let Some(class) = hit.path_class else {
            return false;
        };
        self.rule_class == hit.class
            && self.path_class.covers(class)
            && self.chat_id.as_deref().is_none_or(|c| Some(c) == chat_id)
    }
}

/// The deletion family's rule class: `rm`, `find … -delete` / `-exec rm`, `xargs rm`, `shred`,
/// `unlink`, `truncate`, the bare `> file` truncation idioms, `git clean -f`, `git checkout --` /
/// `git restore` / `git reset --hard`, `rsync --delete`, `mv … /dev/null`, Python's
/// `os.remove` / `shutil.rmtree`. One class, so one standing grant covers the whole family
/// (#1712: `find "$D" -type f -delete` emptied a folder with no card possible).
pub const FILE_DELETE: &str = "file-delete";
const FILE_DELETE_WHY: &str = "deletes files outside the agent's own workspace";

/// Rule classes a standing grant may cover: shell rules whose risk is in filesystem paths.
pub const GRANTABLE_CLASSES: &[&str] = &[FILE_DELETE];

fn regex(pattern: &str) -> Option<regex_lite::Regex> {
    regex_lite::RegexBuilder::new(pattern)
        .case_insensitive(true)
        .build()
        .ok()
}

/// `cwd` + `target` as a normalized absolute path (`.`/`..` folded). `~` and `$HOME` resolve
/// nowhere we classify, so they come back as-is and classify as `Anywhere`. Public so a caller
/// that records paths for `PathFacts::created_here` spells them the way the matcher reads them.
pub fn resolve_path(cwd: &str, target: &str) -> String {
    let t = target.trim_matches(|c| c == '"' || c == '\'');
    if t.starts_with('~') || t.starts_with('$') {
        return t.to_owned();
    }
    let joined = if t.starts_with('/') {
        t.to_owned()
    } else if cwd.starts_with('/') {
        format!("{cwd}/{t}")
    } else {
        return format!("{cwd}/{t}");
    };
    let mut out: Vec<&str> = Vec::new();
    for seg in joined.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            s => out.push(s),
        }
    }
    format!("/{}", out.join("/"))
}

fn under(path: &str, root: &str) -> bool {
    path == root || path.starts_with(&format!("{root}/"))
}

/// Class of one absolute path, against the workspace the caller owns. A glob keeps the class of
/// its fixed prefix.
///
/// `workspace_root` is the asking agent's own project folder (`/work/<project>`). Only paths
/// under it are `Workspace`; `/work` itself, a sibling project's folder and the box's own
/// `/work/.arbos` are all `Anywhere`, because none of them belongs to this agent.
pub fn classify_path(path: &str, workspace_root: &str) -> PathClass {
    if !path.starts_with('/') {
        return PathClass::Anywhere;
    }
    if SCRATCH_ROOTS.iter().any(|r| under(path, r)) {
        return PathClass::Scratch;
    }
    if under(path, workspace_root) {
        return PathClass::Workspace;
    }
    PathClass::Anywhere
}

/// Split a command line into simple commands at `;`, `&&`, `||`, `|` and newlines, keeping
/// order so a leading `cd` applies to what follows.
fn simple_commands(command: &str) -> Vec<Vec<String>> {
    let mut cmds = Vec::new();
    let mut cur: Vec<String> = Vec::new();
    let mut word = String::new();
    let mut quote: Option<char> = None;
    let mut chars = command.chars().peekable();
    let flush_word = |word: &mut String, cur: &mut Vec<String>| {
        if !word.is_empty() {
            cur.push(std::mem::take(word));
        }
    };
    while let Some(c) = chars.next() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                } else {
                    word.push(c);
                }
            }
            None => match c {
                '\'' | '"' => quote = Some(c),
                ';' | '\n' | '|' | '&' => {
                    if (c == '|' || c == '&') && chars.peek() == Some(&c) {
                        chars.next();
                    }
                    flush_word(&mut word, &mut cur);
                    if !cur.is_empty() {
                        cmds.push(std::mem::take(&mut cur));
                    }
                }
                c if c.is_whitespace() => flush_word(&mut word, &mut cur),
                c => word.push(c),
            },
        }
    }
    flush_word(&mut word, &mut cur);
    if !cur.is_empty() {
        cmds.push(cur);
    }
    cmds
}

/// Flags of `git clean` that make it delete (`-f`/`--force`, with `-d`/`-x`/`-X` riding along).
fn git_clean_deletes(words: &[&str]) -> bool {
    words.iter().any(|w| {
        *w == "--force" || (w.starts_with('-') && !w.starts_with("--") && w[1..].contains('f'))
    })
}

/// Skip wrapper words (`sudo`, `env VAR=x`, `time`, …) to the real command.
fn unwrap_head<'a>(
    words: &mut std::iter::Peekable<impl Iterator<Item = &'a str>>,
) -> Option<&'a str> {
    let mut head = words.next()?;
    loop {
        match head {
            "sudo" | "time" | "nice" | "nohup" | "command" | "builtin" => head = words.next()?,
            "env" => {
                head = words.next()?;
                while head.contains('=') && !head.starts_with('-') {
                    head = words.next()?;
                }
            }
            _ if head.contains('=') && !head.starts_with('/') && !head.starts_with('.') => {
                // `VAR=x cmd …`
                head = words.next()?;
            }
            _ => return Some(head),
        }
    }
}

/// The target of the redirect `words[i]` opens — `> f`, `>> f`, `2>f`, `&>> f` — or `None` when
/// that word is no redirect. `2>&1` and `/dev/*` name no file to keep.
fn redirect_at<'a>(words: &[&'a str], i: usize) -> Option<&'a str> {
    let w = *words.get(i)?;
    let gt = w.find('>')?;
    let (fd, rest) = w.split_at(gt);
    if !(fd.is_empty() || fd == "&" || fd.chars().all(|c| c.is_ascii_digit())) {
        return None;
    }
    let rest = rest.trim_start_matches('>');
    let target = if rest.is_empty() {
        *words.get(i + 1)?
    } else {
        rest
    };
    (!target.is_empty() && !target.starts_with('&') && !target.starts_with("/dev/"))
        .then_some(target)
}

/// The paths `command` writes by redirect, resolved from `cwd` with a leading `cd` / `pushd`
/// honoured. No deletion rule reads these: they are for a caller that records which files a run
/// produced, so it can tell its own outputs from what it found (`PathFacts::created_here`).
pub fn redirect_targets(cwd: &str, command: &str) -> Vec<String> {
    let mut cwd = cwd.to_owned();
    let mut out = Vec::new();
    for cmd in simple_commands(command) {
        let words: Vec<&str> = cmd.iter().map(String::as_str).collect();
        if matches!(words.first(), Some(&"cd") | Some(&"pushd")) {
            if let Some(dir) = words.iter().skip(1).find(|w| !w.starts_with('-')) {
                cwd = resolve_path(&cwd, dir);
            }
            continue;
        }
        for i in 0..words.len() {
            if let Some(t) = redirect_at(&words, i) {
                out.push(resolve_path(&cwd, t));
            }
        }
    }
    out
}

/// The paths a `find` walks: every argument before the first expression word (`-name`, `!`,
/// `(`, …). None given means `.`.
fn find_paths<'a>(words: &[&'a str]) -> Vec<&'a str> {
    let mut paths = Vec::new();
    let mut i = 0;
    // GNU find's leading options take no path.
    while i < words.len() && matches!(words[i], "-H" | "-L" | "-P" | "-O0" | "-O1" | "-O2" | "-O3")
    {
        i += 1;
    }
    while i < words.len() {
        let w = words[i];
        if w.starts_with('-') || matches!(w, "!" | "(" | ")" | ",") {
            break;
        }
        paths.push(w);
        i += 1;
    }
    if paths.is_empty() {
        paths.push(".");
    }
    paths
}

fn find_deletes(words: &[&str]) -> bool {
    words.iter().enumerate().any(|(i, w)| {
        *w == "-delete"
            || (matches!(*w, "-exec" | "-execdir" | "-ok" | "-okdir")
                && words
                    .get(i + 1)
                    .is_some_and(|c| matches!(*c, "rm" | "shred" | "unlink" | "truncate")))
    })
}

/// The words of a redirect `> path` (not `>>`, not `/dev/null`) in a simple command whose only
/// purpose is to empty the file: `> f`, `: > f`, `true > f`, `cat /dev/null > f`,
/// `echo -n "" > f`, `printf "" > f`. A real write (`echo text > f`) is not a delete.
fn bare_truncate_target<'a>(words: &[&'a str]) -> Option<&'a str> {
    let gt = words
        .iter()
        .position(|w| *w == ">" || w.starts_with('>') && !w.starts_with(">>") && w.len() > 1)?;
    let target = if words[gt] == ">" {
        *words.get(gt + 1)?
    } else {
        &words[gt][1..]
    };
    if target == "/dev/null" || target.starts_with("/dev/") {
        return None;
    }
    let before: Vec<&str> = words[..gt].to_vec();
    let bare = match before.as_slice() {
        [] | [":"] | ["true"] => true,
        ["cat", "/dev/null"] => true,
        ["echo", rest @ ..] | ["printf", rest @ ..] => rest
            .iter()
            .all(|w| w.starts_with('-') || w.is_empty() || *w == "\"\"" || *w == "''"),
        ["truncate", ..] => false,
        _ => false,
    };
    bare.then_some(target)
}

/// The widest path class among everything `command` deletes — `rm` and the rest of the
/// deletion family (`FILE_DELETE`), with a leading `cd`/`pushd` moving the working directory
/// (default `/work`). `None` when the command deletes nothing. A `find … -delete` deletes
/// under the paths it walks; `xargs rm` deletes what the earlier `find` in the pipeline found,
/// else whatever the working directory holds; `git clean`/`checkout --`/`reset --hard` and
/// `rsync --delete` act on the repository / destination; `mv … /dev/null` on its sources;
/// Python `os.remove`/`shutil.rmtree` cannot be read statically and count as anywhere.
pub fn delete_targets(command: &str) -> Option<PathClass> {
    delete_targets_with(command, PathFacts::assumed())
}

/// One yes/no question about one absolute path, answered by the box. agentd passes a real
/// look at the box; the pure forms pass a constant (`PathFacts::assumed`).
pub type FileHasContent<'a> = &'a dyn Fn(&str) -> bool;

/// What the box knows about the paths a command names. Both answers are about one absolute,
/// normalized path (`resolve_path`), and both default to the cautious side — a card too many,
/// never one too few:
///
/// - `has_content`: an existing, non-empty file? Only the truncation idioms ask. `: > f` on a
///   file that does not exist, or is empty, creates it and deletes nothing (#1719).
/// - `created_here`: did the job making this call create this exact path itself? A job deleting
///   a file it wrote in this same run destroys nothing the user had, so that target raises no
///   card. Exact paths only: no parent, prefix or glob ever matches, so a job that made
///   `/work/out/report.md` still gets a card for `rm -rf /work/out`.
///
/// `workspace_root` is not a fact about one path but the frame the others are read in: the
/// asking agent's own project folder, which is both where a relative target resolves and the
/// boundary of what it may delete uncarded.
#[derive(Clone, Copy)]
pub struct PathFacts<'a> {
    pub has_content: FileHasContent<'a>,
    pub created_here: FileHasContent<'a>,
    pub workspace_root: &'a str,
}

fn path_exists(_: &str) -> bool {
    true
}

fn created_by_nobody(_: &str) -> bool {
    false
}

impl PathFacts<'static> {
    /// No box to ask: every path exists with content and nothing was created by the caller.
    pub fn assumed() -> Self {
        PathFacts {
            has_content: &path_exists,
            created_here: &created_by_nobody,
            workspace_root: WORKSPACE_ROOT,
        }
    }
}

impl<'a> PathFacts<'a> {
    /// A real `stat` for #1719, with no caller's own writes exempt — the view every gate that
    /// cannot name one job uses.
    pub fn with_content(has_content: FileHasContent<'a>) -> Self {
        PathFacts {
            has_content,
            created_here: &created_by_nobody,
            workspace_root: WORKSPACE_ROOT,
        }
    }

    /// Read these paths as the agent whose workspace is `workspace_root` (`/work/<project>`).
    ///
    /// A caller that does not set this is treated as owning the whole of `/work`, which is the
    /// pure matcher's position: it has no project to key on. Every gate that decides a real
    /// card does know the project, and says so here.
    pub fn in_workspace(self, workspace_root: &'a str) -> Self {
        PathFacts {
            workspace_root,
            ..self
        }
    }
}

/// Words of a non-existence guard in the same command: `[ ! -e f ]`, `[ ! -f f ]`,
/// `[[ ! -e f ]]`, `test ! -e f`, `test ! -f f` — the path that must be absent.
fn absence_guard<'a>(words: &[&'a str]) -> Option<&'a str> {
    let head = *words.first()?;
    if !matches!(head, "[" | "[[" | "test") {
        return None;
    }
    let bang = words.iter().position(|w| *w == "!")?;
    let op = *words.get(bang + 1)?;
    if !matches!(op, "-e" | "-f" | "-s" | "-a") {
        return None;
    }
    words.get(bang + 2).copied()
}

/// The parent directory of `path` (`/a/b/c.txt` → `/a/b`).
fn parent_dir(path: &str) -> &str {
    match path.rfind('/') {
        Some(0) => "/",
        Some(i) => &path[..i],
        None => "",
    }
}

/// `delete_targets` with a real view of the box: truncation idioms count as deletes only when
/// the file exists with content, is not guarded by `[ ! -e f ]` / `test ! -e f` in the same
/// command, and its parent is not created by a `mkdir -p` in the same command. A named target
/// the caller created itself (`PathFacts::created_here`) contributes no class; the widest of
/// what is left still decides, so one pre-existing target keeps the card.
pub fn delete_targets_with(command: &str, facts: PathFacts<'_>) -> Option<PathClass> {
    // The shell the agent is given runs in its own project folder, so that is where a relative
    // target resolves. Reading it against `/work` instead put `rm d*.html.bak` a directory
    // above the files it names.
    let mut cwd = facts.workspace_root.to_owned();
    let classify = |p: &str| classify_path(p, facts.workspace_root);
    let mut widest: Option<PathClass> = None;
    let mut pipeline_paths: Vec<String> = Vec::new();
    // Truncation idioms, decided at the end: (resolved path, raw word).
    let mut truncations: Vec<(String, String)> = Vec::new();
    let mut guarded_absent: Vec<String> = Vec::new();
    let mut made_dirs: Vec<String> = Vec::new();
    let note = |widest: &mut Option<PathClass>, class: PathClass| {
        *widest = Some(widest.map_or(class, |c| c.max(class)));
    };
    let plain_args = |words: &[&str]| -> Vec<String> {
        let mut out = Vec::new();
        let mut dashdash = false;
        for w in words {
            if !dashdash && *w == "--" {
                dashdash = true;
                continue;
            }
            if !dashdash && w.starts_with('-') {
                continue;
            }
            out.push((*w).to_owned());
        }
        out
    };
    if command.contains("shutil.rmtree(")
        || command.contains("os.remove(")
        || command.contains("os.unlink(")
        || command.contains(".unlink(")
        || command.contains("os.rmdir(")
    {
        note(&mut widest, PathClass::Anywhere);
    }
    for cmd in simple_commands(command) {
        let mut words = cmd.iter().map(String::as_str).peekable();
        let Some(head) = unwrap_head(&mut words) else {
            continue;
        };
        let rest: Vec<&str> = words.collect();
        let classify_all = |widest: &mut Option<PathClass>, targets: &[String]| {
            for t in targets {
                let path = resolve_path(&cwd, t);
                if (facts.created_here)(&path) {
                    continue;
                }
                note(widest, classify(&path));
            }
        };
        match head {
            "cd" => {
                cwd = match rest.iter().find(|w| !w.starts_with('-')) {
                    Some(dir) => resolve_path(&cwd, dir),
                    None => "~".to_owned(),
                };
            }
            "pushd" => {
                if let Some(dir) = rest.iter().find(|w| !w.starts_with('-')) {
                    cwd = resolve_path(&cwd, dir);
                }
            }
            "rm" | "shred" | "unlink" | "truncate" => {
                // Flags that take a value (`truncate -s 0`, `shred -n 3`): the value is no path.
                let valued: &[&str] = match head {
                    "truncate" => &["-s", "--size", "-r", "--reference"],
                    "shred" => &["-n", "--iterations", "-s", "--size", "--random-source"],
                    _ => &[],
                };
                let mut targets = Vec::new();
                let mut skip = false;
                let mut dashdash = false;
                for w in &rest {
                    if skip {
                        skip = false;
                        continue;
                    }
                    if !dashdash && *w == "--" {
                        dashdash = true;
                        continue;
                    }
                    if !dashdash && valued.contains(w) {
                        skip = true;
                        continue;
                    }
                    if !dashdash && w.starts_with('-') {
                        continue;
                    }
                    targets.push((*w).to_owned());
                }
                if targets.is_empty() {
                    // `rm` fed by a pipe / xargs / glob we cannot see.
                    note(&mut widest, classify(&cwd));
                } else {
                    classify_all(&mut widest, &targets);
                }
            }
            "find" => {
                let paths: Vec<String> =
                    find_paths(&rest).iter().map(|p| (*p).to_owned()).collect();
                pipeline_paths = paths.iter().map(|p| resolve_path(&cwd, p)).collect();
                if find_deletes(&rest) {
                    classify_all(&mut widest, &paths);
                }
            }
            "xargs" | "parallel" => {
                let deletes = rest
                    .iter()
                    .any(|w| matches!(*w, "rm" | "shred" | "unlink" | "truncate"));
                if deletes {
                    if pipeline_paths.is_empty() {
                        note(&mut widest, classify(&cwd));
                    } else {
                        for p in &pipeline_paths {
                            note(&mut widest, classify(p));
                        }
                    }
                }
            }
            "git" => {
                // The subcommand: the first plain word, skipping `-C <dir>` / `-c <k=v>` values.
                let mut sub = "";
                let mut skip = false;
                for w in &rest {
                    if skip {
                        skip = false;
                        continue;
                    }
                    if *w == "-C" || *w == "-c" {
                        skip = true;
                        continue;
                    }
                    if !w.starts_with('-') {
                        sub = w;
                        break;
                    }
                }
                let after_sub: Vec<&str> = rest
                    .iter()
                    .skip_while(|w| **w != sub)
                    .skip(1)
                    .copied()
                    .collect();
                let repo_dir = rest
                    .iter()
                    .position(|w| *w == "-C")
                    .and_then(|i| rest.get(i + 1))
                    .map(|d| resolve_path(&cwd, d))
                    .unwrap_or_else(|| cwd.clone());
                let deletes = match sub {
                    "clean" => git_clean_deletes(&after_sub),
                    "checkout" => after_sub
                        .iter()
                        .any(|w| *w == "--" || *w == "." || *w == "-f" || *w == "--force"),
                    "restore" => {
                        !after_sub.iter().any(|w| *w == "--staged" || *w == "-S")
                            || after_sub.iter().any(|w| *w == "--worktree" || *w == "-W")
                    }
                    "reset" => after_sub.iter().any(|w| *w == "--hard" || *w == "--merge"),
                    "stash" => after_sub.iter().any(|w| *w == "drop" || *w == "clear"),
                    _ => false,
                };
                if deletes {
                    let paths: Vec<String> = if matches!(sub, "checkout" | "restore") {
                        plain_args(&after_sub)
                    } else {
                        Vec::new()
                    };
                    if paths.is_empty() {
                        note(&mut widest, classify(&repo_dir));
                    } else {
                        for p in &paths {
                            note(&mut widest, classify(&resolve_path(&repo_dir, p)));
                        }
                    }
                }
            }
            "rsync" => {
                if rest.iter().any(|w| w.starts_with("--delete")) {
                    if let Some(dest) = plain_args(&rest).last() {
                        if !dest.contains(':') {
                            note(&mut widest, classify(&resolve_path(&cwd, dest)));
                        } else {
                            note(&mut widest, PathClass::Anywhere);
                        }
                    }
                }
            }
            "mv" => {
                let args = plain_args(&rest);
                if args.last().is_some_and(|d| d == "/dev/null") {
                    classify_all(&mut widest, &args[..args.len() - 1]);
                }
            }
            "cp" => {
                // `cp /dev/null f` empties f — a truncation idiom, decided below.
                let args = plain_args(&rest);
                if args.first().is_some_and(|s| s == "/dev/null") {
                    for t in &args[1..] {
                        truncations.push((resolve_path(&cwd, t), t.clone()));
                    }
                }
            }
            "mkdir" => {
                for d in plain_args(&rest) {
                    made_dirs.push(resolve_path(&cwd, &d));
                }
            }
            "[" | "[[" | "test" => {
                let mut all: Vec<&str> = vec![head];
                all.extend(rest.iter().copied());
                if let Some(g) = absence_guard(&all) {
                    guarded_absent.push(resolve_path(&cwd, g));
                }
            }
            _ => {}
        }
        if head != "cd" && head != "pushd" {
            let mut all: Vec<&str> = vec![head];
            all.extend(rest.iter().copied());
            if let Some(t) = bare_truncate_target(&all) {
                truncations.push((resolve_path(&cwd, t), t.to_owned()));
            }
        }
        if head != "find" {
            pipeline_paths.clear();
        }
    }
    for (path, _raw) in truncations {
        if guarded_absent.contains(&path) {
            continue;
        }
        if made_dirs.iter().any(|d| *d == parent_dir(&path)) {
            continue;
        }
        if (facts.created_here)(&path) {
            continue;
        }
        if !(facts.has_content)(&path) {
            continue;
        }
        note(&mut widest, classify(&path));
    }
    widest
}

/// Compatibility shim for callers that only ask "does it `rm`?": the class of what the command
/// deletes, and whether an `rm` in it is recursive.
pub fn rm_targets(command: &str) -> Option<(PathClass, bool)> {
    let class = delete_targets(command)?;
    let recursive = simple_commands(command).iter().any(|cmd| {
        cmd.first().is_some_and(|h| h == "rm")
            && cmd.iter().skip(1).any(|w| {
                w == "--recursive"
                    || (w.starts_with('-') && !w.starts_with("--") && w[1..].contains(['r', 'R']))
            })
    });
    Some((class, recursive))
}

/// Flag a shell command when it matches a destructive pattern. Anything that deletes files
/// (`delete_targets`) is a card when a target sits outside the agent's own workspace; the card
/// carries the path class so a standing grant can cover it (#1544, #1712). The regex rules
/// (force-push, DROP TABLE, cloud deletes, …) follow.
pub fn classify_shell(command: &str) -> Option<RiskMatch> {
    classify_shell_with(command, PathFacts::assumed())
}

/// `classify_shell` with the box's view of the paths it names (#1719, `PathFacts`).
///
/// An agent deleting files in its own `/work/<project>` folder raises no card. That folder is
/// the agent's to work in: a scratch-only rule made the common case — clearing its own build
/// output, its own `.bak` files — ask the user about work they had already asked for, and a
/// card nobody answers is a stall rather than a safeguard. The rule stays strict for every path
/// the agent does not own: another project's folder, the box's own `/work/.arbos`, and the
/// system paths outside `/work` altogether.
pub fn classify_shell_with(command: &str, facts: PathFacts<'_>) -> Option<RiskMatch> {
    if let Some(class) = delete_targets_with(command, facts) {
        if class == PathClass::Anywhere {
            return Some(RiskMatch {
                class: FILE_DELETE,
                risk: Risk::Destructive,
                why: FILE_DELETE_WHY,
                path_class: Some(class),
            });
        }
    }
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
        return Some(RiskMatch {
            class: rule.class,
            risk: rule.risk,
            why: rule.why,
            path_class: None,
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
    classify_tool_with(tool, args, PathFacts::assumed())
}

/// `classify_tool` with the box's view of the paths it names (`PathFacts`): agentd, which runs
/// on the box, passes a `stat` and the list of paths the calling job wrote itself.
pub fn classify_tool_with(tool: &str, args: &Value, facts: PathFacts<'_>) -> Option<RiskMatch> {
    if NOT_RISKY_TOOLS.contains(&tool) {
        return None;
    }
    if SHELL_TOOLS.contains(&tool) {
        let cmd = args
            .get("command")
            .or_else(|| args.get("cmd"))
            .and_then(Value::as_str)
            .unwrap_or("");
        return classify_shell_with(cmd, facts);
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
                path_class: None,
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
            "rm -rf /srv/project/src",
            "cd /srv && rm -r build",
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
            (
                "bash",
                json!({ "command": "find /tmp/cache -type f -delete" }),
            ),
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
            ("deliver_file", json!({ "path": "out/report.pdf" })),
            (
                "post_chart",
                json!({ "chart": { "kind": "line", "series": [] }, "caption": "x" }),
            ),
            ("publish_site", json!({ "app": "pura-vida-surf" })),
            ("unpublish_site", json!({ "app": "pura-vida-surf" })),
        ] {
            assert_eq!(
                classify_tool(tool, &args),
                None,
                "{tool} {args} is not risky"
            );
        }
    }

    /// #1544 / #1712: every way a shell command deletes files is classified by where the
    /// targets are — scratch, the agent's own workspace, or anywhere — with a leading `cd`
    /// moving the working directory. One table, one expected class per command.
    ///
    /// The table's agent owns the whole of `/work`, which is what the pure matcher assumes when
    /// no project is named; `own_workspace_deletes_pass_and_other_projects_still_card` is the
    /// same question asked by an agent that owns one folder in it.
    #[test]
    fn deletion_family_is_classified_by_target_path() {
        use PathClass::*;
        let table: &[(&str, Option<PathClass>)] = &[
            // rm — scratch is free, the rest cards (non-recursive too: #1712's files were plain).
            (
                "cd /tmp && rm -rf arbos-src && git clone --depth 1 https://x/y arbos-src",
                Some(Scratch),
            ),
            ("cd /work/tmp; rm -r build", Some(Scratch)),
            ("pushd /tmp && rm -rf ./cache", Some(Scratch)),
            ("rm -rf /work/tmp/build /tmp/x", Some(Scratch)),
            ("rm -- -weird-file", Some(Workspace)),
            ("rm -f out/*.png", Some(Workspace)),
            ("rm -rf build", Some(Workspace)),
            ("cd /work/app && rm -rf node_modules", Some(Workspace)),
            ("cd /tmp && rm -rf ../etc/foo", Some(Anywhere)),
            ("rm -rf ~/Library", Some(Anywhere)),
            ("rm -rf /tmp/a /work/b", Some(Workspace)),
            ("rm -f /etc/hosts", Some(Anywhere)),
            ("sudo rm -rf /var/log/old", Some(Anywhere)),
            ("FOO=1 env BAR=2 rm -f /work/x", Some(Workspace)),
            // find -delete / -exec rm: the paths find walks.
            (
                "cd /work/p && find \"$D\" -mindepth 1 -maxdepth 1 -type f -delete",
                Some(Anywhere),
            ),
            (
                "find /work/p/old -mindepth 1 -type f -name 'file*.txt' -delete",
                Some(Workspace),
            ),
            ("find . -name '*.pyc' -delete", Some(Workspace)),
            ("find /tmp/build -type f -delete", Some(Scratch)),
            (
                "find /work/p -name '*.log' -exec rm {} \\;",
                Some(Workspace),
            ),
            (
                "find /work/p -name '*.log' -execdir rm -f {} +",
                Some(Workspace),
            ),
            (
                "find /work/p -name '*.log' -print0 | xargs -0 rm -f",
                Some(Workspace),
            ),
            ("find /tmp/p -name '*.log' | xargs rm", Some(Scratch)),
            ("ls *.bak | xargs rm -f", Some(Workspace)),
            ("find /work/p -name '*.log'", None),
            // shred / unlink / truncate / bare truncation idioms.
            ("shred -u /work/p/secret.key", Some(Workspace)),
            ("shred -n 3 -z /tmp/dump.bin", Some(Scratch)),
            ("unlink /work/p/link", Some(Workspace)),
            ("truncate -s 0 /work/p/app.log", Some(Workspace)),
            ("truncate -s 0 /tmp/app.log", Some(Scratch)),
            ("> /work/p/app.log", Some(Workspace)),
            (": > /work/p/app.log", Some(Workspace)),
            ("true > /etc/motd", Some(Anywhere)),
            ("cat /dev/null > /work/p/app.log", Some(Workspace)),
            ("cp /dev/null /work/p/app.log", Some(Workspace)),
            ("echo -n '' > /work/p/app.log", Some(Workspace)),
            ("echo hello > /work/p/notes.txt", None),
            // #1719: create-an-empty-file idioms are not deletes — guarded by `[ ! -e f ]`,
            // or the parent made by `mkdir -p` in the same command.
            ("cd /work/p && D=lt-b8 && mkdir -p $D && ls -A $D && [ ! -e $D/count.txt ] && : > $D/count.txt && for i in $(seq 1 40); do echo $i >> $D/count.txt; sleep 3; done", None),
            ("[ ! -e /work/p/app.log ] && : > /work/p/app.log", None),
            ("test ! -e /work/p/app.log && > /work/p/app.log", None),
            ("[[ ! -f /work/p/app.log ]] && true > /work/p/app.log", None),
            ("mkdir -p /work/p/out && : > /work/p/out/run.log", None),
            ("mkdir -p out && cp /dev/null out/run.log", None),
            ("cargo build > /work/tmp/build.log 2>&1", None),
            ("ls > /dev/null", None),
            // git: clean -f, checkout -- / ., restore, reset --hard, stash drop.
            ("git clean -fdx", Some(Workspace)),
            ("git clean -n", None),
            ("git -C /opt/repo clean -f", Some(Anywhere)),
            ("git checkout -- src/main.rs", Some(Workspace)),
            ("git checkout .", Some(Workspace)),
            ("git checkout feature-branch", None),
            ("git restore src/", Some(Workspace)),
            ("git restore --staged src/", None),
            ("git reset --hard HEAD~1", Some(Workspace)),
            ("git reset --soft HEAD~1", None),
            ("git stash drop", Some(Workspace)),
            ("cd /tmp/scratch-repo && git reset --hard", Some(Scratch)),
            // rsync --delete: the destination.
            ("rsync -a --delete src/ /work/p/site/", Some(Workspace)),
            ("rsync -a --delete-after src/ /srv/www/", Some(Anywhere)),
            ("rsync -a --delete src/ host:/srv/www/", Some(Anywhere)),
            ("rsync -a src/ /work/p/site/", None),
            // mv to /dev/null: the sources.
            ("mv /work/p/old.txt /dev/null", Some(Workspace)),
            ("mv /work/p/a.txt /work/p/b.txt", None),
            // Python deletes: unreadable targets count as anywhere.
            (
                "python3 -c \"import shutil; shutil.rmtree('old')\"",
                Some(Anywhere),
            ),
            (
                "python3 -c \"import os; os.remove('a.txt')\"",
                Some(Anywhere),
            ),
            // Not deletes.
            ("ls -la && cat README.md", None),
            ("git push --force origin main", None),
            ("grep -r 'rm -rf' docs/", None),
            ("echo 'remove the old files' > /work/tmp/plan.txt", None),
        ];
        for (cmd, expected) in table {
            assert_eq!(delete_targets(cmd), *expected, "{cmd}");
            // The card predicate over that class: only a target the agent does not own.
            let hit = classify_shell(cmd).filter(|h| h.class == FILE_DELETE);
            assert_eq!(
                hit.as_ref().and_then(|h| h.path_class),
                expected.filter(|c| *c == Anywhere),
                "{cmd}"
            );
            if let Some(h) = hit {
                assert_eq!(h.risk, Risk::Destructive);
            }
        }
        // Other destructive rules carry no path class.
        assert_eq!(
            classify_shell("git push --force origin main")
                .expect("force push")
                .path_class,
            None
        );
        // With the box's view (#1719): a truncation of a file that does not exist, or is
        // empty, creates it — no delete at all; the same idiom on a file with content is one.
        let missing = |_: &str| false;
        for cmd in [
            ": > /work/p/app.log",
            "> /work/p/app.log",
            "cat /dev/null > /work/p/app.log",
            "cp /dev/null /work/p/app.log",
        ] {
            assert_eq!(
                delete_targets_with(cmd, PathFacts::with_content(&missing)),
                None,
                "{cmd} on a missing file"
            );
        }
        let present = |p: &str| p == "/work/p/app.log";
        assert_eq!(
            delete_targets_with(": > /work/p/app.log", PathFacts::with_content(&present)),
            Some(Workspace)
        );
        assert_eq!(
            delete_targets_with(": > /work/p/other.log", PathFacts::with_content(&present)),
            None
        );
        // The view never softens a real delete.
        assert_eq!(
            delete_targets_with("rm -rf /work/p/build", PathFacts::with_content(&missing)),
            Some(Workspace)
        );
    }

    /// The incident this rule was rewritten for, and the half that proves the rule still
    /// exists. Jacob's agent, working in `/work/01M6S2STTSQ0VPW5C03FZQ0CNA` on a Project Euler
    /// task, ran `sed`, a loop over `p*.py`, then `du -sh .; rm d*.html.bak`. The `rm` named
    /// files in the agent's own folder and still raised "Run a destructive command
    /// (file-delete)?", which nobody answered; the card expired and the agent stalled there.
    ///
    /// Both halves are asserted together on purpose: the permissive half alone would pass with
    /// the whole rule deleted.
    #[test]
    fn own_workspace_deletes_pass_and_other_projects_still_card() {
        let mine = "/work/01M6S2STTSQ0VPW5C03FZQ0CNA";
        let facts = PathFacts::assumed().in_workspace(mine);
        let card = |cmd: &str| classify_shell_with(cmd, facts).filter(|h| h.class == FILE_DELETE);

        // Its own workspace: no card, named absolutely or relative to the folder its shell
        // runs in, and for a whole subtree as much as for one file.
        for cmd in [
            "du -sh .; rm d*.html.bak",
            "rm d018.html.bak",
            "rm -rf /work/01M6S2STTSQ0VPW5C03FZQ0CNA/build",
            "cd /work/01M6S2STTSQ0VPW5C03FZQ0CNA/out && rm -f results.csv",
            "find . -name '*.pyc' -delete",
            "git clean -fdx",
            // Scratch is inside the permission, not beside it.
            "rm -rf /tmp/cache /work/tmp/build",
        ] {
            assert_eq!(card(cmd), None, "{cmd} deletes only what this agent owns");
        }

        // Everything it does not own still cards: another project's folder, the box's own
        // state, the workspace root itself, and the system.
        for cmd in [
            "rm -rf /work/01OTHERPROJECT00000000000/exports",
            "rm -f /work/01OTHERPROJECT00000000000/notes.md",
            "cd /work/01OTHERPROJECT00000000000 && git reset --hard",
            "rm -rf /work/.arbos/audit.jsonl",
            "rm -rf /work",
            "rm -rf ..",
            "rm -f /etc/hosts",
            "sudo rm -rf /var/log/old",
        ] {
            let hit = card(cmd).unwrap_or_else(|| panic!("{cmd} must still raise a card"));
            assert_eq!(hit.risk, Risk::Destructive, "{cmd}");
            assert_eq!(hit.path_class, Some(PathClass::Anywhere), "{cmd}");
        }

        // One target outside the workspace cards the whole command, however much of it is
        // inside: the widest class decides.
        assert!(card("rm -rf build /work/01OTHERPROJECT00000000000/build").is_some());

        // A folder whose name merely starts with the workspace's is not inside it.
        assert!(card("rm -rf /work/01M6S2STTSQ0VPW5C03FZQ0CNA-old").is_some());
    }

    /// A job deleting a file it wrote itself raises no card, and the exemption reaches nothing
    /// else: only the exact absolute paths the caller names as its own, never their parent,
    /// their prefix, a glob over them, or a second target beside them.
    #[test]
    fn a_path_the_caller_created_is_exempt_and_nothing_near_it_is() {
        let mine = |p: &str| matches!(p, "/work/p/out/report.md" | "/work/p/out/data.csv");
        let facts = PathFacts {
            has_content: &|_| true,
            created_here: &mine,
            workspace_root: WORKSPACE_ROOT,
        };
        // The class of what is deleted, not whether a card follows: the exemption's job is to
        // drop the caller's own files from the targets, whichever class they are in.
        let class = |cmd: &str| delete_targets_with(cmd, facts);
        // Its own files, named exactly: no card, one at a time or together, and for the
        // truncation idioms too.
        assert_eq!(class("rm -f /work/p/out/report.md"), None);
        assert_eq!(
            class("rm -f /work/p/out/report.md /work/p/out/data.csv"),
            None
        );
        assert_eq!(class("cd /work/p/out && rm -f report.md"), None);
        assert_eq!(class("mv /work/p/out/report.md /dev/null"), None);
        // T8's first trigger: emptying its own output through a `>` redirect. The same idiom on
        // a file it did not write still cards, which is what makes the exemption the reason.
        assert_eq!(class(": > /work/p/out/report.md"), None);
        assert_eq!(
            class(": > /work/p/out/notes.md"),
            Some(PathClass::Workspace)
        );
        assert_eq!(
            delete_targets_with(": > /work/p/out/report.md", PathFacts::assumed()),
            Some(PathClass::Workspace)
        );
        // Everything around them still cards: the parent folder, a sibling it did not write, a
        // glob that would match its file, a directory walk, and a tree-wide git delete.
        assert_eq!(class("rm -rf /work/p/out"), Some(PathClass::Workspace));
        assert_eq!(
            class("rm -f /work/p/out/report.md /work/p/out/notes.md"),
            Some(PathClass::Workspace)
        );
        assert_eq!(
            class("rm -f /work/p/out/*.md"),
            Some(PathClass::Workspace),
            "a glob is not an exact path"
        );
        assert_eq!(
            class("find /work/p/out -name 'report.md' -delete"),
            Some(PathClass::Workspace)
        );
        assert_eq!(class("git clean -fdx"), Some(PathClass::Workspace));
        assert_eq!(
            class("rm -rf /etc/report.md"),
            Some(PathClass::Anywhere),
            "same file name outside what it wrote"
        );
        // No list of its own writes: the same commands count as deletes, as they did before.
        let none = PathFacts::assumed();
        assert_eq!(
            delete_targets_with("rm -f /work/p/out/report.md", none),
            Some(PathClass::Workspace)
        );
    }

    /// The files a command writes by redirect, which a caller records as the run's own outputs:
    /// every redirect form, a leading `cd` honoured, and nothing that names no file.
    #[test]
    fn redirect_targets_are_the_files_a_command_writes() {
        let t = |cmd: &str| redirect_targets("/work/p", cmd);
        assert_eq!(
            t("curl -sS https://x/y > out/taxi_app_curl.txt"),
            ["/work/p/out/taxi_app_curl.txt"]
        );
        assert_eq!(t("python app.py >>logs/app.log"), ["/work/p/logs/app.log"]);
        assert_eq!(t("make 2> build.err"), ["/work/p/build.err"]);
        assert_eq!(t("make &> build.log"), ["/work/p/build.log"]);
        assert_eq!(t("cd out && zip -r - . > app.zip"), ["/work/p/out/app.zip"]);
        assert_eq!(
            t("python app.py > out/a.txt 2> out/b.txt"),
            ["/work/p/out/a.txt", "/work/p/out/b.txt"]
        );
        assert_eq!(t("ls | tee out/list.txt"), [] as [&str; 0]);
        assert_eq!(t("make > /dev/null 2>&1"), [] as [&str; 0]);
        assert_eq!(t("echo '3 > 2' && grep -r 'a>b' src/"), [] as [&str; 0]);
    }

    /// A standing grant covers its class and every narrower one, for its chat or all chats.
    #[test]
    fn shell_grant_covers_narrower_classes_only() {
        let grant = |class: PathClass, chat: Option<&str>| ShellGrant {
            id: "g".into(),
            project_id: "p".into(),
            rule_class: FILE_DELETE.into(),
            path_class: class,
            chat_id: chat.map(str::to_owned),
            granted_by: "u".into(),
            approval_id: None,
            created_at: 0,
        };
        // A workspace-class hit is built here rather than matched from a command: since a
        // delete inside the agent's own workspace raises no card, no command produces one any
        // more. The ordering it checks still governs the grants the relay already holds.
        let ws = RiskMatch {
            class: FILE_DELETE,
            risk: Risk::Destructive,
            why: FILE_DELETE_WHY,
            path_class: Some(PathClass::Workspace),
        };
        let any = classify_shell("rm -rf /etc/x").unwrap();
        assert!(grant(PathClass::Workspace, None).covers(&ws, Some("c1")));
        assert!(!grant(PathClass::Workspace, None).covers(&any, Some("c1")));
        assert!(grant(PathClass::Anywhere, None).covers(&any, None));
        assert!(!grant(PathClass::Scratch, None).covers(&ws, None));
        assert!(grant(PathClass::Anywhere, Some("c1")).covers(&ws, Some("c1")));
        assert!(!grant(PathClass::Anywhere, Some("c1")).covers(&ws, Some("c2")));
        let push = classify_shell("git push -f").unwrap();
        assert!(!grant(PathClass::Anywhere, None).covers(&push, None));
        assert!(PathClass::Anywhere.covers(PathClass::Scratch));
        assert!(!PathClass::Scratch.covers(PathClass::Workspace));
        assert_eq!(PathClass::parse("workspace"), Some(PathClass::Workspace));
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
