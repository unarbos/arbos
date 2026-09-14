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
        })];
        if wake.kind == WakeKind::User {
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
    // The wake may name a model for this turn only ("switch to <vision
    // model> for this turn"); otherwise the agent's, then the host's.
    let model = if !wake.model.trim().is_empty() {
        wake.model.trim().to_string()
    } else if agent.model == "inherit" || agent.model.is_empty() {
        host.config.model()
    } else {
        agent.model.clone()
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
    let policy = crate::retry::RetryPolicy::from_config(&host.config);
    let mut provider = Provider {
        base: api_base,
        key,
        model,
        reasoning_effort: host.config.reasoning_effort.clone(),
        cache_ttl: host.config.cache_ttl.clone(),
        data_policy: host.config.data_policy.clone(),
        stream_idle: std::time::Duration::from_millis(host.config.stream_idle_ms.max(1_000)),
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
    let cx = RunCx {
        place: place.clone(),
        agent: agent.clone(),
        cwd,
        call_id: String::new(),
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
            let batch: Vec<Event> = steers
                .into_iter()
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
            append_events(&transcript, &batch)?;
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
                    reasoning_details: (!reasoning_details.is_empty())
                        .then(|| serde_json::Value::Array(reasoning_details.clone())),
                }),
            )?;
        }
        if calls.is_empty() && !nudged && wake.kind != WakeKind::Serve {
            // No tool call and either nothing at all (Gemini does this
            // right after a compile error) or a tool call written out as
            // JSON text (small models). One nudge, then the turn ends for
            // real if it happens again.
            // A `nudge` line, not a `user` one: the model reads it as
            // `[kernel] …` either way, and the window draws it dim instead
            // of as a bubble the user never typed.
            let nudge = if content.trim().is_empty() {
                Some("Your reply was empty. Continue the task, or say what is blocking you.")
            } else if looks_like_tool_call_text(&content) {
                Some("That was a tool call written as text, so nothing ran. Call the tool itself.")
            } else {
                None
            };
            if let Some(text) = nudge {
                nudged = true;
                let mut batch = Vec::new();
                if !content.trim().is_empty() {
                    batch.push(Event::new(EventKind::Assistant {
                        text: content.trim_matches('\n').to_string(),
                        reasoning_details: None,
                    }));
                }
                batch.push(Event::new(EventKind::Nudge { text: text.into() }));
                append_events(&transcript, &batch)?;
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
            let mut ev = outcome.into_event(&call);
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
        append_events(&transcript, &results)?;
        events = load_transcript(&transcript)?;
    }
}

/// The tool schemas at the provider's rate, like the messages.
fn scaled_tools(tokens: u64, calib: f64) -> u64 {
    (tokens as f64 * calib).round() as u64
}
