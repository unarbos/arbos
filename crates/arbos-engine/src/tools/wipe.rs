//! The guard between the agent and a wiped tree.
//!
//! qal-j15: `cd / && rm -rf *` ran with no ask and no refusal — seven
//! times, against the project's own store — because the old check split
//! the command on `&&` and `;` and read each piece alone: the `rm` piece
//! named `*`, and the `cd` that made `*` mean everything under `/` was in
//! another piece, never read. The same shape passed as `cd /; rm -rf ./*`,
//! `cd /usr && rm -rf *`, `rm -rf "$PWD"/*`, `find / -delete`, and through
//! `sh -c`.
//!
//! This module reads the command as a shell would, as far as a removal's
//! target is concerned: it walks the pipeline in order, carries the
//! **effective directory** across `cd`/`pushd` and simple assignments,
//! unwraps `sh -c`/`bash -c`/`eval`, and resolves each removal's target
//! against that directory before judging it. A removal whose target
//! resolves to the filesystem root, a home, or a top-level system tree is
//! **refused in every mode** — there is no agent's reason to remove
//! everything under `/` or `$HOME`. A removal the kernel cannot place
//! (an unknown directory and a target that means "here") asks.

use std::path::{Component, Path, PathBuf};

/// What the bash tool does with a command before it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Nothing here wipes a tree.
    Run,
    /// A card in ask mode; refused with the reason in auto (sudo, mkfs, a
    /// fork bomb, a removal the kernel cannot place).
    Ask(String),
    /// Refused in every mode, with the reason: a root, home, or system
    /// tree wipe.
    Refuse(String),
}

impl Verdict {
    /// The stronger of two: a refusal outranks an ask outranks nothing.
    fn or(self, other: Verdict) -> Verdict {
        match (&self, &other) {
            (Verdict::Refuse(_), _) => self,
            (_, Verdict::Refuse(_)) => other,
            (Verdict::Ask(_), _) => self,
            _ => other,
        }
    }
}

/// Where the command runs, as known before it runs.
pub struct Where<'a> {
    /// The tool call's working directory.
    pub cwd: &'a Path,
    /// The user's home, `$HOME`.
    pub home: &'a Path,
    /// The project folder; its root is asked for, not silently removed.
    pub place: Option<&'a Path>,
}

/// The verdict on `cmd`, read in order from `at.cwd`.
pub fn judge(cmd: &str, at: &Where) -> Verdict {
    // `:(){ :|:& };:` — read before the walk splits it into colons.
    let bomb = cmd.contains(":(){") || cmd.contains(":() {");
    let mut state = State {
        dir: Dir::Known(normalize(at.cwd)),
        vars: Vec::new(),
        home: at.home.to_path_buf(),
        place: at.place.map(normalize),
        depth: 0,
    };
    let v = judge_in(cmd, &mut state);
    if bomb {
        return Verdict::Ask("is a fork bomb".into()).or(v);
    }
    v
}

/// The effective directory as the walk knows it.
#[derive(Clone, Debug)]
enum Dir {
    Known(PathBuf),
    /// `cd "$d"`, `cd $(...)`: the walk cannot say where it is.
    Unknown,
}

struct State {
    dir: Dir,
    vars: Vec<(String, String)>,
    home: PathBuf,
    place: Option<PathBuf>,
    depth: usize,
}

fn judge_in(cmd: &str, st: &mut State) -> Verdict {
    if st.depth > 4 {
        return Verdict::Run;
    }
    let mut verdict = Verdict::Run;
    for segment in segments(cmd) {
        verdict = verdict.or(judge_segment(&segment, st));
    }
    verdict
}

fn judge_segment(words: &[Word], st: &mut State) -> Verdict {
    // `d=/` carries; `X=1 rm ...` is a prefix.
    let mut i = 0;
    while i < words.len() {
        let w = &words[i];
        if let Some((name, value)) = w.text.split_once('=')
            && !name.is_empty()
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && !name.starts_with(|c: char| c.is_ascii_digit())
        {
            let value = expand(value, st);
            st.vars.retain(|(n, _)| n != name);
            st.vars.push((name.to_string(), value));
            i += 1;
            continue;
        }
        break;
    }
    let words = &words[i..];
    let Some(first) = words.first() else {
        return Verdict::Run;
    };
    // `/bin/rm`, `command rm`, `exec rm`, `nohup rm`, `env rm`, `time rm`.
    let mut cmd_word = base(&first.text);
    let mut rest = &words[1..];
    while matches!(
        cmd_word,
        "command"
            | "exec"
            | "nohup"
            | "env"
            | "time"
            | "nice"
            | "ionice"
            | "builtin"
            | "timeout"
            | "stdbuf"
    ) {
        // `timeout 30 rm`, `nice -n 5 rm`: skip the wrapper's own arguments.
        let mut k = 0;
        while k < rest.len() && (rest[k].text.starts_with('-') || (cmd_word == "timeout" && k == 0))
        {
            k += 1;
        }
        let Some(next) = rest.get(k) else {
            return Verdict::Run;
        };
        cmd_word = base(&next.text);
        rest = &rest[k + 1..];
    }
    match cmd_word {
        "cd" | "pushd" => {
            st.dir = match rest
                .iter()
                .find(|w| !w.text.starts_with('-') || w.text == "-")
            {
                None => Dir::Known(st.home.clone()),
                Some(w) if w.text == "-" => Dir::Unknown,
                Some(w) => match resolve(&w.text, st) {
                    Some(p) => Dir::Known(p),
                    None => Dir::Unknown,
                },
            };
            Verdict::Run
        }
        "sudo" | "doas" => {
            // Escalation asks; what it escalates is judged on its own.
            let mut k = 0;
            while k < rest.len() && rest[k].text.starts_with('-') {
                k += 1;
            }
            let inner = judge_segment(&rest[k..], st);
            Verdict::Ask("escalates with sudo".into()).or(inner)
        }
        "sh" | "bash" | "zsh" | "dash" | "ksh" => {
            // `sh -c '...'`: the string is a command in the same directory.
            let mut saw_c = false;
            for w in rest {
                if w.text.starts_with('-') && w.text.contains('c') && !w.text.starts_with("--") {
                    saw_c = true;
                    continue;
                }
                if saw_c {
                    st.depth += 1;
                    let v = judge_in(&w.text, st);
                    st.depth -= 1;
                    return v;
                }
            }
            Verdict::Run
        }
        "eval" => {
            let joined: Vec<String> = rest.iter().map(|w| w.text.clone()).collect();
            st.depth += 1;
            let v = judge_in(&joined.join(" "), st);
            st.depth -= 1;
            v
        }
        "rm" => judge_rm(rest, st),
        "find" => judge_find(rest, st),
        "xargs" => {
            // `... | xargs rm -rf`: the targets come from the pipe, which
            // this walk does not see; `find`/`ls` on a tree upstream is
            // caught at the `find`. A bare `xargs rm -r` asks.
            let mut k = 0;
            while k < rest.len() && rest[k].text.starts_with('-') {
                k += 1;
            }
            match rest.get(k).map(|w| base(&w.text)) {
                Some("rm") if rest[k + 1..].iter().any(|w| is_recursive_flag(&w.text)) => {
                    Verdict::Ask("removes recursively whatever the pipe names".into())
                }
                _ => Verdict::Run,
            }
        }
        "mkfs" | "mkswap" | "wipefs" => Verdict::Ask("formats a disk".into()),
        w if w.starts_with("mkfs.") => Verdict::Ask("formats a disk".into()),
        "dd" if rest.iter().any(|w| w.text.starts_with("of=/dev/")) => {
            Verdict::Ask("writes a raw device".into())
        }
        w if w.starts_with(":()") => Verdict::Ask("a fork bomb".into()),
        _ => {
            // `:(){ :|:& };:` with the braces split off.
            if first.text.starts_with(":(){") {
                return Verdict::Ask("a fork bomb".into());
            }
            Verdict::Run
        }
    }
}

fn is_recursive_flag(w: &str) -> bool {
    w == "-r"
        || w == "-R"
        || w == "--recursive"
        || (w.starts_with('-') && !w.starts_with("--") && (w.contains('r') || w.contains('R')))
}

fn judge_rm(args: &[Word], st: &mut State) -> Verdict {
    let mut recursive = false;
    let mut flags_done = false;
    let mut verdict = Verdict::Run;
    for w in args {
        if !flags_done {
            if w.text == "--" {
                flags_done = true;
                continue;
            }
            if w.text.starts_with('-') && !w.quoted {
                recursive |= is_recursive_flag(&w.text);
                continue;
            }
        }
        verdict = verdict.or(judge_target(&w.text, recursive, "rm", st));
    }
    verdict
}

fn judge_find(args: &[Word], st: &mut State) -> Verdict {
    let deletes = args.iter().any(|w| w.text == "-delete")
        || args.windows(2).any(|p| {
            matches!(p[0].text.as_str(), "-exec" | "-execdir" | "-ok" | "-okdir")
                && base(&p[1].text) == "rm"
        });
    if !deletes {
        return Verdict::Run;
    }
    // The starting points: the arguments before the first expression
    // word (`-name`, `!`, `(`); none means `.`.
    let mut roots: Vec<String> = Vec::new();
    for w in args {
        if w.text.starts_with('-') || w.text == "!" || w.text == "(" {
            break;
        }
        roots.push(w.text.clone());
    }
    if roots.is_empty() {
        roots.push(".".into());
    }
    // `-name '*.pyc'`, `-mtime +7`: a selection, not the tree. Under a
    // root, a home or a system tree it is still refused — a system-wide
    // delete is not an agent's job; under the place it runs.
    let selective = args.iter().any(|w| {
        matches!(
            w.text.as_str(),
            "-name"
                | "-iname"
                | "-path"
                | "-ipath"
                | "-wholename"
                | "-regex"
                | "-iregex"
                | "-mtime"
                | "-mmin"
                | "-atime"
                | "-amin"
                | "-ctime"
                | "-cmin"
                | "-newer"
                | "-newermt"
                | "-size"
                | "-empty"
                | "-user"
                | "-group"
                | "-perm"
                | "-samefile"
                | "-links"
        )
    }) || args
        .windows(2)
        .any(|p| p[0].text == "-type" && p[1].text == "f");
    let mut verdict = Verdict::Run;
    for r in roots {
        let v = judge_target(&r, true, "find -delete", st);
        if selective && matches!(v, Verdict::Ask(ref why) if why.contains("project folder")) {
            continue;
        }
        verdict = verdict.or(v);
    }
    verdict
}

/// The verdict on one removal target as spelled, resolved against the
/// effective directory. `recursive` is the `-r` of an `rm`; a wipe of a
/// tree needs it, a wipe of a root's or home's *files* asks without it.
fn judge_target(spelled: &str, recursive: bool, what: &str, st: &State) -> Verdict {
    let expanded = expand(spelled, st);
    // `*`, `.*`, `**`, `.` after the last slash mean everything in that
    // directory: the directory is the tree at stake. `*.o` is selective
    // and is not a tree.
    let whole = |tail: &str| matches!(tail, "*" | ".*" | "**" | "." | "*/");
    let (dir_part, glob) = match expanded.rsplit_once('/') {
        Some((d, tail)) if whole(tail) => (
            if d.is_empty() {
                "/".to_string()
            } else {
                d.to_string()
            },
            true,
        ),
        Some((_, tail)) if tail.contains('*') => return Verdict::Run,
        None if whole(&expanded) => (".".to_string(), true),
        None if expanded.contains('*') => return Verdict::Run,
        _ => (expanded.clone(), false),
    };
    // `.*` and `*` alike mean "everything here".
    let resolved = match resolve(&dir_part, st) {
        Some(p) => p,
        None => {
            // Unknown directory and a target that means "here": the
            // kernel cannot say what this removes.
            let here = glob || matches!(dir_part.as_str(), "." | "./" | "$PWD");
            if here && (recursive || what != "rm") {
                return Verdict::Ask(format!(
                    "{what} of `{spelled}` from a directory the kernel cannot resolve (a `cd` to a variable or a command's output)"
                ));
            }
            return Verdict::Run;
        }
    };
    let tree = tree_of(&resolved, st);
    let Some(tree) = tree else {
        return Verdict::Run;
    };
    let how = if glob {
        format!("`{spelled}` (everything under {})", resolved.display())
    } else {
        format!("`{spelled}` ({})", resolved.display())
    };
    match tree {
        Tree::Root | Tree::Home | Tree::System => {
            if recursive || glob {
                Verdict::Refuse(format!(
                    "{what} of {how} removes {} — the kernel never runs a removal of the filesystem root, a home directory, or a top-level system tree, in any mode. Name the exact files or folder to remove.",
                    tree.name(&resolved)
                ))
            } else {
                Verdict::Ask(format!(
                    "{what} of {how} removes the files of {}",
                    tree.name(&resolved)
                ))
            }
        }
        Tree::Place => Verdict::Ask(format!(
            "{what} of {how} removes the project folder itself, its `.arbos` record with it"
        )),
        Tree::HomeFolder => {
            if recursive || glob {
                Verdict::Ask(format!("{what} of {how} removes {}", tree.name(&resolved)))
            } else {
                Verdict::Run
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Tree {
    Root,
    Home,
    System,
    Place,
    /// A top-level folder of the home: Documents, Projects.
    HomeFolder,
}

impl Tree {
    fn name(&self, p: &Path) -> String {
        match self {
            Tree::Root => "everything under / (the filesystem root)".into(),
            Tree::Home => format!("the home directory {}", p.display()),
            Tree::System => format!("the system tree {}", p.display()),
            Tree::Place => format!("the project folder {}", p.display()),
            Tree::HomeFolder => format!("the home folder {}", p.display()),
        }
    }
}

/// Which protected tree a resolved path is, if any. `/tmp`, `/var/...`
/// and everything deeper than one level (other than a home or the place)
/// are ordinary paths.
fn tree_of(p: &Path, st: &State) -> Option<Tree> {
    let comps: Vec<&std::ffi::OsStr> = p
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s),
            _ => None,
        })
        .collect();
    if comps.is_empty() {
        return Some(Tree::Root);
    }
    let home = normalize(&st.home);
    if p == home {
        return Some(Tree::Home);
    }
    // Another user's home: /home/<x>, /Users/<x>.
    if comps.len() == 2 && matches!(comps[0].to_str(), Some("home") | Some("Users")) {
        return Some(Tree::Home);
    }
    if comps.len() == 1 {
        return match comps[0].to_str() {
            Some("tmp") => None,
            _ => Some(Tree::System),
        };
    }
    if let Some(place) = &st.place
        && (place == p || place.starts_with(p))
    {
        // The place, or a folder that holds it.
        return Some(Tree::Place);
    }
    // A top-level folder of the home — Documents, Projects, code — is a
    // person's tree; their dot-folders (.cache, .npm) are cleanups.
    if let Ok(rel) = p.strip_prefix(&home)
        && rel.components().count() == 1
        && !rel.to_string_lossy().starts_with('.')
    {
        return Some(Tree::HomeFolder);
    }
    None
}

/// `$HOME`, `${HOME}`, `~`, `$PWD`, and simple variables the command set.
fn expand(word: &str, st: &State) -> String {
    let mut s = word.to_string();
    if s == "~" || s.starts_with("~/") {
        s = format!("{}{}", st.home.display(), &s[1..]);
    } else if let Some(rest) = s.strip_prefix('~')
        && rest
            .chars()
            .take_while(|c| *c != '/')
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        && !rest.is_empty()
    {
        // `~alice`: a home, whoever's.
        let (user, tail) = rest.split_once('/').unwrap_or((rest, ""));
        let base = if cfg!(target_os = "macos") {
            "/Users"
        } else {
            "/home"
        };
        s = format!(
            "{base}/{user}{}",
            if tail.is_empty() {
                String::new()
            } else {
                format!("/{tail}")
            }
        );
    }
    let pwd = match &st.dir {
        Dir::Known(p) => Some(p.display().to_string()),
        Dir::Unknown => None,
    };
    for (name, value) in [
        ("HOME", Some(st.home.display().to_string())),
        ("PWD", pwd.clone()),
    ]
    .into_iter()
    .chain(st.vars.iter().map(|(n, v)| (n.as_str(), Some(v.clone()))))
    {
        for form in [format!("${{{name}}}"), format!("${name}")] {
            if s.contains(&form) {
                match &value {
                    Some(v) => s = s.replace(&form, v),
                    None => return "$?unknown".into(),
                }
            }
        }
    }
    s
}

/// The absolute, lexically normalized path a spelled directory means from
/// the effective directory, or None when it depends on something the walk
/// cannot know.
fn resolve(spelled: &str, st: &State) -> Option<PathBuf> {
    let s = expand(spelled, st);
    if s.contains('$') || s.contains('`') || s.contains("$(") {
        return None;
    }
    let p = Path::new(&s);
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        match &st.dir {
            Dir::Known(d) => d.join(p),
            Dir::Unknown => return None,
        }
    };
    Some(normalize(&abs))
}

/// `a/./b/../c` → `a/c`, without touching the filesystem. A `..` above
/// the root stays at the root.
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::RootDir => out.push("/"),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
                if out.as_os_str().is_empty() {
                    out.push("/");
                }
            }
            Component::Normal(s) => out.push(s),
            Component::Prefix(_) => {}
        }
    }
    if out.as_os_str().is_empty() {
        out.push("/");
    }
    out
}

/// The seconds a command spends waiting in `sleep`, read the way the
/// wipe guard reads a removal: through paths (`/bin/sleep`), wrappers
/// (`timeout 20 sleep 8`, `nohup`, `env`), `sh -c '…'`, `eval`, and
/// `&&`/`;` chains (`true && sleep 8`). A sleep inside a loop (`while …;
/// do sleep 1; done`) is a poll that never ends on its own and counts as
/// an hour. `None` when the command has no sleep at all.
///
/// The first version read the literal fragment (`sleep N` as the whole
/// command) and let `sh -c 'sleep 8'` and `/bin/sleep 8` through — the
/// wipe guard's own shape before #410, on a model just told not to sleep.
pub fn sleep_wait_secs(cmd: &str) -> Option<u64> {
    let mut total: Option<u64> = None;
    let mut in_loop = false;
    // How many times the loops run, when every one is a `for` over a
    // literal list: `for i in 1 2 3` is three passes. A `while`/`until`,
    // or a `for` over an expansion (`$(seq 1 60)`, `*`, `{1..60}`), has no
    // count a reader can see and stays a poll. A bounded loop with a
    // `sleep 1` in it is three seconds of script, not a wait on workers
    // (QA co-03: `for i in 1 2 3; do sleep 1; done; echo looped` was
    // refused while a worker ran).
    let mut passes: Option<u64> = Some(1);
    for words in segments(cmd) {
        let mut i = 0;
        // Loop and branch keywords open a segment; the command follows.
        while i < words.len()
            && matches!(
                words[i].text.as_str(),
                "do" | "then" | "else" | "!" | "{" | "while" | "until" | "for" | "if"
            )
        {
            match words[i].text.as_str() {
                "while" | "until" => {
                    in_loop = true;
                    passes = None;
                }
                "for" => {
                    in_loop = true;
                    passes = match (passes, for_list_len(&words[i + 1..])) {
                        (Some(p), Some(n)) => Some(p.saturating_mul(n)),
                        _ => None,
                    };
                }
                _ => {}
            }
            i += 1;
        }
        // `X=1 sleep 8`
        while i < words.len() && words[i].text.contains('=') && !words[i].text.starts_with('-') {
            i += 1;
        }
        let Some(first) = words.get(i) else { continue };
        let mut cmd_word = base(&first.text);
        let mut rest = &words[i + 1..];
        while matches!(
            cmd_word,
            "command"
                | "exec"
                | "nohup"
                | "env"
                | "time"
                | "nice"
                | "ionice"
                | "builtin"
                | "timeout"
                | "stdbuf"
                | "sudo"
                | "doas"
        ) {
            let mut k = 0;
            while k < rest.len()
                && (rest[k].text.starts_with('-') || (cmd_word == "timeout" && k == 0))
            {
                k += 1;
            }
            let Some(next) = rest.get(k) else { break };
            cmd_word = base(&next.text);
            rest = &rest[k + 1..];
        }
        let found = match cmd_word {
            "sleep" => rest.first().and_then(|w| parse_sleep(&w.text)),
            "sh" | "bash" | "zsh" | "dash" | "ksh" => {
                let mut saw_c = false;
                let mut inner = None;
                for w in rest {
                    if w.text.starts_with('-') && w.text.contains('c') && !w.text.starts_with("--")
                    {
                        saw_c = true;
                        continue;
                    }
                    if saw_c {
                        inner = sleep_wait_secs(&w.text);
                        break;
                    }
                }
                inner
            }
            "eval" => {
                let joined: Vec<String> = rest.iter().map(|w| w.text.clone()).collect();
                sleep_wait_secs(&joined.join(" "))
            }
            _ => None,
        };
        if let Some(s) = found {
            total = Some(total.unwrap_or(0).saturating_add(s));
        }
    }
    match (total, passes) {
        (Some(s), Some(n)) if in_loop => Some(s.saturating_mul(n)),
        (Some(s), None) if in_loop => Some(s.max(3600)),
        (other, _) => other,
    }
}

/// The items of `for <name> in <items…>` when every item is a literal
/// word; `None` for a list a reader cannot count (an expansion, a glob, a
/// brace range, a `for ((…))`, or no `in` at all, which loops over `$@`).
fn for_list_len(words: &[Word]) -> Option<u64> {
    let mut it = words.iter();
    let name = it.next()?;
    if name.text.starts_with("((") {
        return None;
    }
    if it.next().map(|w| w.text.as_str()) != Some("in") {
        return None;
    }
    let mut n = 0u64;
    for w in it {
        if w.quoted {
            n += 1;
            continue;
        }
        if w.text.contains(['$', '*', '?', '`', '{', '[']) {
            return None;
        }
        n += 1;
    }
    Some(n)
}

/// `75`, `0.5`, `2m`, `1h` as whole seconds.
fn parse_sleep(n: &str) -> Option<u64> {
    let cut = n.trim_end_matches(|c: char| c.is_ascii_alphabetic()).len();
    let (num, unit) = n.split_at(cut);
    let v: f64 = num.parse().ok()?;
    let mult = match unit {
        "" | "s" => 1.0,
        "m" => 60.0,
        "h" => 3600.0,
        "d" => 86400.0,
        _ => return None,
    };
    Some((v * mult) as u64)
}

fn base(w: &str) -> &str {
    w.rsplit('/').next().unwrap_or(w)
}

/// One shell word after quote removal.
#[derive(Debug, Clone)]
struct Word {
    text: String,
    /// Any part of it was quoted: `'-rf'` is a file name, not a flag.
    quoted: bool,
}

/// The command as a list of simple commands, split at `&&`, `||`, `;`,
/// `|`, `&`, newlines and subshell parentheses, each a list of words with
/// quotes removed. Quoting keeps operators inside a word (`sh -c "cd /
/// && rm -rf *"` is one word to the outer walk).
fn segments(cmd: &str) -> Vec<Vec<Word>> {
    let mut out: Vec<Vec<Word>> = Vec::new();
    let mut cur: Vec<Word> = Vec::new();
    let mut word = String::new();
    let mut quoted = false;
    let mut in_word = false;
    let chars: Vec<char> = cmd.chars().collect();
    let mut i = 0;
    let flush_word =
        |word: &mut String, quoted: &mut bool, in_word: &mut bool, cur: &mut Vec<Word>| {
            if *in_word {
                cur.push(Word {
                    text: std::mem::take(word),
                    quoted: *quoted,
                });
            }
            *quoted = false;
            *in_word = false;
        };
    let flush_seg = |cur: &mut Vec<Word>, out: &mut Vec<Vec<Word>>| {
        if !cur.is_empty() {
            out.push(std::mem::take(cur));
        }
    };
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\'' => {
                in_word = true;
                quoted = true;
                i += 1;
                while i < chars.len() && chars[i] != '\'' {
                    word.push(chars[i]);
                    i += 1;
                }
            }
            '"' => {
                in_word = true;
                quoted = true;
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    if chars[i] == '\\' && i + 1 < chars.len() {
                        i += 1;
                    }
                    word.push(chars[i]);
                    i += 1;
                }
            }
            '\\' if i + 1 < chars.len() => {
                in_word = true;
                word.push(chars[i + 1]);
                i += 1;
            }
            ' ' | '\t' => flush_word(&mut word, &mut quoted, &mut in_word, &mut cur),
            // `{ ...; }` groups split; `${PWD}` and `{a,b}` are part of a word.
            '{' if !in_word && (i + 1 >= chars.len() || chars[i + 1].is_whitespace()) => {
                flush_seg(&mut cur, &mut out);
            }
            '}' if !in_word => {
                flush_seg(&mut cur, &mut out);
            }
            '\n' | ';' | '|' | '&' | '(' | ')' => {
                flush_word(&mut word, &mut quoted, &mut in_word, &mut cur);
                flush_seg(&mut cur, &mut out);
                // `&&`, `||` are one operator.
                if (c == '&' || c == '|') && i + 1 < chars.len() && chars[i + 1] == c {
                    i += 1;
                }
            }
            _ => {
                in_word = true;
                word.push(c);
            }
        }
        i += 1;
    }
    flush_word(&mut word, &mut quoted, &mut in_word, &mut cur);
    flush_seg(&mut cur, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at<'a>(cwd: &'a Path, home: &'a Path, place: Option<&'a Path>) -> Where<'a> {
        Where { cwd, home, place }
    }

    fn v(cmd: &str) -> Verdict {
        judge(
            cmd,
            &at(
                Path::new("/home/jacob/code/app"),
                Path::new("/home/jacob"),
                Some(Path::new("/home/jacob/code/app")),
            ),
        )
    }

    fn refused(cmd: &str) {
        match v(cmd) {
            Verdict::Refuse(why) => assert!(
                why.contains("never runs a removal") || why.contains("removes"),
                "{cmd:?}: {why}"
            ),
            other => panic!("{cmd:?} must be refused in every mode, got {other:?}"),
        }
    }

    fn asks(cmd: &str) {
        assert!(
            matches!(v(cmd), Verdict::Ask(_)),
            "{cmd:?} must ask, got {:?}",
            v(cmd)
        );
    }

    fn runs(cmd: &str) {
        assert_eq!(v(cmd), Verdict::Run, "{cmd:?} must run");
    }

    /// qal-j15's list, one spelling per line, every one refused.
    #[test]
    fn a_cd_then_a_removal_of_here_is_read_as_the_tree_it_is() {
        refused("cd / && rm -rf *");
        refused("cd /; rm -rf ./*");
        refused("cd / ; rm -rf ./*");
        refused("cd /usr && rm -rf *");
        refused("pushd / && rm -rf *");
        refused("cd / && rm -rf -- *");
        refused("cd / && rm -rf .");
        refused("cd / && rm -rf .*");
        refused("cd /\nrm -rf *");
        refused("cd / && ls && rm -rf *");
        refused("cd /etc && cd .. && rm -rf *");
        refused("cd /tmp && cd / && rm -rf *");
        refused("cd /tmp/x && cd ../.. && rm -rf *");
    }

    #[test]
    fn pwd_home_and_tilde_spellings_are_refused() {
        refused("cd / && rm -rf \"$PWD\"/*");
        refused("cd / && rm -rf $PWD/*");
        refused("cd / && rm -rf ${PWD}/*");
        refused("rm -rf ~");
        refused("rm -rf ~/");
        refused("rm -rf ~/*");
        refused("rm -rf $HOME");
        refused("rm -rf \"$HOME\"/*");
        refused("rm -rf ${HOME}/");
        refused("cd ~ && rm -rf *");
        refused("cd && rm -rf *");
        refused("cd $HOME && rm -rf .");
        refused("rm -rf /home/jacob");
        refused("rm -rf /Users/jacob/*");
        refused("rm -rf ~alice");
        // Everything in a home or the root, recursive or not: the tree's files.
        refused("cd / && rm *");
        refused("rm -f ~/*");
        refused("rm -rf /home/jacob/code/app/../..");
    }

    #[test]
    fn roots_and_system_trees_are_refused_however_spelled() {
        refused("rm -rf /");
        refused("rm -rf /*");
        refused("rm -rf //");
        refused("rm -rf /usr");
        refused("rm -rf /etc/");
        refused("rm -r --no-preserve-root /etc");
        refused("rm -rf /var");
        refused("rm -rf /home");
        refused("rm -rf /Users");
        refused("cd /testbed && rm -rf /usr");
        refused("/bin/rm -rf /");
        refused("command rm -rf /");
        refused("exec rm -rf /");
        refused("nohup rm -rf / &");
        refused("timeout 60 rm -rf /");
        refused("rm -rf ./../../../../..");
        refused("rm -fr /");
        refused("rm -Rf /");
        refused("rm --recursive --force /");
        refused("d=/; rm -rf $d/*");
        refused("d=/; cd $d && rm -rf *");
    }

    #[test]
    fn find_delete_and_shell_wrappers_are_read_through() {
        refused("find / -delete");
        refused("find / -type f -delete");
        refused("find / -exec rm -rf {} +");
        refused("find / -name '*.log' -exec rm {} \\;");
        refused("cd / && find . -delete");
        refused("find ~ -delete");
        refused("find $HOME -type f -delete");
        refused("sh -c 'cd / && rm -rf *'");
        refused("bash -c \"cd / && rm -rf *\"");
        refused("bash -lc 'rm -rf ~'");
        refused("sh -c 'sh -c \"cd / && rm -rf *\"'");
        refused("eval \"cd / && rm -rf *\"");
        refused("eval cd / '&&' rm -rf '*'");
        refused("(cd / && rm -rf *)");
        refused("{ cd / && rm -rf *; }");
        refused("sudo rm -rf /");
        refused("sudo -n rm -rf ~");
        refused("true || (cd / && rm -rf *)");
        refused("echo x | (cd / && rm -rf *)");
    }

    #[test]
    fn a_removal_the_kernel_cannot_place_asks_and_sudo_mkfs_and_a_fork_bomb_still_ask() {
        asks("cd $DIR && rm -rf *");
        asks("cd \"$(mktemp -d)\" && rm -rf *");
        asks("cd - && rm -rf *");
        asks("find . -delete && cd $x");
        asks("cd $x && find . -delete");
        asks("rm -rf $UNSET/*");
        asks("ls / | xargs rm -rf");
        asks("sudo apt-get install x");
        asks("mkfs.ext4 /dev/sda1");
        asks("dd if=/dev/zero of=/dev/sda");
        asks(":(){ :|:& };:");
        // A single file at the root or home, not the tree: the ask.
        asks("rm /vmlinuz");
        asks("rm -f ~");
        // The project folder itself goes through a card, not silently.
        asks("cd /home/jacob/code/app && rm -rf .");
        asks("rm -rf ./*");
        asks("rm -rf *");
        asks("rm -rf /home/jacob/code/app");
        asks("cd .. && rm -rf app");
        // A folder that holds the place, and a person's top-level home folders.
        asks("rm -rf ~/code");
        asks("rm -rf ~/Documents");
        asks("rm -rf $HOME/Projects/*");
        asks("cd ~ && rm -rf Documents");
    }

    #[test]
    fn ordinary_cleanups_run() {
        runs("rm -rf /tmp/udltest && cd /testbed && git diff");
        runs("rm -rf build/ dist/");
        runs("rm -rf /var/tmp/x");
        runs("rm -rf /testbed/.pytest_cache");
        runs("rm -f /tmp/a.txt");
        runs("grep -r foo /");
        runs("python -c 'print(1)'");
        runs("rm -rf ./build");
        runs("cd /tmp/x && rm -rf *");
        runs("cd /tmp && rm -rf *");
        runs("rm -rf /tmp/*");
        runs("cd build && rm -rf *");
        runs("cd /home/jacob/code/app/target && rm -rf *");
        runs("rm -rf ~/code/app/target");
        runs("rm -rf $HOME/.cache/thing");
        runs("rm -rf ~/.cache");
        runs("rm -rf ~/Downloads/build.zip");
        runs("rm -rf ~/Documents/old-notes");
        runs("rm ~/Documents");
        runs("cd / && ls -la");
        runs("cd / && rm -rf /tmp/scratch");
        runs("find . -name '*.pyc' -delete");
        runs("find /tmp -mtime +7 -delete");
        runs("find build -type f -exec rm {} +");
        runs("git ls-files | xargs rm");
        runs("rm -- -rf");
        runs("rm '-rf' ./build");
        runs("echo 'cd / && rm -rf *'");
        runs("cat notes.md | grep 'rm -rf /'");
        runs("sh -c 'rm -rf build'");
        runs("rm -rf *.o");
        runs("cd src && rm -rf *.o");
        runs("d=build; rm -rf $d/*");
        runs("cd /usr/local/src/thing && rm -rf *");
    }

    #[test]
    fn the_cwd_of_the_call_is_the_starting_point() {
        let root = Path::new("/");
        let home = Path::new("/home/jacob");
        assert!(matches!(
            judge("rm -rf *", &at(root, home, None)),
            Verdict::Refuse(_)
        ));
        assert!(matches!(
            judge("rm -rf .", &at(home, home, None)),
            Verdict::Refuse(_)
        ));
        assert!(matches!(
            judge("find . -delete", &at(root, home, None)),
            Verdict::Refuse(_)
        ));
        assert_eq!(
            judge("rm -rf *", &at(Path::new("/tmp/scratch"), home, None)),
            Verdict::Run
        );
    }

    /// The sleep guard reads what the command will do, not the fragment
    /// (`co-*`, 2026-09-17: `sh -c 'sleep 8'`, `/bin/sleep 8`, `timeout 20
    /// sleep 8` and `true && sleep 8` all ran with a worker live).
    #[test]
    fn a_sleep_is_found_through_paths_wrappers_shells_and_chains() {
        let s = sleep_wait_secs;
        assert_eq!(s("sleep 75"), Some(75));
        assert_eq!(s("sleep 75; echo waited"), Some(75));
        assert_eq!(s("sleep 2m && ls"), Some(120));
        assert_eq!(s("/bin/sleep 8"), Some(8));
        assert_eq!(s("/usr/bin/sleep 8"), Some(8));
        assert_eq!(s("timeout 20 sleep 8"), Some(8));
        assert_eq!(s("nohup sleep 8 &"), Some(8));
        assert_eq!(s("env sleep 8"), Some(8));
        assert_eq!(s("command sleep 8"), Some(8));
        assert_eq!(s("true && sleep 8"), Some(8));
        assert_eq!(s("echo x; sleep 30"), Some(30));
        assert_eq!(s("sh -c 'sleep 8'"), Some(8));
        assert_eq!(s("bash -lc \"sleep 8; echo x\""), Some(8));
        assert_eq!(s("sh -c 'sh -c \"sleep 8\"'"), Some(8));
        assert_eq!(s("eval sleep 8"), Some(8));
        assert_eq!(s("sleep 3; sleep 3"), Some(6));
        assert_eq!(s("X=1 sleep 8"), Some(8));
        // A sleep in a loop is a poll: an hour, however short the sleep.
        assert!(s("while :; do sleep 1; done").unwrap() >= 3600);
        assert!(s("for i in $(seq 1 60); do echo poll $i; sleep 1; done").unwrap() >= 3600);
        assert!(s("until grep -q done out.log; do sleep 2; done").unwrap() >= 3600);
        // A loop a reader can count is that many passes, not a poll
        // (QA co-03): three seconds of script ran while a worker was up.
        assert_eq!(s("for i in 1 2 3; do sleep 1; done; echo looped"), Some(3));
        assert_eq!(s("for f in a b; do sleep 2; sleep 1; done"), Some(6));
        assert_eq!(
            s("for i in 1 2 3 4 5 6 7 8 9 10; do sleep 1; done"),
            Some(10)
        );
        // Lists a reader cannot count stay a poll.
        assert!(s("for i in {1..60}; do sleep 1; done").unwrap() >= 3600);
        assert!(s("for f in *.log; do sleep 1; done").unwrap() >= 3600);
        // A quoted expansion is one word, so one pass; unquoted, it is
        // as many as the shell finds.
        assert_eq!(s("for x in \"$LIST\"; do sleep 1; done"), Some(1));
        assert!(s("for x in $LIST; do sleep 1; done").unwrap() >= 3600);
        assert!(s("for i; do sleep 1; done").unwrap() >= 3600);
        assert!(s("for ((i=0;i<9;i++)); do sleep 1; done").unwrap() >= 3600);
        // No sleep: nothing.
        assert_eq!(s("ls -la"), None);
        assert_eq!(s("cargo build"), None);
        assert_eq!(s("echo 'sleep 8'"), None);
        assert_eq!(s("python3 -c 'import time; time.sleep(30)'"), None);
        assert_eq!(s("sleep"), None);
    }

    #[test]
    fn the_reason_names_the_spelling_and_the_tree() {
        let Verdict::Refuse(why) = v("cd / && rm -rf *") else {
            panic!()
        };
        assert!(why.contains("`*`"), "{why}");
        assert!(why.contains("everything under /"), "{why}");
        assert!(why.contains("in any mode"), "{why}");
        let Verdict::Refuse(why) = v("rm -rf ~") else {
            panic!()
        };
        assert!(why.contains("/home/jacob"), "{why}");
    }
}
