> **RECOVERED, INCOMPLETE — this is not the original file.** 9,586 of the original 18,990 bytes. Source: a full `read_file` of this document captured in the symmetry worker's transcript at 2026-09-13 06:15 UTC. Everything added between 2026-09-13 06:15 and the last known write at 2026-09-15 13:28 UTC is missing — roughly the second half of the document, including later dated decisions.
>
> The original was lost together with the whole `docs/` directory on 2026-09-16 between 07:43 and 09:01 UTC. Restored by the store-recovery worker `bc-0b112226-cf98-5cab-92c3-2671518dd9b9`. Cause, timeline and the full recovery inventory: `internal/store-docs-loss-2026-09-16.md`.

# Arbos — project context

Stable goals, constraints, and decisions. Progress lives in `notes.md`.

## High-level goal

Speak to Arbos in full duplex mode from Jacob's phone and from his computer.

- Full duplex: both sides can talk and listen at the same time. Arbos can be interrupted mid-sentence, and it hears Jacob while it speaks.
- Two clients: phone and desktop.

### The end goal is the iPhone app (stated 2026-09-12)

- Elegant, minimalist, simple. Jacob talks with his agent project in one long-form, full-duplex conversation, like a phone call with Arbos, with no interruption of the flow.
- Arbos explains things in a non-wordy way: the experience is human and brief, but full in power to build anything.
- Use case: Jacob is out running with his phone and gets work done just by talking.
- Start the iPhone app right away. iOS builds run on Jacob's Macbook (self-hosted worker), not on cloud VMs.

### Ambition (stated 2026-09-12)

Turn Arbos into the premier agent, on par with Hermes, Codex, Cursor, and Claude Code.

## Acceptance benchmark: this project's own kickoff thread (stated 2026-09-12)

The tasks Jacob gave in this Project's first session are the cardinal example of what Arbos must achieve. If Arbos cannot do this, we have failed.

What that session looked like, as a checklist Arbos must pass:

- Take a stream of high-level goals in plain speech, one after another, and keep going without losing any.
- Record goals, principles, and constraints in one master file that every agent reads first.
- Install and run a repo branch on a fresh machine, and send back a screenshot the user can see.
- Research a topic from primary sources and deliver a linked document.
- Draft design documents from a code audit plus stated principles, with decisions listed for the user.
- Spin off parallel workers: cloud VMs for independent work, the user's own Mac for iOS builds.
- Steer running workers with new constraints mid-task instead of restarting them.
- Fix a bug found along the way and open a PR against the right base branch.
- Stand up full-time loops (QA break-and-fix, features, Cursor-parity) that keep running after the session.
- Look up and use API keys from a vault to rent compute or call services, without leaking them.
- Keep a live status file the user can scan, and report results briefly and clearly.
- Hold risky decisions (auto-merge) for the user and ask once, plainly.

## Resources: 1Password service account (stated 2026-09-12)

Agents have a 1Password service account (`OP_SERVICE_ACCOUNT_TOKEN`, vault Arbos; `op` CLI, read items by ID). It holds API keys agents may use to solve problems: rent GPUs, rent other computers, run evaluations, call model APIs. Use them when a task needs compute or a service. Never Doppler. Never print secret values into logs, docs, or PRs.

## Resources: machines and Cloudflare (stated 2026-09-12)

- Jacob has machines available, including ArbosLife, a very powerful server. Use it when Arbos needs a place to run (kernel, QA loop, self-served models). Access details live in the 1Password vault.
- A Cloudflare account is available (also in the vault). Use it (Tunnels, Access, DNS) to make agents reachable from anywhere when needed; that is the easy path for the remote-attach stretch goal.
- Cloud agents: nice to have, not the top priority. Keep the design open-source-friendly so anyone can run their own; feel free to use all this compute.

## Model constraint

- Preferred: an open source speech model, either self-served or reached through an API key.
- Fallback: the OpenAI API (realtime/voice) if open source is not workable.

### Full-duplex target (stated 2026-09-13)

- The experience to emulate is OpenAI's GPT Live (https://openai.com/index/introducing-gpt-live/): one continuous conversation, no turn-taking, natural interruption.
- The full-duplex idea comes from NVIDIA's work. Look there and at other open-source full-duplex speech models before falling back to a pipeline (ASR then LLM then TTS).
- The voice model must be able to act: Jacob asks it by voice to send off an agent to do something, it dispatches the agent, and reports back in voice inside the app.
- The phone app also streams text and lets Jacob view the main agent chat.

## Model backend: OpenRouter first (stated 2026-09-12)

- An OpenRouter key is in the 1Password vault. Agents may use it for everything (model calls in Arbos, evaluations, workers).
- Assume OpenRouter is the backend most people will use for Arbos. Setup with an OpenRouter key must be seamless: paste key, pick model, go. Any model provider (OpenAI, open source via API) sits behind the same provider interface; OpenRouter is the default.

## Stretch goal: reach agents from anywhere (stated 2026-09-12)

Build Arbos agents so they can be reached from any client.

- Attach the desktop app to agents running remotely.
- Connect from the phone (an app we build).
- Maybe let other people connect to an agent and share it.
- Implication: the agent runs as a service with a network-reachable attach point; clients (desktop, phone, others) are views onto it, not its host.

## Architecture principle: super long-running agents, context by delegation (stated 2026-09-12)

Future-proof the agent for how agents will be used in the future of work: they run for very long periods and handle context properly.

- The main chat can grow without limit. Most context is managed by delegating the right information to sub-agents, not by holding it in the main chat.
- Spin off a sub-agent with direction, not a full dump: "here is the project folder, here is what to work on, here is a compressed context of what you should know, go." The sub-agent then pulls in any relevant context from the local file system, across a very long history of work on the project.
- Master file: the main chat keeps the overarching goals of the project in one master file that all agents can refer to (same idea as Cursor's Projects context). In this Project that is `project-context.md`.

## Desktop app principle: look and feel like Cursor, every detail (stated 2026-09-12)

The most important thing for the desktop app is that it looks and feels like Cursor down to every detail.

- Chat box. Voice recording, including inputs appearing as they arrive.
- Working status that flashes and tells the user the agent is going.
- Background agents shown just above the chat.
- Sub-agent status shown and highlighted while they work.

Process: a parity agent runs full time. It drives Cursor's agent chat, records how it looks and feels, runs the same prompts through Arbos's chat, and compares. Every gap becomes a task. Any change to the app runs this parity suite as a check.

## Full-time QA and self-improvement loop (stated 2026-09-12)

A QA agent runs continually. It uses Arbos to solve coding problems and do ordinary tasks while trying to break it: crash the apps, make state inconsistent, find gaps.

- Every attempt is recorded as a rollout with Arbos's tracing tooling.
- Rollouts that show a break go to fix agents, which open PRs into Arbos.
- PRs are merged automatically and pushed to `main` (gate: CI green and the parity suite green; Jacob asked for auto-merge).
- This is a recursive, full-time loop whose job is to keep making Arbos better.

## Full-time features agent: parity with Cursor Projects (stated 2026-09-12)

A features agent runs full time. Its main goal: make Arbos feature complete with Cursor Projects.

- Think through everything agents in Cursor Projects can do, including the nice-to-haves. Build those features in Arbos.
- Test each feature and try to break it. Tell the QA agent what it is doing so QA can target it.
- Everything is recorded on GitHub as well-documented PRs that merge into the `rust` base.

## Desktop app layout (decided 2026-09-13; supersedes the 2026-09-12 sketch)

1. No left sidebar at all.
2. Top bar with tabs. Each tab is a project. Cycle with Cmd+Shift+{ and Cmd+Shift+}.
3. Opening the app lands on a new tab whose project root is `~/.arbos`.
4. A new tab (Cmd+T) picks a machine and folder with the existing picker; that folder is the project's home.
5. Closing a tab closes the project. Opening the same folder again reopens it with its state intact.
6. Each project has one main chat. Sub-agents the main chat creates appear in the right-hand panel.
7. The right-hand panel is a view of the project's `.arbos/`: agents and their sub-agents (nested), processes the agents started, resources they use, plus the project goals and notes (the Cursor Projects idea). Clicking a sub-agent opens its chat.
8. Otherwise the panel is clean: nothing but a settings button at the bottom.

## Architecture principle: file system holds the state (stated 2026-09-12)

Make Arbos's code state light and its file-system state heavy.

- Any agent can read the file system and see everything: what is happening, the plans, the other agents in the project, how to write to and read from them, and how to wake them.
- The full agent state is wakeable, git-saveable, and rewindable (go back in time from a saved state).
- Tests: write a complex system state into the file system, then run Arbos on it.
- Plan structure, crons, and agent wake-up all live in this model. Replaces today's built-in wake/plan system.

## Codebase facts

- Repo: `unarbos/arbos`. Active branch for this work: `rust`.
- Desktop app: `desktop/` crate (binary `arbos-desktop`, crate `cydonia`), built on gpui via `bezel`. Spawns `arbos-kernel serve` per folder.
- Linux launch needed `x11`/`wayland` features on `gpui_platform` (fix PR in progress).
