//! Two things a shell command may not do by accident: commit as nobody, and
//! land work on the wrong branch.
//!
//! Checked before every `bash` call. The command is cut into segments at
//! `&&`, `||`, `;`, and `|`; each segment that starts a `git` or `gh` call
//! is looked at. Quoted text is left alone, so `echo "git commit"` passes.
//! Settings come from `<place>/.arbos/git.toml`; without one, the base is
//! unknown and `main`/`master` are protected.

use anyhow::{Result, bail};
use serde::Deserialize;
use std::path::Path;
use std::process::Command;

/// `<place>/.arbos/git.toml`.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GitRules {
    /// The branch pull requests target. Empty = unknown: `gh pr create`
    /// then only needs an explicit `--base`.
    pub base: String,
    /// Branches a direct push may not land on. The base is always one.
    pub protected: Vec<String>,
    pub enabled: bool,
}

impl Default for GitRules {
    fn default() -> Self {
        Self {
            base: String::new(),
            protected: vec!["main".into(), "master".into()],
            enabled: true,
        }
    }
}

impl GitRules {
    pub fn load(place: &Path) -> Self {
        let path = place.join(".arbos").join("git.toml");
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        match toml::from_str::<Self>(&text) {
            Ok(rules) => rules,
            Err(e) => {
                eprintln!("{}: {e}; using the defaults", path.display());
                Self::default()
            }
        }
    }

    fn is_protected(&self, branch: &str) -> bool {
        let b = branch.trim_start_matches("refs/heads/");
        (!self.base.is_empty() && b == self.base) || self.protected.iter().any(|p| p == b)
    }

    /// The line the instance prompt carries.
    pub fn prompt_line(&self) -> String {
        if !self.enabled {
            return String::new();
        }
        let base = if self.base.is_empty() {
            "not configured: name --base explicitly on gh pr create".to_string()
        } else {
            format!("`{}`: open PRs against it, never push to it", self.base)
        };
        format!(
            "Git: base branch {base}. Protected: {}. Commits need a configured user.name/user.email, and never an identity you have not confirmed with the user — when the repo's identity may be someone else's, ask once who the author should be.",
            self.protected.join(", ")
        )
    }
}

/// Refuse a command the rules forbid; Ok when it may run.
pub fn check(place: &Path, cwd: &Path, command: &str) -> Result<()> {
    if std::env::var("ARBOS_GIT_GUARD").is_ok_and(|v| v == "off" || v == "0") {
        return Ok(());
    }
    let rules = GitRules::load(place);
    if !rules.enabled {
        return Ok(());
    }
    // `git config user.email … && git commit` sets the identity in the
    // same command: the check must see what the earlier segment does.
    let mut configured = (false, false);
    for segment in segments(command) {
        let words = shell_words(&segment);
        let Some(call) = GitCall::parse(&words) else {
            continue;
        };
        match call {
            GitCall::Config { name, email } => {
                configured.0 |= name;
                configured.1 |= email;
            }
            GitCall::Commit { dir, sets_author } => {
                if sets_author {
                    continue;
                }
                let dir = dir
                    .map(|d| cwd.join(d))
                    .unwrap_or_else(|| cwd.to_path_buf());
                let (name, email) = identity(&dir);
                let name = if configured.0 {
                    "set".to_string()
                } else {
                    name
                };
                let email = if configured.1 {
                    "set".to_string()
                } else {
                    email
                };
                if name.is_empty() || email.is_empty() {
                    bail!(
                        "git guard: {} has no {} configured, so this commit would be attributed to nobody or to a machine default. Ask the user who the author is, then run: git config user.name \"<name>\" && git config user.email \"<email>\" — or pass --author=\"Name <email>\" on the commit.",
                        dir.display(),
                        match (name.is_empty(), email.is_empty()) {
                            (true, true) => "user.name or user.email",
                            (true, false) => "user.name",
                            _ => "user.email",
                        }
                    );
                }
            }
            GitCall::Push { targets } => {
                for t in targets {
                    if rules.is_protected(&t) {
                        bail!(
                            "git guard: a direct push to `{t}` is refused; it is a protected branch here ({}). Push a feature branch (git push -u origin <branch>) and open a pull request{}.",
                            rules.protected.join(", "),
                            if rules.base.is_empty() {
                                String::new()
                            } else {
                                format!(" against `{}`", rules.base)
                            }
                        );
                    }
                }
            }
            GitCall::PrCreate { base } => match (base, rules.base.is_empty()) {
                (None, true) => bail!(
                    "git guard: gh pr create needs --base <branch>: this place does not configure one (set base in .arbos/git.toml, or name it on the command)."
                ),
                (None, false) => bail!(
                    "git guard: gh pr create needs --base {}: pull requests here target that branch.",
                    rules.base
                ),
                (Some(b), false) if b != rules.base => bail!(
                    "git guard: pull requests here target `{}`, not `{b}`. Use --base {} (or change base in .arbos/git.toml if that is really wanted).",
                    rules.base,
                    rules.base
                ),
                (Some(_), _) => {}
            },
        }
    }
    Ok(())
}

/// The `user.name` and `user.email` git would use in `dir`.
fn identity(dir: &Path) -> (String, String) {
    let get = |key: &str| -> String {
        Command::new("git")
            .args(["config", "--get", key])
            .current_dir(dir)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default()
    };
    (get("user.name"), get("user.email"))
}

#[derive(Debug, PartialEq, Eq)]
enum GitCall {
    Commit {
        /// `-C <dir>`, when given.
        dir: Option<String>,
        /// `--author`, `-c user.email=`, or `GIT_AUTHOR_EMAIL=` in front.
        sets_author: bool,
    },
    Push {
        /// Branch names the push lands on (the remote side of each refspec;
        /// the local name when there is no colon).
        targets: Vec<String>,
    },
    PrCreate {
        base: Option<String>,
    },
    /// `git config user.name|user.email <value>` earlier in the command.
    Config {
        name: bool,
        email: bool,
    },
}

impl GitCall {
    fn parse(words: &[String]) -> Option<Self> {
        let mut i = 0;
        let mut env_author = false;
        // Leading VAR=value assignments and `command` / `env` wrappers.
        while i < words.len() {
            let w = &words[i];
            if let Some((k, _)) = w.split_once('=')
                && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && !k.is_empty()
            {
                if k == "GIT_AUTHOR_EMAIL" || k == "GIT_AUTHOR_NAME" {
                    env_author = true;
                }
                i += 1;
            } else if w == "command" || w == "env" || w == "exec" {
                i += 1;
            } else {
                break;
            }
        }
        let program = words.get(i)?.trim_start_matches('\\');
        let program = Path::new(program)
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default();
        i += 1;
        match program.as_str() {
            "git" => Self::parse_git(&words[i..], env_author),
            "gh" => Self::parse_gh(&words[i..]),
            _ => None,
        }
    }

    fn parse_git(words: &[String], env_author: bool) -> Option<Self> {
        let mut i = 0;
        let mut dir: Option<String> = None;
        let mut sets_author = env_author;
        // Global options before the subcommand.
        while i < words.len() {
            let w = words[i].as_str();
            if w == "-C" {
                dir = words.get(i + 1).cloned();
                i += 2;
            } else if let Some(d) = w.strip_prefix("-C") {
                dir = Some(d.to_string());
                i += 1;
            } else if w == "-c" {
                if words
                    .get(i + 1)
                    .is_some_and(|v| v.starts_with("user.email=") || v.starts_with("user.name="))
                {
                    sets_author = true;
                }
                i += 2;
            } else if w.starts_with('-') {
                i += 1;
            } else {
                break;
            }
        }
        let sub = words.get(i)?.as_str();
        let rest = &words[i + 1..];
        match sub {
            "commit" => {
                if rest
                    .iter()
                    .any(|w| w == "--help" || w == "-h" || w == "--dry-run")
                {
                    return None;
                }
                if rest.iter().any(|w| w.starts_with("--author")) {
                    sets_author = true;
                }
                Some(Self::Commit { dir, sets_author })
            }
            "config" => {
                let setting = rest
                    .iter()
                    .filter(|w| !w.starts_with('-'))
                    .collect::<Vec<_>>();
                match setting.as_slice() {
                    [key, _value, ..] => Some(Self::Config {
                        name: key.as_str() == "user.name",
                        email: key.as_str() == "user.email",
                    }),
                    _ => None,
                }
            }
            "push" => {
                if rest
                    .iter()
                    .any(|w| w == "--dry-run" || w == "-n" || w == "--help")
                {
                    return None;
                }
                // `git push [opts] <remote> <refspec>...`; the first bare
                // word is the remote, the rest are refspecs. `HEAD:main`
                // and `+HEAD:main` land on `main`.
                let mut bare: Vec<&str> = Vec::new();
                let mut k = 0;
                while k < rest.len() {
                    let w = rest[k].as_str();
                    if w == "-o" || w == "--push-option" || w == "--repo" || w == "--receive-pack" {
                        k += 2;
                        continue;
                    }
                    if w.starts_with('-') {
                        k += 1;
                        continue;
                    }
                    bare.push(w);
                    k += 1;
                }
                let targets = bare
                    .iter()
                    .skip(1)
                    .map(|spec| {
                        let spec = spec.trim_start_matches('+');
                        match spec.split_once(':') {
                            Some((_, remote_side)) => remote_side.to_string(),
                            None => spec.to_string(),
                        }
                    })
                    .filter(|t| !t.is_empty() && !t.starts_with(':'))
                    .collect();
                Some(Self::Push { targets })
            }
            _ => None,
        }
    }

    fn parse_gh(words: &[String]) -> Option<Self> {
        if words.len() < 2 || words[0] != "pr" || words[1] != "create" {
            return None;
        }
        let rest = &words[2..];
        if rest
            .iter()
            .any(|w| w == "--help" || w == "-h" || w == "--web" || w == "-w")
        {
            return None;
        }
        let mut base = None;
        let mut i = 0;
        while i < rest.len() {
            let w = rest[i].as_str();
            if w == "--base" || w == "-B" {
                base = rest.get(i + 1).cloned();
                i += 2;
            } else if let Some(b) = w.strip_prefix("--base=") {
                base = Some(b.to_string());
                i += 1;
            } else {
                i += 1;
            }
        }
        Some(Self::PrCreate { base })
    }
}

/// Cut at `&&`, `||`, `;`, `|`, and newlines, outside quotes.
fn segments(command: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut chars = command.chars().peekable();
    while let Some(c) = chars.next() {
        match quote {
            Some(q) => {
                cur.push(c);
                if c == '\\' && q == '"' {
                    if let Some(n) = chars.next() {
                        cur.push(n);
                    }
                } else if c == q {
                    quote = None;
                }
            }
            None => match c {
                '\'' | '"' => {
                    quote = Some(c);
                    cur.push(c);
                }
                '\\' => {
                    cur.push(c);
                    if let Some(n) = chars.next() {
                        cur.push(n);
                    }
                }
                '&' | '|' if chars.peek() == Some(&c) => {
                    chars.next();
                    out.push(std::mem::take(&mut cur));
                }
                ';' | '|' | '\n' => out.push(std::mem::take(&mut cur)),
                _ => cur.push(c),
            },
        }
    }
    out.push(cur);
    out.into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Whitespace split that keeps quoted spans as one word, quotes removed.
fn shell_words(segment: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    let mut chars = segment.chars().peekable();
    while let Some(c) = chars.next() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                } else if c == '\\' && q == '"' {
                    if let Some(n) = chars.next() {
                        cur.push(n);
                    }
                } else {
                    cur.push(c);
                }
            }
            None => match c {
                '\'' | '"' => {
                    quote = Some(c);
                    in_word = true;
                }
                '\\' => {
                    if let Some(n) = chars.next() {
                        cur.push(n);
                        in_word = true;
                    }
                }
                c if c.is_whitespace() => {
                    if in_word {
                        out.push(std::mem::take(&mut cur));
                        in_word = false;
                    }
                }
                _ => {
                    cur.push(c);
                    in_word = true;
                }
            },
        }
    }
    if in_word {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(cmd: &str) -> Option<GitCall> {
        GitCall::parse(&shell_words(cmd))
    }

    #[test]
    fn recognises_commit_push_and_pr_create() {
        assert_eq!(
            call("git commit -m 'x'"),
            Some(GitCall::Commit {
                dir: None,
                sets_author: false
            })
        );
        assert_eq!(
            call("git -C sub commit --author='A <a@b>' -m x"),
            Some(GitCall::Commit {
                dir: Some("sub".into()),
                sets_author: true
            })
        );
        assert_eq!(
            call("GIT_AUTHOR_EMAIL=a@b git commit -m x"),
            Some(GitCall::Commit {
                dir: None,
                sets_author: true
            })
        );
        assert_eq!(
            call("git push origin HEAD:main"),
            Some(GitCall::Push {
                targets: vec!["main".into()]
            })
        );
        assert_eq!(
            call("git push -u origin feature-x"),
            Some(GitCall::Push {
                targets: vec!["feature-x".into()]
            })
        );
        assert_eq!(
            call("gh pr create --base rust --title t"),
            Some(GitCall::PrCreate {
                base: Some("rust".into())
            })
        );
        assert_eq!(
            call("gh pr create --title t"),
            Some(GitCall::PrCreate { base: None })
        );
        assert_eq!(
            call("git config user.email qa@x"),
            Some(GitCall::Config {
                name: false,
                email: true
            })
        );
        assert_eq!(call("git config --get user.email"), None);
        assert_eq!(call("echo 'git commit'"), None);
        assert_eq!(call("git commit --help"), None);
        assert_eq!(call("git push --dry-run origin main"), None);
    }

    #[test]
    fn segments_split_outside_quotes() {
        assert_eq!(
            segments("a && b || c; d | e \"x && y\""),
            vec!["a", "b", "c", "d", "e \"x && y\""]
        );
    }
}
