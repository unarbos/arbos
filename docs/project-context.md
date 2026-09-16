> **RECOVERED, INCOMPLETE — this is not the original file.** 9,586 of the original 18,990 bytes. Source: a full `read_file` of this document captured in the symmetry worker's transcript at 2026-09-13 06:15 UTC. Everything added between 2026-09-13 06:15 and the last known write at 2026-09-15 13:28 UTC is missing — roughly the second half of the document, including later dated decisions.
>
> The original was lost together with the whole `docs/` directory on 2026-09-16 between 07:43 and 09:01 UTC. Restored by the store-recovery worker `bc-0b112226-cf98-5cab-92c3-2671518dd9b9`. Cause, timeline and the full recovery inventory: `internal/store-docs-loss-2026-09-16.md`.
>
> The coordinator has since re-entered the decisions from 2026-09-13 to 2026-09-16 in the section near the end, from its own record of making them. Those decisions are accurate; their wording is not the original's.

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

## Decisions from 2026-09-13 to 2026-09-16 (re-entered by the coordinator after the store loss)

These were in the lost half of this file. Re-entered from the coordinator's own record of making them; the wording is not the original.

### Adopt Cursor's agent model (2026-09-13)

Jacob's rule: "open source Cursor which just works is good enough for v1." Every choice that moves Arbos towards Cursor's shape is a yes.

1. The plan engine is gone, replaced by `plan.md` plus `subscriptions/`. Subscriptions — timer, PR, CI, chat, inbox, shell — are the only scheduler.
2. Root is a coordinator that does not edit code. It keeps `bash` for the one quick command the user asks to see run, or a read-only probe; builds, test runs and edits belong to a worker.
3. Kernel-run shell jobs with no model turn stay, as shell subscriptions.
4. `ask` parks the turn; the answer wakes a new one. `approve` stays blocking.
5. Worker-to-worker `say` stays, with a hop limit. `say to=user` is refused — the user reads the reply.
6. The kernel writes a `done` inbox file to the parent at every child turn end.

### Two chat styles (2026-09-14)

The project's main chat renders in Cursor's Project style — prompt, folded "Worked" header, worker lines, short prose, no thinking blocks or tool cards. Every delegated worker chat renders in the classic agent style, with thinking, tool cards and diffs. Rules live in `docs/project-chat-vs-agent-chat.md`.

### Process parity, not only style (2026-09-14)

Arbos's agents must do the same things Cursor's do: same tool roster, same turn loop, same store files filled the same way, same notes and project-board algorithm, same delegation rules. Reference: `docs/cursor-coordinator-spec.md` and its tools appendix.

### Default permissions are full auto (2026-09-15)

Jacob: "Default is go go go." Every agent defaults to `permission = auto` with no approval cards; protected-file asks apply only in `ask` mode. Only hard safety refusals remain — a commit on a protected branch, a force-push, destructive commands outside the place. An agent's own bookkeeping never asks.

### The iPhone app's shape (2026-09-15)

Individual projects, each with its own icon and name; a chat where Jacob types, dictates and attaches files; and a call screen for full duplex over AirPods. Simple and elegant in the Mac app's style — not a copy of Cursor Mobile. The call screen follows Jacob's GPT voice-mode reference: voice-first with a reactive orb, composer on pull-down.

### Federated stores across machines (2026-09-15)

Jacob: "keeps it on its own but when arbos nodes discover each other they will know where to read and write to." Every node keeps its own `.arbos/`; there is no central store and no copying a parent's store to a child's machine. Discovery carries store locations, so a node that meets another learns where its stores are and may read and write them across the link. A spawn brief hands a child addresses, not inlined context, including across machines. Addresses are `arbos://<machine>/<project>/<path>`, always into a store and never into a checkout; an unreachable peer fails loudly, with no cache.

Coordinator rulings, open to Jacob's veto: a remote agent may not write another node's `notes.md` (each root owns its page; peers propose with `say`); projects default to `mesh` sharing while every node is Jacob's, and to `private` as soon as another person's token joins; a remote child's deliverables land in the parent's store by address. Design: `docs/arbos-mesh-design.md` part 3.

### The Linux build is the desktop rig (2026-09-16)

Jacob: "just use the linux build for the desktop." No Mac leg in the symmetry loop. This is sound because the app now bundles its own font (Inter, every weight), so type, weight and spacing render identically on both platforms. Genuinely untested on Linux, and tracked as such: Retina pixel scale, macOS window chrome, native menus, system permission dialogs, Apple's text smoothing.

### Updates install themselves (2026-09-16)

Every merge to `main` publishes a build signed with Jacob's Developer ID, notarised, stapled and Gatekeeper-checked, unattended. The desktop app carries a bottom-left bar with a filled blue Update button; the iPhone app arrives through TestFlight the same way. A build that cannot reach its hub, or whose number disagrees with its bundle, never publishes.

### Secrets are never printed (2026-09-16)

After four leaks in one day, every one from a worker masking output it had already decided to print: masking is banned as a technique, because it fails on the one variable nobody anticipated and on values that sit a line below their label.

- Never fetch a whole vault item (`op item get` in any format). Read the one field by reference — `op read "op://Arbos/<item>/<field>"` piped into the consuming command, or `op document get` to a 0600 temp path that is deleted after use.
- Never print an env file, a vault field of free text, or a credential-bearing config, masked or not. Learn what exists from field labels and file names.
- Never display the output of a command that could contain a secret; redirect it to a file and search it for what you expect.
- A credential is proved by the result of using it, never by showing it.

### The store is never the only copy (2026-09-16)

The store service deleted `docs/` and `artifacts/` by itself, and the only documents that survived intact were those mirrored into git. So a document is built in `/tmp` and copied in rather than rewritten in place, and `docs/` is mirrored to the orphan branch `store-docs` on `unarbos/arbos`, with `docs/` at its root so paths read the same in both places.

After writing anything in `docs/`, run:

```bash
bash /cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/mirror-docs.sh
```

Do not start another mirror branch. The script refuses to push when the store looks damaged — a refusal means the store faulted again or someone deleted a document, and it can restore from the branch. Details in `internal/store-docs-mirror.md`.

## Codebase facts

- Repo: `unarbos/arbos`. Work merges to `main`, with `rust` kept fast-forwarded to match. An hourly merge steward merges green PRs.
- Desktop app: `desktop/` crate (binary `arbos-desktop`), built on gpui via `bezel`. Spawns `arbos-kernel serve` per folder, with the kernel embedded in the macOS bundle.
- Linux launch needed `x11`/`wayland` features on `gpui_platform`; fixed.
- The mesh: `arbos-hub` plus per-machine kernels. The hub and Jacob's phone kernel run on ArbosLife behind a named Cloudflare tunnel; only the voice model needs rented GPU hardware.
- **Tests: wait for each fact you assert, never infer a later step from an earlier one, and poll rather than sleep** (QA loop, 2026-09-16). Four kernel e2e flakes in one week had the same shape — a check that took one event as proof of the next: `recreate_e2e` waited for the turn's `idle` frame and then for the `user` event, but the wait consumes frames and the event sometimes came first (#149); `standing_pass_e2e` read the transcript once after the `rewound` frame while the cut was still being written (#168, which also made the cut atomic); several tests slept 300–500 ms after `idle` before reading a file the kernel was still finishing (#170, `common::wait_for`); `goals_e2e` took "the goal file is gone" as proof the `Goal met` wake had landed, one scan later (#313). A test that fails a tenth of the time teaches everyone to re-run rather than to look. So: one `wait_for(pred)` per fact, on the file or frame the assertion reads; a bounded poll for "nothing happens" checks; a fixed `sleep` only to place an action in time, never to wait for a result.
