> **RECOVERED, BADLY INCOMPLETE — this is not the original file.** 4,536 of the original 27,444 bytes, and four days stale. Source: a `read_file` in the kernel-inventory worker's transcript at 2026-09-12 22:08 UTC. Everything after that is missing, including the 26-row gap list and the official-post analysis added up to 2026-09-15 17:15 UTC. The research worker (`bc-29849c4b-8dca-5015-b120-a8af96d9a98f`) wrote those additions and should replace this file.
>
> The original was lost together with the whole `docs/` directory on 2026-09-16 between 07:43 and 09:01 UTC. Restored by the store-recovery worker `bc-0b112226-cf98-5cab-92c3-2671518dd9b9`. Cause, timeline and the full recovery inventory: `internal/store-docs-loss-2026-09-16.md`.

# Cursor Projects research

Research only. No code was changed. Written 2026-09-12, two days after launch.

## Summary

1. Cursor shipped "Projects" in beta on 2026-09-10. A Project is one long-lived thread with a **coordinator agent** that plans and delegates but does not write code.
2. The coordinator spawns **subagents** (worker agents) that run in parallel, mostly on cloud machines. It can also start a **local agent** on your computer when a task needs your machine.
3. Every Project has a **shared context**: a folder of files (research, plans, demos, notes on how you like work done) that syncs to every cloud and local agent in the Project.
4. **Subscriptions** let the coordinator act without a prompt: watch a Slack channel, follow PRs, fix CI, run on a schedule.
5. UI facts confirmed by Cursor: Projects live in the **left-hand nav** of the Agents Window. That is all Cursor says about layout. No official docs page exists yet.
6. No public source describes project tabs, a sub-agent side panel, or Project keyboard shortcuts. Cursor's `Cmd+T` is "new chat tab"; `Cmd+[`/`Cmd+]` move between chats; `Ctrl+Tab` switches agents.
7. Arbos today: sidebar on the **left**, one heading per project folder, chats nested under it, child agents indented under their parent. No tabs. Opening a folder is a two-step picker: machine (this Mac or an ssh host), then folder.
8. Jacob's requirement (a) — right sidebar, top project tabs, `Cmd+Shift+{ }` cycling, `Cmd+T` for new project — is **not** how Cursor does it. It is an Arbos design choice, not a copy.
9. Jacob's requirement (b) — one main chat per project that spawns sub-agent chats listed in a top-right panel — matches Cursor's coordinator model in spirit. The panel placement is Arbos's own; Cursor's placement is not documented.
10. Arbos already has the data model for (b): sessions carry `parent`, `delegate_number`, and the kernel reports child sessions. The gap is UI and the "one main chat" rule.

## What Cursor shipped

Terms used below:

- **Agents Window**: Cursor's agent-first window, separate from the code editor. It lists agents across many repos and machines. [Docs](https://cursor.com/docs/agent/agents-window)
- **Coordinator agent**: the one agent you talk to in a Project. It plans and delegates. It does not edit code.
- **Subagent**: an agent started by another agent. It has its own context window (its own memory of the conversation) and returns a result to its parent. [Docs](https://cursor.com/docs/subagents)
- **Cloud Agent**: an agent that runs on a Cursor-hosted virtual machine (VM), not your laptop. [Docs](https://cursor.com/docs/cloud-agent)

What a Project is (confirmed, primary sources):

- "Projects lets you take on larger bodies of work, such as a feature, a migration, or a full app. It maintains context over months of work, delegates tasks to thousands of subagents, and performs recurring work without being prompted." — [Changelog](https://cursor.com/changelog/projects)
- "Rather than creating a chat for every task, you work with a coordinator agent in a single, persistent thread." — [@cursor_ai launch thread](https://x.com/cursor_ai/status/2098162488013455784) ([unrolled](https://unrollnow.com/status/2098162488013455784))
- "Because it delegates rather than executes, it is never blocked and is always responsive to direction." — [Blog](https://cursor.com/blog/projects)
- Three pillars named by Cursor: **Cloud by default, local when needed**; **Shared context**; **Subscriptions**. — [Blog](https://cursor.com/blog/projects)
- Status: beta, rolling out to all users from 2026-09-10. Cloud Agents need a paid plan (inferred from Cloud Agent docs; reviewers say the same). — [Changelog](https://cursor.com/changelog/projects), [eesel review](https://www.eesel.ai/blog/cursor-projects-review)
- Cursor's own numbers: new users merge 30% more PRs; heavy Projects users merge 6x as many. Self-reported, no method given. — [Blog](https://cursor.com/blog/projects)

Three use patterns Cursor names (confirmed): **feature work** (research → plan → parallel implement/test → local try-out → post-ship bug handling), **migrations** (hundreds of PRs, review loosens over time), **gardening** (never-ending upkeep driven by PR/Slack/schedule signals). — [Blog](https://cursor.com/blog/projects)

Real-world workflow from a Project author (confirmed, Fredrika Lindh, Cursor):

- Projects run for weeks or months; all work for one big task goes in one chat. Some people use one Project for everything.