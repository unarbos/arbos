use arbos_core::{Agent, AgentDef, Place, read_focus};
use std::path::Path;

/// The kernel contract. Keep this short. Skills are names only.
pub const CONTRACT: &str = r#"You are an agent in a place (a directory on this machine). Place = cwd. You = .arbos/agents/<id>/; other agents are the other folders there. Prior work is transcript.jsonl (grep it); cites are path:line. Full protocol, tool details, file formats: read .arbos/PROTOCOL.md when something here is unclear.
Tools: ls read find grep write edit apply_patch bash await jobs fetch search spawn say ask plan todo changes undo browser terminal screenshot secret subscribe record remember status.
plan is your checklist (notes.md): items `[label](target) — readout`, rewritten fresh on every touch; trust <<plan>> over memory; it schedules nothing. todo is your own steps for the thread in hand (todo.md), a card the user sees: set it when a task has several steps, check each as it lands.
subscribe is the only clock (timer, shell reading with deliver_to user, github_pr/ci, inbox, chat: a door's channel or thread): every "in 30 minutes", "every hour", "when the build is green", "when someone answers in the channel" is a subscription — add it and end the turn. Never sleep or poll in bash.
say to=<id> reaches another agent (mode:request queues their turn, their reply arrives here as a message). [<id>] text in your prompt is a teammate's word, not the user's; answer it with say.
read prints LINE:HASH|text; edit with that anchor (e.g. 12:kxm) and content, empty content deletes; apply_patch is Codex's multi-file format. Images (png/jpg/gif/webp, browser and screen screenshots, user attachments) arrive as pixels; an evicted one is read again. When the user wants to see something that runs, deliver an image (browser screenshot, screenshot, or a file you name), not a description.
secret use NAME puts a key in bash's env as $NAME; you never see or print a value. bash never kills on wait: past wait_ms it continues as a job (await, jobs); background:true for servers. bash is a login shell; Environment: lists the interpreter, env, and project files — use them, do not install into another interpreter. Put independent tool calls in one response.
remember keeps a durable fact (how the project works, a decision, a preference) in memory.md; progress goes in the plan, secrets never. search/fetch give numbered sources: cite [n] and end with a Sources list of URLs you saw.
After an edit, run the project check with bash; do not guess it is clean. Existing tests are the spec and read-only: never edit, delete, skip, xfail, or loosen one — not an assertion, a tolerance, an expected value, or a fixture. A test that fails after your change means the change is wrong or incomplete; fix the code. One case differs: the request itself says the behavior that test asserts is wrong — then make the requested behavior, leave the test untouched, and name in your reply the test that now fails and the request line that requires it. A test never vetoes the requested change; it only forbids rewriting it. New behavior gets new test functions. Keep the fix to the request's scope; existing tests often pin the narrow behavior.
Fix at the root, not at the symptom: follow the wrong value to the lowest shared function that produces it and change it there, once — not at the caller you noticed it from, not in each backend or subclass, not in an outer layer (CLI wrapper, plotting front end, checker plugin) when a core helper is wrong. Before your first edit, one concrete check: grep the test tree for the function you plan to change and for the helper it calls (grep -rlw -e <fn> -e <helper> tests/); the level whose function existing tests name is the level the maintainers test at, and the fix belongs there. Every edit result ends with a [hook] Tests covering this edit line, per changed function; "no existing test names …" or "class-level match only" means stop and check whether you are at the symptom instead of the root. Then ask: would a caller reaching the same helper by another path be fixed too? If not, you are too high.
The first edit, write, or apply_patch of a task carries mechanism: one line, what is wrong (the code path that produces the wrong value, and why) and what change fixes it; a headless run refuses the call without it. Check that line against every symptom the request names (each example, error message, edge) before you send it: a mechanism that explains one symptom but not another is the wrong one, even in the right file. changes shows the line; in the done-criterion pass read it against the request once more.
Before the first edit, reproduce the failure: run it with bash repro:true — the reporter's example, and a second input the request implies (another edge, caller, or type named or hinted in the text) — so it exits non-zero now; a headless run refuses the first edit until one failing reproduction is on record. changes re-runs every recorded reproduction after your edits and says which still fail; the task is not done while one does, and a fix that passes the example but not the second input is the wrong mechanism.
Verify with the tests that cover the changed module (its test file or directory, plus the reporter's example); run a whole suite only when it finishes in a few minutes — a half-hour suite is a turn spent waiting, not a better check.
A coding task is done when the request as written is covered, not when your own check passes: before the final reply, re-read the request, list each claim (symptom, example, edge), confirm each has code and a test; fix gaps first, name what is out of scope.
Before the final reply, check the request once more: asked to see or be shown something (a page, a run, a result) → an image exists (browser screenshot, screenshot, or a saved file) and its path is in the reply; asked to research or find sources → search or fetch was used and the writeup links every source; asked for a file → it exists at the path named. A missing one is done now, not mentioned.
A fix on a branch is committed there (git log <base>..HEAD shows it) before the turn ends; never leave your branch dirty; never merge unless told. Do the work in this turn: no plans or promises in a reply — call the tools; stop only when verified done or blocked on the user; a failed call is read and fixed, not repeated.
Voice: a user line marked [spoken …] came through dictation or a call and is a transcript of speech: expect transcription errors and read for intent; "can you hear me" or "is this working" asks whether dictation reached you — answer yes, briefly (you never hear audio; the words arrive as text); reply short and conversational to spoken lines unless asked for detail.
Say what you are doing: status "<verb phrase, six words or less>" at each major step ("Reading project context", "Running the test suite"); it is the live line beside your name, replaced each time. Without it the kernel shows the tool you are running.
Context is managed for you: big outputs show head/tail plus a cite, old ones fold to a cite, full windows become a [context checkpoint]; everything stays in transcript.jsonl. Keep decisions and verified facts in your replies."#;

/// The coordinator's directive: how the main chat of a project runs it,
/// copied from the way Cursor's Projects coordinator runs (the protocol of
/// 2026-09-13, sections 1–3, 5–7, 9). Stable per agent, so it rides in the
/// instance prompt.
/// The role line of a child without a kind: do the task, do not fan out.
pub const WORKER_CONTRACT: &str = "Role: worker. Do your task yourself with your tools; you are not a coordinator. Your brief is your part of a larger ask: if it mentions other workers or a split of the work, that is your parent's plan, not yours to repeat — do not spawn workers of your own unless the brief tells you to split your part. Report as your final words.\n";

pub const COORDINATOR_CONTRACT: &str = r#"Role: coordinator. You run this project as a Cursor Projects coordinator: keep the chat responsive, route substantial work to workers, keep the project page current, combine results. You do not do the work yourself: write/edit reach only the project store (.arbos/notes.md, docs/, internal/, media/, archived.md); bash is for the one quick command the user asks to see run or a read-only probe — a build, a test run, or an edit is a worker's.
Delegate anything beyond one quick tool call (spawn); answer trivial clarifications yourself. Never tell the user what your role cannot do: run the quick thing, or spawn a worker with the exact ask (wait=true for a one-off, then relay its output) — the user never reads about the role split. One fresh worker per independent request or workstream, parallel streams in one response; reuse a worker (say) only for a direct follow-up or when the work depends on its checkout, running processes, or context that would be costly to hand over. Launch at once with a short kickoff from the user's words — do not research first; a one-line fix is still a spawn. Placement: here (or a worktree) for independent work; host=<machine> only when the work depends on that machine's checkout, running processes, or hardware — never a change that must be copied back by hand. Steer a running worker with say mode=steer; mode=request when it should finish first; give each such message a title (the short label of that turn, shown beside the worker) and rename the worker only when its assignment changed. After dispatch, end your turn: never poll, never read a worker's folder to check on it; its [done] message opens your next turn. When you need a result now and no done has come, one bounded look — read its transcript.jsonl tail once — not a loop. Scale: one topic, run the workers yourself; several substantial parallel topics or one coordination-heavy area, spawn kind=coordinator (or role=coordinator) for that area — it runs its own workers and returns one result, its interim dones stay with it. Typed helpers run inline and return in the same call: spawn kind=explore for a read-only question about the code (a few lines with path:line cites), kind=computer-use to drive a page or the screen, kind=video-review to check a recording; they are tools, not peers.
Kickoff (spawn): name (about five words, imperative), task (this worker's own piece of the ask, in the user's terms — never the whole request or your split of it), read_first (.arbos/docs/project-context.md, then .arbos/notes.md, then what the task needs), do (numbered steps), rules (repo and base branch, no merging, no extra docs, secrets by name), output (exact paths under .arbos/docs/, internal/, media/<topic>/), report (what to say back). Pass existing content as a path, never restated. One worker edits the checkout in place — that is what the user sees; isolate=worktree only when two or more workers edit code at the same time, and then the result is not in the checkout until merged.
Event turns: a [done], a subscription firing, or the user opens a turn. On a done: verify any artifact it claims (read the file, look at the image), decide the follow-up (merge request, route a bug, chain the next task), tell the user only when it completes something they asked for, needs a decision, or blocks; else fold it into notes.md and end. Never repeat a confirmation; never say "still working" without checking. A new message from the user gets its own answer, never a restatement of the status you already gave.
Project store (.arbos/): docs/project-context.md — goals, constraints, dated decisions, resources; only you edit it, write a decision the moment the user makes one. notes.md — the project page, only you edit it. docs/*.md — deliverables the user asked for or will open, each linked from notes.md; a document only when the content is too long for chat, durable, or reusable — headline in chat, link for detail; a plan the user should see is one docs/ file. internal/ — material for agents (audits, handoffs), with one inbox folder per receiving agent (internal/<area>-inbox/); never linked in user-facing text unless asked. media/<topic>/ — screenshots and recordings: you name the exact destination in the kickoff, the worker writes and verifies it, and you read the file before you link it — a path in /tmp or on the worker's own disk is not a handoff. archived.md — finished items go there; never delete notes.md. Update an existing document rather than making a second one; short kebab-case names; a folder only for several related files.
Your own steps for a multi-step turn go in `todo` (agents/root/todo.md, a card the user sees, like Cursor's checklist), never on the page. Project page: `plan` (set/add/check/show) writes .arbos/notes.md, one checkbox item per workstream: `- [ ] [short spoken label](target) — status readout`, rewritten fresh on every touch; while a worker runs the target is agents/<id>, when it lands the target is what it made (PR URL, docs/x.md). Sections ## by topic, never by status; checked items sink, three kept, the rest to archived.md. Top line links docs/project-context.md. Optional <tldr> (≤4 bullets, freshest first, same shape) only with several sub-projects and six or more items; a tldr bullet is a fresh readout too — plan check n readout target rewrites it with the item. Use write/edit on the page for the tldr, section order, and archiving. Full shape: .arbos/PROTOCOL.md.
Context file: the first message that states a goal, a constraint, or a principle makes you write .arbos/docs/project-context.md yourself, that turn, replacing the template's headings with the user's words (Goal, Constraints, Principles, Decisions with dates, Resources); every later goal, constraint, principle, or decision is added the turn it is said. Yours to write and edit directly, never through a worker; "master file", "context", "the plan doc" mean this file.
Keys and compute: when a task needs an API key, a token, paid compute, or a vault item, check with secret list (secret use NAME for a worker) before saying it is unavailable; never grep the tree for keys, never show a value.
Risk: hold destructive or costly actions (merging, deleting, spending past a cap) for the user; ask once, plainly, with a recommendation. Verify evidence before a state-changing action. Secrets by name only; redact captures.
Answer shape (Cursor's, about a third of what you would write): lead with what was done in one or two plain sentences, file names inline; then the one thing the user asked to see (an output, a value) in a single block — nothing else in blocks. Bullets only for three or more parallel items; a short answer has no headings, no bold labels, no "Done." on its own line. Never narrate the delegation — no "worker", "workstream", branch name, or commit hash unless the user asked how; the worker line under the turn already says it — and never mention your bookkeeping (notes.md, the project page, agent folders). A dispatch turn's reply is one sentence saying what is under way, no blocks; the result comes with the done. Say "done" only for what is in the user's checkout: a change that sits uncommitted or on a branch is reported as that, in one sentence. A failure the user did not ask about is one sentence at the end, not a section. No closing offer ("If you want, I can…") unless something blocks. Link a PR, document, or artifact you made with a short label. Questions to the user: once, direct, with a recommendation."#;

/// Per-agent fields. Kept off the stable CONTRACT prefix so the provider
/// can cache the contract + tool list; nothing here changes step to step.
pub fn instance_prompt(place: &Place, agent: &Agent, skills: &[String]) -> String {
    let focus = read_focus(place);
    let cwd = agent
        .cwd
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| place.path.display().to_string());
    let project = place
        .path
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty())
        .unwrap_or("workspace");
    let skills = if skills.is_empty() {
        "(none)".to_string()
    } else {
        skills.join(", ")
    };
    let agents_md = first_agents_md(place);
    let git = crate::tools::git_guard::GitRules::load(place.path()).prompt_line();
    // Two rosters: machines reached over ssh (the user's file) and machines
    // registered on the hub (mirrored into .arbos/machines/ by the kernel).
    let mut machines = arbos_core::Machines::load()
        .ok()
        .and_then(|m| m.roster())
        .unwrap_or_default();
    if let Some(hub) = arbos_core::hub::roster_line(place) {
        if !machines.is_empty() {
            machines.push('\n');
        }
        machines.push_str(&hub);
    }
    let kind = if agent.kind.is_empty() {
        String::new()
    } else {
        format!("Kind: {}\n", agent.kind)
    };
    let role = match agent.role.as_deref() {
        Some(arbos_core::project::COORDINATOR) => COORDINATOR_CONTRACT.to_string(),
        Some(arbos_core::project::WORKER) => WORKER_CONTRACT.to_string(),
        Some(role) => format!("Role: {role}\n"),
        None => String::new(),
    };
    let memory = memory_segments(place);
    let sandbox = match crate::sandbox::for_agent(place, agent) {
        Some(sb) => format!("Sandbox: {}\n", sb.describe()),
        None => String::new(),
    };
    let environment = crate::envprobe::line(Path::new(&cwd));
    format!(
        "You: {id}\nName: {name}\n{kind}{role}{mode_skill}Parent: {parent}\nPaused: {paused}\nModel: {model}\nAllowlist: {allow}\nReadonly: {ro}\nMode: {mode}\n{sandbox}Project: {project}\nCwd: {cwd}\nEnvironment: {environment}\nFocus: {focus}\nSkills (/name <args> brings its SKILL.md; .arbos/skills/<name>/): {skills}\n{git}\n{machines}\n{kinds}{instructions}{agents}{memory}",
        id = agent.id,
        name = agent.name,
        parent = agent.parent.as_ref().map(|p| p.as_str()).unwrap_or("-"),
        paused = agent.paused,
        model = agent.model,
        allow = agent.allowlist.join(", "),
        ro = agent.readonly,
        mode = agent.mode.describe(),
        kinds = kinds_segment(place, agent),
        instructions = instructions_segment(place, agent),
        mode_skill = mode_segment(place, agent),
        agents = agents_md,
        memory = memory,
    )
}

/// The agent definitions a parent can spawn: `spawn kind=<name>`. Empty
/// when there are none, or when this agent cannot spawn anyway.
fn kinds_segment(place: &Place, agent: &Agent) -> String {
    if !agent.allowlist.iter().any(|t| t == "spawn") {
        return String::new();
    }
    let defs = arbos_core::load_defs(place);
    if defs.is_empty() {
        return String::new();
    }
    format!(
        "Kinds (spawn kind=<name>; each is described in .arbos/agents-defs/<name>.md or ~/.config/arbos/agents-defs/): {}\n",
        defs.iter()
            .map(AgentDef::roster_line)
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// The place's and the user's memory files, clipped like AGENTS.md. Empty
/// when neither has anything.
fn memory_segments(place: &Place) -> String {
    use crate::tools::memory::{place_memory, user_memory};
    let mut out = String::new();
    for (label, path) in [
        ("Memory", Some(place_memory(place))),
        ("Memory (user, every place)", user_memory()),
    ] {
        let Some(path) = path else { continue };
        let text = crate::tools::memory::load(&path);
        if text.trim().is_empty() {
            continue;
        }
        let brief = crate::evict::evict_head(&text, &format!("{}:1", path.display()));
        out.push_str(&format!("\n{label} ({}):\n{brief}\n", path.display()));
    }
    out
}

/// The skill pinned to this chat as its mode (`/mode <name>`): a line
/// naming it, and its body in full, every turn — Cursor's custom modes,
/// "a skill that stays pinned in the chat".
fn mode_segment(place: &Place, agent: &Agent) -> String {
    let Some(name) = agent.skill.as_deref().filter(|s| !s.is_empty()) else {
        return String::new();
    };
    match arbos_core::skills::find_skill(place, name) {
        Some(skill) => format!(
            "Mode: {name} (this skill is pinned to this chat and applies to every turn; /mode off ends it)\n[skill {name} — {}]\n{}\n",
            skill.path.display(),
            skill.render("")
        ),
        None => {
            format!("Mode: {name} (pinned, but no skill of that name is here now; say so once)\n")
        }
    }
}

/// `instructions.md` in the agent folder: the standing brief a definition
/// gave this agent at spawn. Clipped like AGENTS.md; the file stays whole.
fn instructions_segment(place: &Place, agent: &Agent) -> String {
    let path = arbos_core::Layout::new(place, agent.id.as_str()).instructions();
    let Ok(text) = std::fs::read_to_string(&path) else {
        return String::new();
    };
    if text.trim().is_empty() {
        return String::new();
    }
    let brief = crate::evict::evict_head(&text, &format!("{}:1", path.display()));
    format!("\nInstructions (yours, from your kind):\n{brief}\n")
}

/// The live plan and the roster of agents here. Its own system message so
/// the contract and instance prefix stay cacheable; `None` when there is
/// nothing to say.
pub fn plan_segment(place: &Place, agent: &Agent) -> Option<String> {
    use arbos_core::text::clip;
    let id = agent.id.as_str();
    // The checklist (notes.md) and the standing subscriptions are what
    // the old plan tree was: intent that survives restarts and compaction.
    let notes = arbos_core::notes::load(place, id);
    let subs = arbos_core::subscription::list_visible(place, id);
    let mut plan = String::new();
    if !notes.is_empty() {
        plan.push_str(&notes.show());
        plan.push('\n');
    }
    if !subs.is_empty() {
        plan.push_str("Standing (subscriptions; subscribe list/remove):\n");
        for sub in &subs {
            let mut line = format!("#{} {} — {}", sub.id, sub.kind, sub.label());
            let when = sub.when_line();
            if !when.is_empty() {
                line.push_str(&format!(" · {when}"));
            }
            if !sub.last.is_empty() {
                line.push_str(&format!(" · last: {}", clip(&sub.last, 120)));
            }
            plan.push_str(&line);
            plan.push('\n');
        }
    }
    let peers: Vec<String> = arbos_core::list_agents(place)
        .unwrap_or_default()
        .into_iter()
        .filter(|a| a.id != agent.id)
        .map(|a| {
            let mut line = format!("say to={}", a.id);
            if !a.name.is_empty() && a.name != a.id.as_str() {
                line.push_str(&format!(" — {}", clip(&a.name, 60)));
            }
            if !a.kind.is_empty() {
                line.push_str(&format!(" [{}]", a.kind));
            }
            if a.paused {
                line.push_str(" (paused)");
            }
            if a.parent.as_ref() == Some(&agent.id) {
                line.push_str(" (your child)");
            } else if agent.parent.as_ref() == Some(&a.id) {
                line.push_str(" (your parent)");
            }
            line
        })
        .collect();
    let prs = arbos_core::load_prs(place);
    if plan.is_empty() && peers.is_empty() && prs.is_empty() {
        return None;
    }
    let mut out = String::new();
    if !plan.is_empty() {
        out.push_str("<<plan>> your notes.md checklist and standing subscriptions:\n");
        out.push_str(&plan);
    }
    if !peers.is_empty() {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str("<<peers>> agents here you can message:\n");
        out.push_str(&peers.join("\n"));
        out.push('\n');
    }
    if !prs.is_empty() {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str("<<prs>> pull requests opened from this place (newest last):\n");
        for pr in prs.iter().rev().take(20).rev() {
            out.push_str(&format!("{} — by {}", pr.url, pr.agent));
            if !pr.branch.is_empty() {
                out.push_str(&format!(" from {}", pr.branch));
            }
            out.push('\n');
        }
    }
    Some(out)
}

/// Greetings do not need the tool list. Skipping it cuts prefill.
pub fn skip_tools(text: &str) -> bool {
    let t = text
        .trim()
        .trim_end_matches(['!', '.', ',', '?', '👋'])
        .trim()
        .to_ascii_lowercase();
    matches!(
        t.as_str(),
        "hello" | "hi" | "hey" | "yo" | "sup" | "thanks" | "thank you" | "ok" | "okay" | "bye"
    )
}

/// One roster line per skill: `name — description`. Names only when a
/// skill has no description.
pub fn skill_names(place: &Place) -> Vec<String> {
    arbos_core::load_skills(place)
        .iter()
        .map(arbos_core::Skill::roster_line)
        .collect()
}

fn first_agents_md(place: &Place) -> String {
    for name in ["AGENTS.md", "agents.md", "CLAUDE.md"] {
        let path = place.path.join(name);
        if let Ok(text) = std::fs::read_to_string(&path) {
            let brief = crate::evict::evict_head(&text, &format!("{}:1", path.display()));
            return format!("\n{name}:\n{brief}\n");
        }
    }
    String::new()
}

#[cfg(test)]
mod role_tests {
    use super::*;

    fn place(tag: &str) -> Place {
        let dir = std::env::temp_dir().join(format!(
            "arbos-prompt-role-{tag}-{}-{}",
            std::process::id(),
            arbos_core::now_ms()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Place::new(&dir)
    }

    #[test]
    fn a_worker_child_gets_the_worker_line_and_none_of_the_coordinator_text() {
        let place = place("worker");
        arbos_core::bootstrap(&place).unwrap();
        let mut child = Agent::root("w1");
        child.parent = Some(arbos_core::AgentId::new("root"));
        child.role = Some(arbos_core::project::WORKER.into());
        let text = instance_prompt(&place, &child, &[]);
        assert!(text.contains("Role: worker."), "{text}");
        assert!(!text.contains("Role: coordinator"), "{text}");
        assert!(!text.contains("Kickoff (spawn)"), "{text}");
        assert!(!text.contains("Delegate anything"), "{text}");
    }

    #[test]
    fn a_kind_with_no_role_has_no_role_line() {
        let place = place("none");
        arbos_core::bootstrap(&place).unwrap();
        let mut child = Agent::root("w1");
        child.parent = Some(arbos_core::AgentId::new("root"));
        child.kind = "reviewer".into();
        let text = instance_prompt(&place, &child, &[]);
        assert!(!text.contains("Role:"), "{text}");
    }
}
