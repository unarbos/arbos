//! Plain-words label for what the agent is doing right now, derived from the tool it just
//! started (agent-flow contract §2). "Cloning RaoFoundation/subtensor", "Running tests",
//! "Reading src/lib.rs". `None` when nothing concrete fits: clients then show "<agent> is working".

use serde_json::Value;

const MAX_CHARS: usize = 48;

/// `mcp__arbos__web_search` → `web_search`: project tools reach pi under the MCP server prefix.
fn bare_tool_name(tool_name: &str) -> &str {
    tool_name
        .strip_prefix("mcp__")
        .and_then(|rest| rest.split_once("__"))
        .map(|(_, name)| name)
        .unwrap_or(tool_name)
}

/// A search query as the user would read it in the status line: quotes and operators dropped,
/// cut at a word boundary. "surf schools Santa Teresa prices" → "Searching for surf schools…".
fn query_label(verb: &str, q: &str) -> String {
    let cleaned: String = q
        .split_whitespace()
        .filter(|w| !w.contains(':') || w.len() > 24)
        .collect::<Vec<_>>()
        .join(" ")
        .replace(['"', '\''], "");
    let cleaned = cleaned.trim().trim_end_matches(['.', '?']);
    if cleaned.is_empty() {
        return format!("{verb} the web");
    }
    let room = MAX_CHARS.saturating_sub(verb.len() + 5);
    let mut out = String::new();
    for w in cleaned.split_whitespace() {
        if !out.is_empty() && out.len() + 1 + w.len() > room {
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(w);
    }
    if out.is_empty() {
        out = cleaned.chars().take(room).collect();
    }
    format!("{verb} for {out}")
}

/// Label for a tool call, or `None`.
pub fn label(tool_name: &str, args: &Value) -> Option<String> {
    let tool_name = bare_tool_name(tool_name);
    let s = |keys: &[&str]| -> Option<&str> {
        keys.iter()
            .find_map(|k| args.get(k).and_then(Value::as_str))
    };
    let out = match tool_name {
        "bash" | "shell" | "execute" | "run_command" | "execute_command" => {
            command_label(s(&["command", "cmd"])?)
        }
        "read" | "read_file" | "cat" => Some(format!(
            "Reading {}",
            short_path(s(&["path", "file_path"])?)
        )),
        "write" | "write_file" | "create_file" => Some(format!(
            "Writing {}",
            short_path(s(&["path", "file_path"])?)
        )),
        "edit" | "edit_file" | "str_replace" | "multi_edit" => Some(format!(
            "Editing {}",
            short_path(s(&["path", "file_path"])?)
        )),
        "grep" | "search" => Some(match s(&["pattern", "query"]) {
            Some(q) if !q.trim().is_empty() => query_label("Searching the code", q),
            _ => "Searching the project".into(),
        }),
        "glob" | "find" | "ls" | "list" => Some("Searching the project".into()),
        "web_fetch" | "fetch" | "fetch_url" | "browse" | "browser_navigate" | "browser_open" => {
            Some(match s(&["url"]).and_then(host_of) {
                Some(h) => format!("Reading {h}"),
                None => "Reading a web page".into(),
            })
        }
        "web_search" | "search_web" => Some(match s(&["query", "q"]) {
            Some(q) => query_label("Searching", q),
            None => "Searching the web".into(),
        }),
        "context_search" | "history_search" | "memory_search" | "chat_read" | "chat_search" => {
            Some("Looking back through the chat".into())
        }
        "secrets_list" | "secrets_request" | "vault_list" | "vault_request" | "egress"
        | "browser_fill" => Some("Using your secrets".into()),
        "routines_create" | "routines_update" | "routines_delete" | "routines_list" => {
            Some("Setting the schedule".into())
        }
        "ask_user" => Some("Waiting for your answer".into()),
        "generate_image" => Some("Making the image".into()),
        "deliver_file" => Some("Sending the file".into()),
        "submit_feedback" => Some("Filing your feedback".into()),
        "publish_site" | "unpublish_site" | "list_sites" => Some("Publishing the site".into()),
        "connector_call" | "connectors_list" => Some("Checking your connected accounts".into()),
        "local_run" | "local_read" | "local_list" | "local_devices" => {
            Some("Working on your computer".into())
        }
        // Prime workers call project tools from Python (`mcp.call_tool("arbos", "web_search",
        // {"query": …})`); pi's `codemode` from JavaScript. Both are read for the tool they call.
        "codemode" | "ipython" | "python" | "execute_python" | "run_python" => {
            code_label(s(&["code", "source", "script"])?)
        }
        _ => None,
    };
    out.map(|l| trim_to(&l, MAX_CHARS))
}

/// The line when nothing concrete fits but the agent is at work (never leave the presence row
/// empty mid-turn, #1657).
pub fn generic_label(tool_name: &str) -> String {
    match bare_tool_name(tool_name) {
        "bash" | "shell" | "execute" | "run_command" | "execute_command" | "codemode"
        | "ipython" | "python" => "Running a command".to_owned(),
        _ => "Working on it".to_owned(),
    }
}

/// A shell command head → label. Compound commands (`&&`, `;`, `|`) are judged by their first
/// recognisable step.
fn command_label(cmd: &str) -> Option<String> {
    let cmd = cmd.trim();
    let steps: Vec<&str> = cmd
        .split("&&")
        .flat_map(|s| s.split(';'))
        .flat_map(|s| s.split("||"))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    for step in &steps {
        if let Some(l) = step_label(step) {
            return Some(l);
        }
    }
    None
}

fn step_label(step: &str) -> Option<String> {
    // Drop leading `cd x`, env assignments, sudo, time, nohup.
    let mut words: Vec<&str> = step.split_whitespace().collect();
    while let Some(w) = words.first() {
        if w.contains('=') && !w.starts_with('-')
            || matches!(*w, "sudo" | "time" | "nohup" | "env" | "exec")
        {
            words.remove(0);
        } else {
            break;
        }
    }
    if words.is_empty() {
        return None;
    }
    if words[0] == "cd" {
        return None;
    }
    let prog = words[0].rsplit('/').next().unwrap_or(words[0]);
    let sub = words.get(1).copied().unwrap_or("");
    let has = |flag: &str| words.contains(&flag);
    let positional = |from: usize| -> Option<&str> {
        words
            .iter()
            .skip(from)
            .find(|w| !w.starts_with('-') && !w.contains('='))
            .copied()
    };
    let l = match (prog, sub) {
        ("git", "clone") => {
            // The URL is the first argument that looks like one (`--depth 1` must not win).
            let target = words
                .iter()
                .skip(2)
                .find(|w| !w.starts_with('-') && (w.contains('/') || w.contains(':')))
                .map(|w| repo_name(w))
                .unwrap_or_else(|| "the repository".into());
            format!("Cloning {target}")
        }
        ("git", "pull") | ("git", "fetch") => "Fetching the latest changes".into(),
        ("git", "push") => format!("Pushing to {}", positional(2).unwrap_or("origin")),
        ("git", "commit") => "Committing the changes".into(),
        ("git", "checkout") | ("git", "switch") => match positional(2) {
            Some(b) => format!("Checking out {b}"),
            None => "Checking out".into(),
        },
        ("git", "log")
        | ("git", "show")
        | ("git", "status")
        | ("git", "diff")
        | ("git", "rev-parse")
        | ("git", "branch") => "Reading the git history".into(),
        ("gh", "pr") | ("gh", "issue") | ("gh", "repo") => "Talking to GitHub".into(),
        ("cargo", "test")
        | ("pytest", _)
        | ("npm", "test")
        | ("pnpm", "test")
        | ("yarn", "test")
        | ("go", "test")
        | ("bun", "test")
        | ("jest", _)
        | ("vitest", _) => "Running tests".into(),
        ("cargo", "build")
        | ("cargo", "check")
        | ("cargo", "clippy")
        | ("make", _)
        | ("cmake", _)
        | ("go", "build")
        | ("tsc", _)
        | ("swift", "build")
        | ("xcodebuild", _)
        | ("gradle", _)
        | ("mvn", _) => "Building".into(),
        ("npm", "run") | ("pnpm", "run") | ("yarn", "run") | ("bun", "run") => {
            match positional(2) {
                Some("build") => "Building".into(),
                Some("test") => "Running tests".into(),
                Some("lint") => "Linting".into(),
                Some("dev") | Some("start") => "Starting the dev server".into(),
                Some(x) => format!("Running {x}"),
                None => "Running a script".into(),
            }
        }
        ("cargo", "run")
        | ("npm", "start")
        | ("python", _)
        | ("python3", _)
        | ("node", _)
        | ("uv", "run")
        | ("bun", "x")
        | ("deno", "run") => match positional(if prog == "uv" { 2 } else { 1 }) {
            Some(f) if f.contains('.') || f.contains('/') => format!("Running {}", short_path(f)),
            _ => "Running the program".into(),
        },
        ("cargo", "fmt")
        | ("rustfmt", _)
        | ("prettier", _)
        | ("black", _)
        | ("ruff", "format")
        | ("gofmt", _) => "Formatting the code".into(),
        ("ruff", _) | ("eslint", _) | ("pylint", _) | ("mypy", _) | ("shellcheck", _) => {
            "Linting".into()
        }
        ("npm", "install")
        | ("npm", "i")
        | ("npm", "ci")
        | ("pnpm", "install")
        | ("pnpm", "i")
        | ("pnpm", "add")
        | ("yarn", "add")
        | ("yarn", "install")
        | ("bun", "install")
        | ("bun", "add")
        | ("pip", "install")
        | ("pip3", "install")
        | ("uv", "pip")
        | ("uv", "add")
        | ("uv", "sync")
        | ("cargo", "add")
        | ("cargo", "install")
        | ("brew", "install")
        | ("apt", "install")
        | ("apt-get", "install")
        | ("go", "get")
        | ("gem", "install") => "Installing dependencies".into(),
        ("docker", "build") => "Building the container".into(),
        ("docker", _) | ("docker-compose", _) | ("kubectl", _) => "Working with containers".into(),
        ("curl", _) | ("wget", _) | ("http", _) | ("httpie", _) => {
            match words.iter().find_map(|w| host_of(w)) {
                Some(h) => format!("Reading {h}"),
                None => "Fetching from the web".into(),
            }
        }
        ("cat", _) | ("head", _) | ("tail", _) | ("less", _) | ("bat", _) => match positional(1) {
            Some(p) => format!("Reading {}", short_path(p)),
            None => "Reading a file".into(),
        },
        ("ls", _)
        | ("tree", _)
        | ("find", _)
        | ("rg", _)
        | ("grep", _)
        | ("fd", _)
        | ("ag", _)
        | ("wc", _)
        | ("du", _) => "Searching the project".into(),
        ("mkdir", _) | ("touch", _) => "Setting up the folder".into(),
        ("cp", _) | ("mv", _) | ("rsync", _) => "Moving files".into(),
        ("rm", _) => "Removing files".into(),
        ("tar", _) | ("unzip", _) | ("zip", _) | ("gzip", _) | ("gunzip", _) => {
            "Unpacking the archive".into()
        }
        ("ssh", _) | ("scp", _) => "Reaching the remote machine".into(),
        ("sed", _) | ("awk", _) | ("jq", _) | ("sort", _) | ("cut", _) | ("tr", _) => return None,
        ("echo", _)
        | ("printf", _)
        | ("true", _)
        | ("pwd", _)
        | ("which", _)
        | ("date", _)
        | ("sleep", _)
        | ("export", _) => return None,
        ("ffmpeg", _) | ("ffprobe", _) | ("convert", _) | ("magick", _) => {
            "Processing the media".into()
        }
        ("sqlite3", _) | ("psql", _) | ("mysql", _) => "Querying the database".into(),
        (_, _) if has("--version") || has("-V") => return None,
        _ => return None,
    };
    Some(l)
}

/// `codemode` JavaScript snippets name the arbos tool they call: `arbos.history_search(...)`.
fn code_label(code: &str) -> Option<String> {
    // `tools.mcp__arbos__web_search({query: "…"})` / `arbos.web_search(...)`: the first project
    // tool the script calls names the activity; a search query makes it concrete.
    let after = [
        "mcp__arbos__",
        "arbos.",
        "call_tool(\"arbos\", \"",
        "call_tool(\"arbos\",\"",
        "call_tool('arbos', '",
        "call_tool('arbos','",
    ]
    .iter()
    .filter_map(|marker| code.find(marker).map(|i| (i, i + marker.len())))
    .min_by_key(|(i, _)| *i)
    .map(|(_, end)| &code[end..])?;
    let name: String = after
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    // The argument is often a variable filled from a list above the call (`qs=["best yoga
    // studios …", …]; for q in qs: … {"query": q}`): then the first literal in the script that
    // looks like a search phrase / a URL stands in for it.
    if name == "web_search" {
        if let Some(q) = first_string_arg(after, &["query", "q"]).or_else(|| {
            string_literals(code).find(|l| l.split_whitespace().count() >= 3 && !l.contains("://"))
        }) {
            return Some(query_label("Searching", &q));
        }
    }
    if name == "web_fetch" {
        if let Some(h) = first_string_arg(after, &["url"])
            .or_else(|| {
                string_literals(code)
                    .find(|l| l.starts_with("http://") || l.starts_with("https://"))
            })
            .as_deref()
            .and_then(host_of)
        {
            return Some(format!("Reading {h}"));
        }
    }
    let l = match name.as_str() {
        "history_search" | "history_read" | "threads" | "thread_read" => {
            "Looking back through the chat"
        }
        "memory_search" | "memory_read" | "memory_write" | "remember" => "Checking project memory",
        "projects" | "project_read" | "group_send" => "Checking the other projects",
        "subagents" | "subagent_read" | "subagent_status" => "Checking on the background work",
        "draft_message" => "Drafting the message",
        "deliver_file" => "Sending the file",
        "generate_image" => "Making the image",
        "routines_create" | "routines_update" | "routines_delete" | "routines_list" => {
            "Setting the schedule"
        }
        "goals_create" | "goals_update" | "goals_delete" | "goals_list" | "goals_log" => {
            "Updating a goal"
        }
        "vault_list" | "vault_request" | "secrets_list" | "secrets_request" | "egress"
        | "browser_fill" => "Using your secrets",
        "connector_call" | "connectors_list" => "Checking your connected accounts",
        "web_search" => "Searching the web",
        "web_fetch" => "Reading a web page",
        "submit_feedback" => "Filing your feedback",
        "publish_site" | "unpublish_site" | "list_sites" => "Publishing the site",
        _ => return None,
    };
    Some(l.into())
}

/// `https://github.com/RaoFoundation/subtensor.git` → `RaoFoundation/subtensor`;
/// `git@github.com:org/repo.git` → `org/repo`; a bare `org/repo` stays.
fn repo_name(target: &str) -> String {
    let t = target.trim_matches(|c| c == '"' || c == '\'');
    let t = t.strip_suffix(".git").unwrap_or(t).trim_end_matches('/');
    let path = if let Some(i) = t.find("://") {
        t[i + 3..].split_once('/').map(|x| x.1).unwrap_or(t)
    } else if let Some((_, p)) = t.split_once(':').filter(|(h, _)| h.contains('@')) {
        p
    } else {
        t
    };
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    match parts.len() {
        0 => "the repository".into(),
        1 => parts[0].into(),
        n => format!("{}/{}", parts[n - 2], parts[n - 1]),
    }
}

fn host_of(url: &str) -> Option<String> {
    let rest = url
        .trim_matches(|c| c == '"' || c == '\'')
        .split("://")
        .nth(1)?;
    let host = rest.split(['/', '?', '#']).next()?;
    let host = host.rsplit('@').next()?.split(':').next()?;
    let host = host.strip_prefix("www.").unwrap_or(host);
    if host.is_empty() || !host.contains('.') {
        return None;
    }
    Some(host.to_owned())
}

/// Last two path components at most, so the label fits on one line.
fn short_path(p: &str) -> String {
    let p = p.trim_matches(|c| c == '"' || c == '\'');
    let parts: Vec<&str> = p.split('/').filter(|s| !s.is_empty()).collect();
    match parts.len() {
        0 => p.to_owned(),
        1 => parts[0].to_owned(),
        n => format!("{}/{}", parts[n - 2], parts[n - 1]),
    }
}

fn trim_to(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out.push('…');
    out
}

/// `{query: "surf schools"}` / `{ "query": 'x' }` → the first string value of one of `keys` in
/// the call's argument object, plain text only.
fn first_string_arg(call: &str, keys: &[&str]) -> Option<String> {
    let args: String = call.chars().take(600).collect();
    let args = args.as_str();
    for k in keys {
        for pat in [format!("{k}:"), format!("\"{k}\":"), format!("'{k}':")] {
            if let Some(i) = args.find(&pat) {
                let rest = args[i + pat.len()..].trim_start();
                let quote = rest.chars().next()?;
                if !matches!(quote, '"' | '\'' | '`') {
                    continue;
                }
                let body = &rest[1..];
                let end = body.find(quote)?;
                let v = body[..end].trim();
                if !v.is_empty() {
                    return Some(v.to_owned());
                }
            }
        }
    }
    None
}

/// Every `"…"` / `'…'` literal of a script, in order, without the quotes (no escapes handled:
/// a literal with an escaped quote just ends early, which is fine for a status line).
fn string_literals(code: &str) -> impl Iterator<Item = String> + '_ {
    let mut rest = code;
    std::iter::from_fn(move || loop {
        let i = rest.find(['"', '\''])?;
        let quote = rest.as_bytes()[i] as char;
        let body = &rest[i + 1..];
        let Some(end) = body.find(quote) else {
            rest = "";
            return None;
        };
        let lit = body[..end].trim().to_owned();
        rest = &body[end + 1..];
        if !lit.is_empty() {
            return Some(lit);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Grok parity (traits 14): the status line names the search ("Searching for surf schools
    /// near Santa Teresa"), also when the tool arrives under its MCP prefix or inside a script.
    #[test]
    fn searches_are_described_from_their_query() {
        assert_eq!(
            label(
                "mcp__arbos__web_search",
                &json!({ "query": "best reviewed surf schools near Santa Teresa prices" })
            )
            .as_deref(),
            Some("Searching for best reviewed surf schools near")
        );
        assert_eq!(
            label(
                "web_search",
                &json!({ "query": "\"Del Soul\" surf school site:tripadvisor.com" })
            )
            .as_deref(),
            Some("Searching for Del Soul surf school")
        );
        assert_eq!(
            label(
                "mcp__arbos__web_fetch",
                &json!({ "url": "https://www.tripadvisor.com/x" })
            )
            .as_deref(),
            Some("Reading tripadvisor.com")
        );
        assert_eq!(
            label("grep", &json!({ "pattern": "spend_cap" })).as_deref(),
            Some("Searching the code for spend_cap")
        );
        assert_eq!(
            label(
                "codemode",
                &json!({ "code": "const r = await tools.mcp__arbos__web_search({query: \"surf schools Santa Teresa\"}); return r;" })
            )
            .as_deref(),
            Some("Searching for surf schools Santa Teresa")
        );
        assert_eq!(
            label(
                "codemode",
                &json!({ "code": "await tools.mcp__arbos__routines_create({name: 'x'})" })
            )
            .as_deref(),
            Some("Setting the schedule")
        );
        // Prime worker: Python calling the project tools (what the surf-school research ran).
        assert_eq!(
            label(
                "ipython",
                &json!({ "code": "r = await mcp.call_tool(\"arbos\",\"web_search\",{\"query\":\"best surf schools Santa Teresa Costa Rica TripAdvisor reviews\"})\nprint(str(r)[:4000])" })
            )
            .as_deref(),
            Some("Searching for best surf schools Santa Teresa")
        );
        assert_eq!(
            label(
                "ipython",
                &json!({ "code": "async def fetch(u):\n    r=await mcp.call_tool(\"arbos\",\"web_fetch\",{\"url\":u}); return str(r)\nurls=[\"https://www.ondateresa.com/guide\"]" })
            )
            .as_deref(),
            Some("Reading ondateresa.com")
        );
        assert_eq!(
            label("ipython", &json!({ "code": "await mcp.call_tool('arbos', 'web_fetch', {'url': 'https://www.bluemysticsurf.com/'})" })).as_deref(),
            Some("Reading bluemysticsurf.com")
        );
        // Queries / URLs kept in a list and passed as a variable (the yoga-studio run).
        assert_eq!(
            label(
                "ipython",
                &json!({ "code": "qs=[\"best yoga studios Santa Teresa Costa Rica\",\"Santa Teresa yoga drop-in price\"]\nfor q in qs:\n    r=await mcp.call_tool(\"arbos\",\"web_search\",{\"query\":q})" })
            )
            .as_deref(),
            Some("Searching for best yoga studios Santa Teresa")
        );
        assert_eq!(
            label(
                "ipython",
                &json!({ "code": "for u in [\"https://www.tripadvisor.com/Attractions-g635538\",\"https://horizon-yogahotel.com/yoga/\"]:\n    r=await mcp.call_tool(\"arbos\",\"web_fetch\",{\"url\":u})" })
            )
            .as_deref(),
            Some("Reading tripadvisor.com")
        );
        assert_eq!(generic_label("ipython"), "Running a command");
        assert_eq!(
            label("mcp__arbos__ask_user", &json!({})).as_deref(),
            Some("Waiting for your answer")
        );
        assert_eq!(generic_label("mcp__arbos__codemode"), "Running a command");
        assert!(label("web_search", &json!({})).unwrap().len() <= MAX_CHARS);
    }
}
