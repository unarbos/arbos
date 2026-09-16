use anyhow::Result;
use arbos_core::{
    Agent, Event, EventKind, Layout, Place, Usage, Wake, WakeKind, append_event, append_events,
    load_transcript,
};
use std::sync::Arc;

use crate::{
    batch::BatchCfg,
    compact,
    control::TurnControl,
    host::Host,
    prompt::{skill_names, skip_tools},
    provider::{Interrupted, Provider},
    retry::Models,
    step::{Step, StepCx, model_step},
    tool::{Registry, RunCx},
    tools::{self, Grep, Hooks},
};

/// Provider prompt_tokens ÷ our chars/4 estimate stays in this band; outside
/// it the provider is counting something we do not project.
const CALIB_MIN: f64 = 0.5;
const CALIB_MAX: f64 = 3.0;
/// Smallest window the loop will plan against.
const MIN_WINDOW: u64 = 4_000;
/// What `window_tokens = 0` falls back to when the provider does not list
/// a context length for the model.
const DEFAULT_WINDOW: u64 = 128_000;
/// Output tokens held back from the prompt budget when the provider does
/// not say how long a completion may be.
const DEFAULT_OUTPUT_RESERVE: u64 = 4_096;
/// Least `max_tokens` a step is ever sent, however full the prompt is.
const MIN_OUTPUT_TOKENS: u64 = 256;
/// Mid-stream cuts a turn rides through before giving up.
const MAX_CUTS: u32 = 2;

/// A reply that is one JSON object naming a tool, or a tool's arguments
/// (`{"path": "src/lib.rs"}`), instead of a function call.
/// The model a turn runs: the wake's, when the user switched one turn
/// ("switch to <vision model> for this turn"); else, for a child, the
/// host's `child_model` when set (it beats the spawn call and the kind);
/// else the agent's own; else the host's.
pub fn pick_model(wake_model: &str, agent: &Agent, host_model: &str, child_model: &str) -> String {
    let wake_model = wake_model.trim();
    if !wake_model.is_empty() {
        return wake_model.to_string();
    }
    if agent.parent.is_some() && !child_model.trim().is_empty() {
        return child_model.trim().to_string();
    }
    if agent.model == "inherit" || agent.model.is_empty() {
        host_model.to_string()
    } else {
        agent.model.clone()
    }
}

/// What the model reads when its turn is about to end with the brief's
/// image still owed.
pub const SHOW_NUDGE: &str = "Your brief says the user asked to see the result (Show), and this turn made no image. Make it now: a command's output → screenshot target:\"text\" title:\"<the command>\" text:\"<its output>\"; a page → browser action:screenshot; a window → screenshot target:\"window\". Then put the image path in your report.";

/// The turn's wake carried a `Show:` line (the user asked to see the
/// result) and no tool call since has produced an image.
fn image_owed(events: &[Event]) -> bool {
    let Some(start) = events.iter().rposition(Event::is_wake) else {
        return false;
    };
    let brief_shows = matches!(
        &events[start].kind,
        EventKind::Wake { text: Some(t), .. } if t.contains("\nShow: ") || t.starts_with("Show: ")
    );
    if !brief_shows {
        return false;
    }
    !events[start..].iter().any(|e| match &e.kind {
        EventKind::Tool(rec) => !rec.images.is_empty() && rec.error.is_none(),
        _ => false,
    })
}

/// What the model reads when its turn is about to end with a file its
/// brief named as `Output:` still missing. `{paths}` is filled in.
pub const OUTPUT_NUDGE: &str = "Your brief names Output: {paths} — not written yet. A reply is not the deliverable: write the file now (write path:\"<path>\" contents:…), check it exists, then name its path in your report.";

/// The files a brief's `Output:` line names — one line, or the indented
/// lines under it — as paths: tokens with a `/` or a `.arbos` head and a
/// file extension. Folders (`…/`), placeholders (`<topic>`), and prose
/// are not files to owe.
pub fn brief_output_paths(text: &str) -> Vec<String> {
    let mut lines = text.lines().peekable();
    let mut spec = String::new();
    while let Some(line) = lines.next() {
        let Some(rest) = line.trim_start().strip_prefix("Output:") else {
            continue;
        };
        spec.push_str(rest);
        spec.push('\n');
        while let Some(next) = lines.peek() {
            if next.starts_with("  ") && !next.trim().is_empty() {
                spec.push_str(next);
                spec.push('\n');
                lines.next();
            } else {
                break;
            }
        }
        break;
    }
    spec.split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | '(' | ')' | '`' | '"' | '\''))
        .map(|t| t.trim_end_matches(['.', ':']))
        .filter(|t| !t.is_empty() && !t.ends_with('/') && !t.contains('<') && !t.contains('*'))
        .filter(|t| t.contains('/') || t.starts_with(".arbos"))
        .filter(|t| {
            let name = t.rsplit('/').next().unwrap_or(t);
            name.rsplit_once('.').is_some_and(|(stem, ext)| {
                !stem.is_empty()
                    && (1..=5).contains(&ext.len())
                    && ext.chars().all(|c| c.is_ascii_alphanumeric())
            })
        })
        .map(str::to_string)
        .collect()
}

/// The turn's wake named `Output:` files and some are not on disk: which.
/// `docs/x.md` and `.arbos/docs/x.md` are the same file (the store is
/// spoken of without its folder); a path is looked for at the place and
/// under `.arbos/`.
fn output_owed(events: &[Event], place: &std::path::Path) -> Vec<String> {
    let Some(start) = events.iter().rposition(Event::is_wake) else {
        return Vec::new();
    };
    let EventKind::Wake { text: Some(t), .. } = &events[start].kind else {
        return Vec::new();
    };
    brief_output_paths(t)
        .into_iter()
        .filter(|p| {
            let rel = p.trim_start_matches("./");
            !(place.join(rel).exists() || place.join(".arbos").join(rel).exists())
        })
        .collect()
}

/// What the model reads when its reply links a pull request no tool
/// opened. `{urls}` is filled in.
pub const PR_LINK_NUDGE: &str = "Your reply links a pull request ({urls}) that no tool opened: no pr create or gh pr create output in your transcript has it, and the store has no record of it. A PR link comes from the tool's output, never from memory. Either open it now with pr action:create and report from its output, or say plainly that no pull request was opened and where the work is (a branch in this checkout, uncommitted edits).";

/// GitHub pull-request URLs in `reply` that appear in no tool result on
/// the transcript and in no `prs.jsonl` record: links the model made up
/// (a worker "opened PR 1" on a repository with no remote, cold-p5).
fn unbacked_pr_links(events: &[Event], reply: &str, place: &arbos_core::Place) -> Vec<String> {
    let claimed: Vec<String> = arbos_core::prs::pr_urls(reply)
        .into_iter()
        .map(|(url, _, _)| url)
        .collect();
    if claimed.is_empty() {
        return Vec::new();
    }
    let recorded: Vec<String> = arbos_core::load_prs(place)
        .into_iter()
        .map(|p| p.url)
        .collect();
    let mut seen: Vec<String> = Vec::new();
    for e in events {
        match &e.kind {
            EventKind::Tool(rec) => {
                if let Some(b) = &rec.body {
                    seen.extend(arbos_core::prs::pr_urls(b).into_iter().map(|(u, _, _)| u));
                }
                seen.extend(rec.paths.iter().filter(|p| p.contains("/pull/")).cloned());
            }
            // Someone else's report may carry a real PR: the user or a
            // worker said it, and the worker's own tool output backs it.
            EventKind::User { text, .. }
            | EventKind::Say { text, .. }
            | EventKind::Wake {
                text: Some(text), ..
            } => {
                seen.extend(
                    arbos_core::prs::pr_urls(text)
                        .into_iter()
                        .map(|(u, _, _)| u),
                );
            }
            _ => {}
        }
    }
    let known = |u: &str| {
        let key = u.trim_end_matches('/');
        recorded
            .iter()
            .chain(seen.iter())
            .any(|k| k.trim_end_matches('/') == key)
    };
    let mut out: Vec<String> = Vec::new();
    for url in claimed {
        if !known(&url) && !out.contains(&url) {
            out.push(url);
        }
    }
    out
}

/// A reply that only announces a command — "Running ls; cat README*
/// 2>/dev/null | head -60;" — and ends the turn without calling anything
/// (Jacob's Mac, 2026-09-15). Short: one or two lines, opening with a
/// doing-word, carrying a shell fragment. A real answer that happens to
/// quote a command runs longer than that.
fn looks_like_announced_command(content: &str) -> bool {
    let t = content.trim();
    if t.is_empty() || t.lines().count() > 2 || t.chars().count() > 300 {
        return false;
    }
    let lower = t.to_ascii_lowercase();
    let opens = [
        "running ",
        "run ",
        "executing ",
        "checking ",
        "listing ",
        "reading ",
        "let me run",
        "let me check",
        "let me list",
        "i'll run",
        "i will run",
        "i'll check",
        "i will check",
        "now running",
        "next, run",
        "next: run",
        "status: running",
    ];
    if !opens.iter().any(|o| lower.starts_with(o)) {
        return false;
    }
    // Telling the user what to run is an answer, not an announcement.
    if ["yourself", "you can", "when you", "if you"]
        .iter()
        .any(|m| lower.contains(m))
    {
        return false;
    }
    let shell = [
        " | ",
        "&&",
        "2>/dev/null",
        "2> /dev/null",
        "ls ",
        "ls;",
        "cat ",
        "grep ",
        "find ",
        "git ",
        "head ",
        "tail ",
        "wc ",
        "python",
        "cargo ",
        "npm ",
        "make ",
        "curl ",
        "tree",
    ];
    shell.iter().any(|m| lower.contains(m))
}

fn looks_like_tool_call_text(content: &str) -> bool {
    let t = content
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    if !(t.starts_with('{') && t.ends_with('}')) || t.len() > 4000 {
        return false;
    }
    let Ok(v) = serde_json::from_str::<serde_json::Value>(t) else {
        return false;
    };
    let Some(obj) = v.as_object() else {
        return false;
    };
    let keys = [
        "name",
        "tool",
        "function",
        "arguments",
        "path",
        "command",
        "pattern",
        "anchor",
        "content",
        "patch",
    ];
    obj.keys().filter(|k| keys.contains(&k.as_str())).count() >= 1
}

/// End a turn that could not start. The reason lands on the transcript as
/// a failed notice, so the window shows it instead of a silent stderr line.
fn refuse(
    transcript: &std::path::Path,
    place: &Place,
    agent: &Agent,
    message: String,
) -> Result<()> {
    eprintln!("turn {}: {message}", agent.id);
    append_events(
        transcript,
        &[
            Event::new(EventKind::Notice {
                text: message,
                failed: true,
            }),
            Event::new(EventKind::TurnComplete { usage: None }),
        ],
    )?;
    tools::file_hooks::after_turn(place, agent);
    Ok(())
}

pub struct TurnOpts {
    pub place: Place,
    pub agent: Agent,
    pub wake: Wake,
    pub host: Host,
    pub registry: Arc<Registry>,
    pub grep: Arc<dyn Grep>,
    pub hooks: Arc<dyn Hooks>,
    pub control: TurnControl,
}

/// Run one wake to completion. The job is gone when this returns. Every
/// exit leaves the transcript ended (`TurnComplete`), so a stopped or failed
/// turn is never replayed as unfinished on the next kernel start.
pub async fn turn(opts: TurnOpts) -> Result<()> {
    let TurnOpts {
        place,
        agent,
        wake,
        host,
        registry,
        grep,
        hooks,
        control,
    } = opts;
    let layout = Layout::new(&place, agent.id.as_str());
    let transcript = layout.transcript();

    // Every turn starts with the wake that caused it, so an `assistant`
    // reply never appears on the transcript without its cause. `Compact` is
    // housekeeping and makes no model step (it returns early below).
    if wake.kind != WakeKind::Compact {
        let mut batch = vec![Event::new(EventKind::Wake {
            wake: wake.kind.as_str().into(),
            text: wake.text.clone(),
            brief: (!wake.brief.is_empty()).then(|| wake.brief.clone()),
        })];
        if wake.kind == WakeKind::User {
            // A new task: the mechanism line belongs to the last one.
            crate::mechanism::reset(&place, &agent.id);
            crate::repro::reset(&place, &agent.id);
            if let Some(text) = &wake.text {
                batch.push(Event::new(EventKind::User {
                    text: text.clone(),
                    attachments: wake.attachments.clone(),
                    channel: wake.channel.clone(),
                    device: wake.device.clone(),
                }));
            }
        }
        append_events(&transcript, &batch)?;
    }
    // Loaded after the append so every event carries its line. Another
    // writer may have landed between; the reload sees that too.
    let mut events = load_transcript(&transcript)?;

    let cwd = agent.work_dir(&place.path);
    {
        let snap = cwd.clone();
        let agent_dir = layout.dir.clone();
        let agent_id = agent.id.to_string();
        let line = events.len() as u64;
        tokio::task::spawn_blocking(move || {
            let _ = crate::tools::git::snapshot_turn(&snap, &agent_dir, &agent_id, line);
        });
    }

    // No key or no usable base: the turn cannot start. Say so on the
    // transcript, where the window shows it, and close the turn so the
    // wake is not replayed as unfinished at the next kernel start.
    // A scripted model needs no key and asks the network nothing.
    let replay = crate::replay::current()?;
    // No key in config or environment: the place's secrets.toml may name
    // one (`OPENROUTER_API_KEY = "op://…"`), for servers with 1Password.
    let key_from_secrets = match host.api_key() {
        Some(_) => None,
        None => {
            let dir = place.path().to_path_buf();
            let env = host.config.key_env();
            tokio::task::spawn_blocking(move || crate::secrets::model_key_from_place(&dir, &env))
                .await
                .ok()
                .flatten()
        }
    };
    let found_key = match key_from_secrets {
        Some(Ok(k)) => {
            crate::secrets::store().protect("MODEL_API_KEY", k.clone());
            Some(k)
        }
        Some(Err(e)) => {
            return refuse(
                &transcript,
                &place,
                &agent,
                format!(
                    "{} (secrets.toml names it, but: {e:#})",
                    host.missing_key_hint()
                ),
            );
        }
        None => host.api_key(),
    };
    let (key, api_base) = match (found_key, host.config.api_base()) {
        _ if replay.is_some() => (
            "replay".to_string(),
            host.config
                .api_base()
                .unwrap_or_else(|_| "http://replay.invalid/v1".to_string()),
        ),
        (Some(key), Ok(base)) => (key, base),
        // A compaction has no transcript of its own to refuse on.
        (None, _) if wake.kind == WakeKind::Compact => {
            anyhow::bail!("{}", host.missing_key_hint())
        }
        (None, _) => return refuse(&transcript, &place, &agent, host.missing_key_hint()),
        (_, Err(e)) => return refuse(&transcript, &place, &agent, format!("{e:#}")),
    };
    let picked = pick_model(
        &wake.model,
        &agent,
        &host.config.model(),
        &host.config.child_model,
    );
    // A family this key cannot call (learned from a 403 on an earlier
    // turn, or the kickoff probe) is not tried again turn after turn: the
    // first model the key can use stands in, and the user hears why in
    // one plain sentence, once per turn.
    let mut blocked_note: Option<String> = None;
    let model = if replay.is_none() && crate::blocked::is_blocked(&api_base, &picked) {
        let suggested = host.config.provider().suggested_models();
        let mut candidates: Vec<&str> = host
            .config
            .fallback_models
            .iter()
            .map(String::as_str)
            .filter(|m| !m.is_empty() && *m != "none")
            .collect();
        candidates.extend(suggested.iter().copied());
        match crate::blocked::alternative(&api_base, &picked, &candidates) {
            Some(alt) => {
                blocked_note = Some(format!(
                    "This key cannot use {} models ({} was refused by the provider), so {alt} answers for now. Pick another default in Settings › Model to make it stick.",
                    crate::retry::family(&picked),
                    picked
                ));
                alt.to_string()
            }
            None => picked,
        }
    } else {
        picked
    };
    // The model's own context length, from the provider's model list.
    // `window_tokens = 0` uses it, capped so a 1M-token model does not turn
    // every step into a 1M-token prompt. A configured `window_tokens` is a
    // cap on it too, never a raise: a 16k model planned against 128k has
    // every request past 16k rejected. A provider that does not say gets
    // the old default.
    let listed = if replay.is_some() {
        None
    } else {
        crate::provider::context_window(&api_base, &key, &model).await
    };
    // Whether the model takes image input, when the provider's list says.
    let sees_images = if replay.is_some() {
        None
    } else {
        crate::provider::accepts_images(&api_base, &key, &model).await
    };
    let limit = match (host.config.window_tokens, listed) {
        (0, Some(c)) => c.min(host.config.window_tokens_max.max(MIN_WINDOW)),
        (0, None) => DEFAULT_WINDOW,
        (n, Some(c)) => n.min(c),
        (n, None) => n,
    }
    .max(MIN_WINDOW);
    // Output per step: the model's own completion limit under the
    // configured cap, and never more than a quarter of its context, so a
    // small model keeps room to read its prompt. Unknown limit: send
    // nothing rather than guess high and get a 400.
    let output_cap = match host.config.max_output_tokens {
        0 => None,
        _ if replay.is_some() => None,
        cap => crate::provider::max_completion_tokens(&api_base, &key, &model)
            .await
            .map(|n| n.min(cap)),
    }
    .map(|n| n.min(limit / 4).max(MIN_OUTPUT_TOKENS));
    // The prompt is not only the messages: tool schemas ride on every call
    // and the answer needs room. The window the loop plans against is what
    // is left for the messages once both are held back.
    let view = registry.view(&agent);
    let tool_tokens =
        crate::evict::estimate_tokens(&serde_json::to_string(view.schemas()).unwrap_or_default());
    let output_reserve = output_cap.unwrap_or(DEFAULT_OUTPUT_RESERVE).min(limit / 4);
    let window = limit
        .saturating_sub(output_reserve)
        .saturating_sub(tool_tokens)
        .max(MIN_WINDOW);
    let skills = skill_names(&place);
    let compact_policy = compact::Policy::new(window, &host.config);
    let ccx = compact::Cx {
        place: &place,
        agent: &agent,
        transcript: &transcript,
        skills: &skills,
        policy: &compact_policy,
        hooks: &hooks,
    };
    // provider prompt_tokens ÷ our estimate. 1.0 until the first step reports.
    let mut calib = 1.0f64;

    let mut models = Models::with_defaults(
        model.clone(),
        &host.config.fallback_models,
        &host.config.api_base,
    );
    if replay.is_none() {
        models.drop_blocked(&api_base);
    }
    let policy = crate::retry::RetryPolicy::from_config(&host.config);
    let mut provider = Provider {
        base: api_base,
        key,
        model,
        reasoning_effort: host.config.reasoning_effort.clone(),
        cache_ttl: host.config.cache_ttl.clone(),
        data_policy: host.config.data_policy.clone(),
        stream_idle: std::time::Duration::from_millis(host.config.stream_idle_ms.max(1_000)),
        first_byte: std::time::Duration::from_millis(
            host.config
                .first_byte_ms
                .max(3_000)
                .min(host.config.stream_idle_ms.max(1_000)),
        ),
        max_tokens: output_cap,
        trace: host.config.trace.then(|| layout.dir.join("trace")),
        trace_agent: agent.id.to_string(),
        trace_purpose: "turn".into(),
        trace_line: 0,
        replay,
    };
    let batch_cfg = BatchCfg {
        max_parallel: host.config.max_parallel_tools,
        speculate: host.config.speculate,
    };
    // A new project's first turn: ask the key whether it can call the
    // model at all before anything is generated. A refusal marks the
    // family, switches to a model the key can use, and says so in one
    // sentence — instead of the provider's policy text as the project's
    // opening line (Code2, 2026-09-16). Fifteen seconds at most; any
    // other failure is left to the turn's own retries.
    if wake.kind == WakeKind::Kickoff && provider.replay.is_none() && blocked_note.is_none() {
        let api_base = provider.base.clone();
        if let Err(pe) = provider.probe(control.cancel()).await
            && pe.status == Some(403)
        {
            eprintln!(
                "provider probe: {}: {pe} — {}",
                provider.model,
                pe.message.trim()
            );
            crate::blocked::mark(&api_base, &provider.model, &pe.message);
            let mut candidates: Vec<&str> = host
                .config
                .fallback_models
                .iter()
                .map(String::as_str)
                .filter(|m| !m.is_empty() && *m != "none")
                .collect();
            candidates.extend(host.config.provider().suggested_models().iter().copied());
            if let Some(alt) = crate::blocked::alternative(&api_base, &provider.model, &candidates)
            {
                blocked_note = Some(format!(
                    "This key cannot use {} models ({} was refused by the provider), so {alt} answers for now. Pick another default in Settings › Model to make it stick.",
                    crate::retry::family(&provider.model),
                    provider.model
                ));
                provider.model = alt.to_string();
                models = Models::with_defaults(
                    alt.to_string(),
                    &host.config.fallback_models,
                    &host.config.api_base,
                );
                models.drop_blocked(&api_base);
            }
        }
    }
    if let Some(note) = blocked_note.take() {
        append_event(
            &transcript,
            &Event::new(EventKind::Notice {
                text: note,
                failed: false,
            }),
        )?;
        events = load_transcript(&transcript)?;
    }
    let mut cx = RunCx {
        place: place.clone(),
        agent: agent.clone(),
        cwd,
        call_id: String::new(),
        step: 0,
        cancel: control.cancel().clone(),
        grep,
        hooks: Arc::clone(&hooks),
        bash_wait_ms: host.config.bash_wait_ms,
        hops: wake.hops,
        web: Arc::new(crate::tool::WebCfg {
            search_url: host.config.search_url.clone(),
            search_key: host.config.search_key.clone(),
            api_base: provider.base.clone(),
            api_key: Some(provider.key.clone()),
            model: host.config.search_model.clone(),
        }),
    };

    // The desktop deletes a chat with `rm -rf` on its folder, turn or no
    // turn. Every append would recreate the folder as a ghost (a transcript
    // with no agent.md that nothing lists). A turn whose folder is gone is
    // over: nothing more is written for it (QA bug qa-017).
    let gone = || !layout.agent_md().exists();

    let end = |usage: Option<Usage>, interrupted: Option<&str>| -> Result<()> {
        if gone() {
            eprintln!(
                "turn {}: agent folder was deleted; ending without writing",
                agent.id
            );
            return Ok(());
        }
        let mut batch = Vec::new();
        if let Some(detail) = interrupted {
            batch.push(Event::new(EventKind::Interrupted {
                detail: detail.into(),
            }));
        }
        batch.push(Event::new(EventKind::TurnComplete { usage }));
        append_events(&transcript, &batch)?;
        tools::file_hooks::after_turn(&place, &agent);
        Ok(())
    };

    let mut nudged = false;
    // The reason of the last nudge, for what the next reply may not say.
    let mut nudge_reason: Option<&'static str> = None;
    // Replies with no words and no calls in a row (a done wake's silence
    // does not count): the second one is the model failing, not
    // answering — another model takes the turn, or the user is told.
    let mut empty_in_a_row = 0u32;
    let mut cuts = 0u32;
    // Dollars over every model call of this turn, when the provider prices them.
    let mut turn_cost: Option<f64> = None;
    // Prompt tokens the provider served from cache, summed the same way.
    let mut turn_cached: Option<u64> = None;
    // Same tool, same arguments, same failure, again and again: name it.
    let mut last_failure: Option<String> = None;
    let mut failure_streak = 0u32;
    // Identical read-only calls since the last write.
    let mut repeats: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    // The same words said again in one turn: a coordinator waiting on its
    // workers streamed "Please provide the sentences … once both are
    // available" five to eight times between status calls (mobile cycle
    // 1, item 4). Told once at the second; the turn ends at the third.
    // Each reply's words, in order of saying; a new reply that overlaps
    // one of them almost entirely (reworded or not) is the same reply
    // (`repeat::near_duplicate`).
    let mut said: Vec<Vec<String>> = Vec::new();
    // On a worker's done or a peer's say, the last thing said before this
    // turn: a final reply that repeats it is not said again — the user
    // read it once already (mobile cycle 5). A subscription's or a job's
    // wake may report the same state again on purpose, so those are not
    // held to it.
    let prior_reply: Option<Vec<String>> = if !matches!(wake.kind, WakeKind::Done | WakeKind::Say) {
        None
    } else {
        events
            .iter()
            .rev()
            .skip_while(|e| !e.is_wake())
            .find_map(|e| match &e.kind {
                EventKind::Assistant { text, .. } if !text.trim().is_empty() => Some(text),
                _ => None,
            })
            .and_then(|t| crate::repeat::words(t))
    };
    let mut hidden_seen = 0usize;
    let mut first_step = true;
    loop {
        if gone() {
            eprintln!(
                "turn {}: agent folder was deleted; ending without writing",
                agent.id
            );
            return Ok(());
        }
        if control.is_stopped() {
            return end(None, Some(&control.stop_reason()));
        }
        // Every steer that arrived since the last boundary, in order, in
        // one write: the inbox files of kind `steer` (a user's words while
        // the turn runs, a peer's `say mode=steer`) and `wake` (the kernel:
        // a job finished). One file per message, so none is lost when they
        // come faster than the model steps (qa-014), and one a kernel
        // crash leaves behind starts the next turn instead.
        let steers = arbos_core::inbox::take_steers(&place, agent.id.as_str());
        if !steers.is_empty() {
            // An answer's words are already on the transcript (the kernel
            // appended the `answer` line when the user replied); taking the
            // file is what makes this step read them.
            let batch: Vec<Event> = steers
                .into_iter()
                .filter(|msg| msg.kind != "answer")
                .map(|msg| match msg.from.as_str() {
                    "kernel" => Event::new(EventKind::Notice {
                        text: msg.body,
                        failed: false,
                    }),
                    from if from.starts_with("agent:") => Event::new(EventKind::Say {
                        from: from["agent:".len()..].to_string(),
                        text: msg.body,
                    }),
                    _ => Event::new(EventKind::User {
                        text: msg.body,
                        attachments: msg.attachments,
                        channel: msg.channel,
                        device: msg.device,
                    }),
                })
                .collect();
            if !batch.is_empty() {
                append_events(&transcript, &batch)?;
            }
            events = load_transcript(&transcript)?;
        }

        let manual = control.take_compact() || wake.kind == WakeKind::Compact;
        let managed =
            match compact::manage(&ccx, &mut events, &provider, &control, calib, manual).await {
                Ok(m) => m,
                Err(e) if e.is::<Interrupted>() || control.is_stopped() => {
                    return end(
                        None,
                        Some(&format!("{} during compaction", control.stop_reason())),
                    );
                }
                Err(e) => return Err(e),
            };
        if first_step {
            first_step = false;
            hooks.prompt_size(crate::tools::PromptSize {
                model: provider.model.clone(),
                system: managed.system,
                tools: scaled_tools(tool_tokens, calib),
                conversation: managed.estimated.saturating_sub(managed.system),
            });
        }
        // After a fold or compaction the earlier answers are gone from the
        // model's view; asking again is the right move, not a repeat.
        let hidden = events
            .iter()
            .filter(|e| {
                matches!(
                    e.kind,
                    EventKind::Fold { .. } | EventKind::Compaction { .. }
                )
            })
            .count();
        if hidden != hidden_seen {
            hidden_seen = hidden;
            repeats.clear();
        }
        if wake.kind == WakeKind::Compact {
            // Housekeeping only: no model step, no turn on the log.
            return Ok(());
        }
        let tools = if wake.text.as_deref().is_some_and(skip_tools) {
            &[][..]
        } else {
            view.schemas()
        };
        // The answer gets what the context has left after this prompt.
        // Compaction aims to keep that at `output_cap`; the estimate can
        // still run under the provider's count, so the request itself is
        // sized to fit rather than rejected whole.
        if let Some(cap) = output_cap {
            let room = limit.saturating_sub(tool_tokens + managed.estimated);
            provider.max_tokens = Some(cap.min(room).max(MIN_OUTPUT_TOKENS));
        }
        // The reply lands on the line after the last one loaded. Another
        // writer may slip in between; the call ids still tie the two.
        provider.trace_line = events.last().map(|e| e.seq).unwrap_or(0) + 1;
        // One model call = one step; every event this step writes carries
        // the number, so a window pairs streamed text with the settled line.
        cx.step += 1;
        let step = model_step(
            StepCx {
                provider: &mut provider,
                models: &mut models,
                policy: &policy,
                view: &view,
                cx: &cx,
                control: &control,
                batch_cfg,
                transcript: &transcript,
                window: limit,
                vision_model: &host.config.vision_model,
                sees_images,
            },
            &managed.messages,
            tools,
        )
        .await?;
        // The model call took seconds; the chat may have been deleted
        // meanwhile (qa-017). Its reply is not written anywhere.
        if gone() {
            eprintln!(
                "turn {}: agent folder was deleted during the model call; ending without writing",
                agent.id
            );
            return Ok(());
        }
        let (content, calls, usage, outcomes, reasoning_details) = match step {
            Step::Done {
                content,
                calls,
                usage,
                outcomes,
                reasoning_details,
            } => (content, calls, usage, outcomes, reasoning_details),
            Step::Interrupted => {
                return end(
                    None,
                    Some(&format!("{} during model call", control.stop_reason())),
                );
            }
            Step::Cut { partial, why } if cuts < MAX_CUTS => {
                cuts += 1;
                let mut batch = Vec::new();
                if !partial.trim().is_empty() {
                    batch.push(Event::new(EventKind::Assistant {
                        text: partial.trim_matches('\n').to_string(),
                        step: cx.step,
                        reasoning_details: None,
                    }));
                }
                batch.push(Event::new(EventKind::Notice {
                    text: format!("{why} — the reply was cut off; continuing from there"),
                    failed: false,
                }));
                append_events(&transcript, &batch)?;
                events = load_transcript(&transcript)?;
                continue;
            }
            Step::Cut { partial, why } => {
                if !partial.trim().is_empty() {
                    append_event(
                        &transcript,
                        &Event::new(EventKind::Assistant {
                            text: partial,
                            step: cx.step,
                            reasoning_details: None,
                        }),
                    )?;
                }
                let message =
                    format!("{why} — cut off {cuts} times in one turn; send the message again");
                eprintln!("turn {}: {message}", agent.id);
                append_event(
                    &transcript,
                    &Event::new(EventKind::Notice {
                        text: message,
                        failed: true,
                    }),
                )?;
                return end(None, None);
            }
            Step::Failed { message } => {
                // On the transcript, so the window shows it and the wake is
                // not replayed as unfinished on the next kernel start.
                eprintln!("turn {}: {message}", agent.id);
                append_event(
                    &transcript,
                    &Event::new(EventKind::Notice {
                        text: message,
                        failed: true,
                    }),
                )?;
                return end(None, None);
            }
        };
        // Tool-call markup written as prose (`<invoke name="bash">…`, a
        // `<function_calls>` block, `<tool_call>`): nothing ran, and it
        // looks broken on every client. The markup never reaches the
        // transcript; the words around it do, and the nudge below says
        // what happened (subnet120 from the phone, 2026-09-16).
        let (mut content, had_markup) = crate::markup::strip_tool_markup(&content);
        // After "your reply was empty", an opening apology about the
        // empty reply is the kernel's correction leaking into the chat
        // ("Sorry — empty reply on my side, nothing blocking." was the
        // first thing Jacob read). The answer follows it; the apology
        // does not.
        if nudge_reason == Some("empty reply") {
            content = crate::apology::strip_empty_reply_apology(&content);
            nudge_reason = None;
        }
        // The provider's own count beats our chars/4 guess. Remember the
        // ratio against the *raw* estimate so it does not feed on itself.
        // The provider counts the tool schemas too; they go on our side as
        // well, or a short prompt reads as 2–3× denser than it is.
        if let Some(c) = usage.and_then(|u| u.cost) {
            turn_cost = Some(turn_cost.unwrap_or(0.0) + c);
        }
        if let Some(n) = usage.and_then(|u| u.cached) {
            turn_cached = Some(turn_cached.unwrap_or(0) + n);
        }
        if let Some(u) = usage {
            let ours = managed.raw + if tools.is_empty() { 0 } else { tool_tokens };
            if u.used > 0 && ours > 0 {
                calib = (u.used as f64 / ours as f64).clamp(CALIB_MIN, CALIB_MAX);
            }
        }
        // A `status` written as a line of text: the live line takes the
        // words and the line is not a reply — written as one it was a
        // code-looking bubble before the greeting, three to eleven of
        // them with Gemini (qal J1). With no tool call beside it the turn
        // is nudged on, not ended.
        let mut spoke_only = false;
        if let Some(step) = arbos_core::status::spoken(&content) {
            hooks.spoke_status(&step);
            content = String::new();
            spoke_only = calls.is_empty();
        }
        // A final reply that says again what this turn, or the reply
        // before this wake, already said: not written twice. The turn
        // ends with a notice in its place.
        if calls.is_empty() && !content.trim().is_empty() {
            let earlier = said
                .iter()
                .any(|w| crate::repeat::near_duplicate(w, &content));
            let before = prior_reply
                .as_ref()
                .is_some_and(|w| crate::repeat::near_duplicate(w, &content));
            if earlier || before {
                append_event(
                    &transcript,
                    &Event::new(EventKind::Notice {
                        text: if before {
                            "The reply repeated the previous message and was not said again; the turn ends here."
                                .to_string()
                        } else {
                            "The reply repeated something said earlier this turn and was not said again; the turn ends here."
                                .to_string()
                        },
                        failed: false,
                    }),
                )?;
                return end(
                    usage.map(|mut u| {
                        u.cost = turn_cost;
                        u.cached = turn_cached;
                        u
                    }),
                    None,
                );
            }
        }
        if !content.trim().is_empty() || !calls.is_empty() {
            // The Assistant line is the step boundary the projection and the
            // fold units cut on. A step that called tools without saying
            // anything still needs one, or every text-less step merges into
            // the previous step's assistant message and the model is shown
            // one parallel batch of forty calls where there were thirty
            // sequential steps. Empty text is fine; the UI skips it.
            append_event(
                &transcript,
                &Event::new(EventKind::Assistant {
                    text: content.trim_matches('\n').to_string(),
                    step: cx.step,
                    reasoning_details: (!reasoning_details.is_empty())
                        .then(|| serde_json::Value::Array(reasoning_details.clone())),
                }),
            )?;
        }
        let empty_reply = calls.is_empty()
            && content.trim().is_empty()
            && !had_markup
            && !spoke_only
            && wake.kind != WakeKind::Done
            && wake.kind != WakeKind::Serve;
        if empty_reply {
            empty_in_a_row += 1;
        } else {
            empty_in_a_row = 0;
        }
        if empty_in_a_row >= 2 {
            // Nudged once and still nothing: an empty answer is a model
            // failing. The next model takes the turn as it does after a
            // 403 or a silent first byte; alone, the user hears what
            // happened and what to do — not a blank chat (qal-040).
            let failed = models.current().to_string();
            if let Some(next) = models.next().map(str::to_string) {
                eprintln!("provider: {failed}: two empty replies in a row; switching to {next}");
                append_event(
                    &transcript,
                    &Event::new(EventKind::Notice {
                        text: format!(
                            "{failed} returned nothing twice, so {next} answers this turn."
                        ),
                        failed: false,
                    }),
                )?;
                nudged = false;
                empty_in_a_row = 0;
                events = load_transcript(&transcript)?;
                continue;
            }
            let what_now = if wake.kind == WakeKind::Kickoff {
                " Nothing was set up yet; your first message starts the project as usual."
            } else {
                " Try again, or pick another model."
            };
            append_event(
                &transcript,
                &Event::new(EventKind::Notice {
                    text: format!(
                        "{failed} returned nothing twice. Check the model in Settings › Model (or fallback_models in config.toml).{what_now}"
                    ),
                    failed: true,
                }),
            )?;
            return end(
                usage.map(|mut u| {
                    u.cost = turn_cost;
                    u.cached = turn_cached;
                    u
                }),
                None,
            );
        }
        if calls.is_empty() && !nudged && wake.kind != WakeKind::Serve {
            // No tool call and either nothing at all (Gemini does this
            // right after a compile error) or a tool call written out as
            // JSON text (small models). One nudge, then the turn ends for
            // real if it happens again.
            // A `nudge` line, not a `user` one: the model reads it as
            // `[kernel] …` either way, and the window draws it dim instead
            // of as a bubble the user never typed.
            let nudge = if had_markup {
                // Markup cut above: the reply may now be empty, but it was
                // a call written as text, not silence.
                Some((
                    "That was a tool call written as text, so nothing ran (the markup was not kept). Call the tool itself: the tools are functions, not text.".to_string(),
                    "tool call written as text",
                ))
            } else if spoke_only {
                Some((
                    "That line was a status, not a reply: the live line beside your name took it and the chat does not show it. Go on with the step it named — a tool call — or answer the user.".to_string(),
                    "status written as text",
                ))
            } else if content.trim().is_empty() {
                // A done wake that has nothing to add ends in silence:
                // Cursor's coordinator says nothing between worker reports
                // when the user is owed nothing yet (cold-p5).
                (wake.kind != WakeKind::Done).then(|| {
                    (
                        "Your reply was empty. Continue the task, or say what is blocking you — and do not mention the empty reply or this note: the user saw neither, and an apology about it would be the first thing they read."
                            .to_string(),
                        "empty reply",
                    )
                })
            } else if looks_like_tool_call_text(&content) {
                Some((
                    "That was a tool call written as text, so nothing ran. Call the tool itself."
                        .to_string(),
                    "tool call written as text",
                ))
            } else if looks_like_announced_command(&content) {
                Some((
                    "You announced a command but did not call bash, so nothing ran. Call bash with it now, then answer from its output."
                        .to_string(),
                    "tool call written as text",
                ))
            } else if let Some(urls) =
                Some(unbacked_pr_links(&events, &content, &place)).filter(|u| !u.is_empty())
            {
                Some((
                    PR_LINK_NUDGE.replace("{urls}", &urls.join(", ")),
                    "pr link not from a tool",
                ))
            } else if image_owed(&events) {
                // The brief said the user asked to see the result and the
                // turn is ending with no image made: once, before the
                // report goes out with words alone (kickoff item 3).
                Some((SHOW_NUDGE.to_string(), "image owed"))
            } else {
                // The brief named a deliverable and the turn is ending
                // without it: a research worker answered in its last words
                // and never wrote research.md (kickoff, 2026-09-15).
                let owed = output_owed(&events, place.path());
                (!owed.is_empty()).then(|| {
                    (
                        OUTPUT_NUDGE.replace("{paths}", &owed.join(", ")),
                        "output owed",
                    )
                })
            };
            if let Some((text, reason)) = nudge {
                nudged = true;
                nudge_reason = Some(reason);
                // The reply itself is already on the transcript (the
                // Assistant line above); only the nudge follows it, or the
                // window shows the same words twice.
                append_event(
                    &transcript,
                    &Event::new(EventKind::Nudge {
                        text,
                        reason: reason.into(),
                    }),
                )?;
                events = load_transcript(&transcript)?;
                continue;
            }
        }
        if calls.is_empty() {
            end(
                usage.map(|mut u| {
                    u.cost = turn_cost;
                    u.cached = turn_cached;
                    u
                }),
                None,
            )?;
            // A compact requested during the last model step would otherwise
            // die with this TurnControl. Run it now, on the finished turn.
            if control.take_compact() {
                events = load_transcript(&transcript)?;
                if let Err(e) =
                    compact::manage(&ccx, &mut events, &provider, &control, calib, true).await
                {
                    if !e.is::<Interrupted>() {
                        return Err(e);
                    }
                }
            }
            return Ok(());
        }

        let mut results: Vec<Event> = Vec::with_capacity(outcomes.len());
        // A tool that parked the agent (an `ask`): its result is written,
        // then the turn ends; the answer starts the next one.
        let mut parked: Option<String> = None;
        for (call, outcome) in outcomes {
            if let crate::batch::Outcome::Ran { out: Ok(o), .. } = &outcome
                && let Some(why) = &o.park
            {
                parked = Some(why.clone());
            }
            let sig = format!("{}\u{0}{}", call.name, call.arguments);
            let mut ev = outcome.into_event(&call, cx.step);
            if let EventKind::Tool(rec) = &mut ev.kind {
                // Re-asking the same question of an unchanged tree gets the
                // same answer. Any write clears the slate; a repeated read
                // after an edit is legitimate.
                if matches!(call.name.as_str(), "read" | "grep" | "find" | "ls")
                    && rec.error.is_none()
                {
                    let n = repeats.entry(sig.clone()).or_insert(0);
                    *n += 1;
                    if *n >= 3 {
                        if let Some(b) = &mut rec.body {
                            b.push_str(&format!(
                                "\n[kernel] identical {} call #{n} in this turn with no edits in between; the answer above has not changed. Decide and act.",
                                call.name
                            ));
                        }
                    }
                } else if !matches!(
                    call.name.as_str(),
                    "read" | "grep" | "find" | "ls" | "jobs" | "await"
                ) {
                    repeats.clear();
                }
                if rec.error.is_some() {
                    if last_failure.as_deref() == Some(sig.as_str()) {
                        failure_streak += 1;
                    } else {
                        failure_streak = 1;
                        last_failure = Some(sig);
                    }
                    if failure_streak >= 3 {
                        let note = format!(
                            " — this identical call has now failed {failure_streak} times in a row; the same call will fail the same way. Change the arguments or the approach."
                        );
                        if let Some(e) = &mut rec.error {
                            e.push_str(&note);
                        }
                        if let Some(b) = &mut rec.body {
                            b.push_str(&note);
                        }
                    }
                } else {
                    last_failure = None;
                    failure_streak = 0;
                }
            }
            results.push(ev);
        }
        if gone() {
            eprintln!(
                "turn {}: agent folder was deleted; ending without writing",
                agent.id
            );
            return Ok(());
        }
        if let Some(why) = parked {
            results.push(Event::new(EventKind::Notice {
                text: why,
                failed: false,
            }));
            append_events(&transcript, &results)?;
            return end(None, None);
        }
        if let Some(now) = crate::repeat::words(&content) {
            // Steps with tool calls between them: told once at the second
            // saying, the turn ends at the third.
            let n = 1 + said
                .iter()
                .filter(|w| crate::repeat::overlap(w, &now) >= crate::repeat::THRESHOLD)
                .count();
            said.push(now);
            if n == 2 {
                results.push(Event::new(EventKind::Nudge {
                    text: REPEAT_NUDGE.to_string(),
                    reason: "repeated reply".into(),
                }));
            } else if n >= 3 {
                results.push(Event::new(EventKind::Notice {
                    text: "The same words a third time in one turn: the turn ends here. A worker's done, a subscription, or the user opens the next one.".to_string(),
                    failed: false,
                }));
                append_events(&transcript, &results)?;
                return end(None, Some("repeated itself three times"));
            }
        }
        append_events(&transcript, &results)?;
        events = load_transcript(&transcript)?;
    }
}

/// What the model reads the second time it says the same thing in one
/// turn.
pub const REPEAT_NUDGE: &str = "You said that already this turn. Do not say it again. If you are waiting on workers, end the turn now with no tool calls — their done wakes you, and asking or polling does not bring it sooner. Otherwise do the next step.";

/// The tool schemas at the provider's rate, like the messages.
fn scaled_tools(tokens: u64, calib: f64) -> u64 {
    (tokens as f64 * calib).round() as u64
}

#[cfg(test)]
mod pick_model_tests {
    use super::*;

    fn agent(parent: Option<&str>, model: &str) -> Agent {
        let mut a = Agent::root("x");
        a.parent = parent.map(arbos_core::AgentId::new);
        a.model = model.into();
        a
    }

    #[test]
    fn child_model_pins_every_child_but_not_root_and_not_a_users_one_turn_switch() {
        let host = "openai/gpt-5.4-mini";
        // Root: its own, else the host's; child_model never applies.
        assert_eq!(
            pick_model("", &agent(None, "inherit"), host, "cheap/x"),
            host
        );
        assert_eq!(
            pick_model("", &agent(None, "anthropic/claude"), host, "cheap/x"),
            "anthropic/claude"
        );
        // A child: as asked when child_model is empty…
        assert_eq!(
            pick_model("", &agent(Some("root"), "inherit"), host, ""),
            host
        );
        assert_eq!(
            pick_model("", &agent(Some("root"), "kind/model"), host, ""),
            "kind/model"
        );
        // …and child_model over the spawn call and the kind when set.
        assert_eq!(
            pick_model("", &agent(Some("root"), "kind/model"), host, "cheap/x"),
            "cheap/x"
        );
        assert_eq!(
            pick_model("", &agent(Some("root"), "inherit"), host, " cheap/x "),
            "cheap/x"
        );
        // The user's per-turn switch beats everything.
        assert_eq!(
            pick_model(
                "vision/y",
                &agent(Some("root"), "kind/model"),
                host,
                "cheap/x"
            ),
            "vision/y"
        );
    }
}

#[cfg(test)]
mod output_owed_tests {
    use super::brief_output_paths;

    #[test]
    fn the_briefs_output_files_are_read_and_prose_is_not() {
        // The research worker's brief (kickoff, 2026-09-15).
        let brief = "Task: research full duplex voice agents\nDo: \n  1. Search.\n  2. Write it up.\nRules: no merging\nOutput: .arbos/docs/research.md\nReport: a few lines";
        assert_eq!(brief_output_paths(brief), vec![".arbos/docs/research.md"]);
        // Two files, spoken without the store's folder, in a sentence.
        let two = "Output: write docs/design.md and internal/qa-plan.md (both under the store).\nReport: x";
        assert_eq!(
            brief_output_paths(two),
            vec!["docs/design.md", "internal/qa-plan.md"]
        );
        // The default Output text: folders and a placeholder, no file owed.
        let default = format!("Output: {}\nReport: x", arbos_core::store::KICKOFF_OUTPUT);
        assert!(brief_output_paths(&default).is_empty(), "{default}");
        // Indented lines under Output: count; the Report line does not.
        let multi = "Output: \n  media/toy/run.txt\n  docs/notes.md\nReport: docs/never.md";
        assert_eq!(
            brief_output_paths(multi),
            vec!["media/toy/run.txt", "docs/notes.md"]
        );
        assert!(brief_output_paths("Task: no output line").is_empty());
    }
}

#[cfg(test)]
mod announced_command_tests {
    use super::looks_like_announced_command;

    #[test]
    fn an_announced_command_with_no_call_is_caught_and_answers_are_not() {
        assert!(looks_like_announced_command(
            "Running ls; cat README* 2>/dev/null | head -60;"
        ));
        assert!(looks_like_announced_command(
            "Let me check with `git status`."
        ));
        assert!(looks_like_announced_command(
            "Running `cargo test -p arbos-kernel`"
        ));
        assert!(!looks_like_announced_command(
            "The repo holds a kernel, a desktop app, and a harness."
        ));
        assert!(!looks_like_announced_command(
            "Running the suite showed 3 failures:\n- a\n- b\n- c\nAll three are in the parser."
        ));
        assert!(!looks_like_announced_command("Done."));
        assert!(!looks_like_announced_command(
            "Run it yourself with `make` when you are ready; I have not changed anything."
        ));
    }
}
