> **RECOVERED, BADLY INCOMPLETE — this is not the original file, and it is a fragment that begins mid-table.** 17,853 of the original 71,361 bytes, and three days stale. Source: a `read_file` in the features worker's transcript at 2026-09-13 02:47 UTC. The features worker (`bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027`) holds the full content in its own history and should replace this file wholesale; do not treat anything below as current.
>
> The original was lost together with the whole `docs/` directory on 2026-09-16 between 07:43 and 09:01 UTC. Restored by the store-recovery worker `bc-0b112226-cf98-5cab-92c3-2671518dd9b9`. Cause, timeline and the full recovery inventory: `internal/store-docs-loss-2026-09-16.md`.

| K-02 | **Remote spawn**: `spawn host=<name>` runs the child on another machine | Cloud by default, local when needed ([changelog](https://cursor.com/changelog/projects)) | **Shipped, PR #33** (with K-05): `remote.rs` — sync, install kernel, start, tunnel, relay, `remotes.json`; verified on ArbosLife | Follow-ups: per-child remote places, hub/wss carrier (F-06), macOS (Jacob's Mac) | L | P0 | — |
| K-03 | **Steer a running child**: `say mode=steer` injects at the next tool boundary; receipt says so | You can message a subagent's chat mid-run | **Shipped, PR #8**: `SayMode::Steer`, `Steer` enum on `TurnControl`, end-of-turn drain re-queues leftovers | Review + merge | S | P0 | — |
| K-04 | **Subscriptions / doors**: GitHub PR + CI events, timers with wall-clock anchor, Slack/Discord, webhooks wake an agent | Watch a Slack channel, follow PRs, fix CI, run on schedule ([capabilities](https://cursor.com/docs/cloud-agent/capabilities)) | **PR follow shipped, PR #32** (`subscribe` + poller, stacked on #30) | Still open: CI runs by branch, Slack/Discord door, webhooks, `at=` wall-clock (F-03) | M | P0 | F-03 |
| K-05 | **Machine registry**: named machines with capabilities, readable by agents | Cloud / self-hosted / local picker | **Shipped, PR #33**: `~/.config/arbos/machines.toml`, roster in the prompt, `hub` field reserved | Desktop Opener could read it too | S | P1 | — |
| K-06 | **Desktop screenshot tool** (whole screen/window, not only the browser page) | Cloud agents attach screenshots/videos ([capabilities](https://cursor.com/docs/cloud-agent/capabilities)) | **Shipped, PR #16**: `screenshot.rs`, screencapture/grim/import/scrot backends, images/ + pixels to the model | Follow-up: recording (A-01) | S | P0 | — |
| K-07 | **Git identity + base-branch guard**: refuse commits under an unverified identity, PR targets the configured base | Cursor sets author and base per run | **Shipped, PR #18**: `git_guard.rs` before every bash call; `.arbos/git.toml`; prompt line restored | Follow-up: read base from project file (F design) | S | P0 | — |
| K-08 | **Secrets door**: vault keys into bash's environment, values redacted from tool results | n/a (Cursor injects env secrets and redacts `[REDACTED]`) | **Shipped, PR #30**: `secrets.toml`, `secret` tool, kernel-wide store, model key protected | Follow-ups: per-agent scoping, redaction of door messages, sandbox (P-07) for encoded leaks | M | P0 | — |
| K-09 | **Child result join**: `spawn wait=true` returns the child's first report as the tool result | Subagent returns one final message to the parent ([subagents docs](https://cursor.com/docs/subagents)) | **Shipped, PR #35** | — | S/M | P1 | — |
| K-10 | **Custom agent definitions**: `.arbos/agents-defs/<name>.md` with model, allowlist, prompt, usable as `spawn kind=<name>` | `.cursor/agents/*.md` custom subagents | `Agent::root` template only (`agent.rs:66-78`) | Parse front matter, apply on spawn | S | P1 | — |
| K-11 | **Depth-2 nesting cap to match Cursor** (today 3), and 8 → configurable children | main → subagent → child, then stop | `MAX_DEPTH = 3`, `MAX_CHILDREN = 8` (`sched.rs:11-12`) | Config keys; keep 3 as Arbos default (a coordinator wants depth) | S | P3 | — |
| K-12 | **PR tracking**: agents record PRs they open (`prs.jsonl`), UI shows "PRs N" | "PRs 5" pill ([parity report](./cursor-parity-report-2026-09-12.md) row 4) | Nothing | Bash hook parses `gh pr create` output; `Frame::Tree` carries PR count | S | P1 | U-06 |

### File-system-first state (owned by the design worker; listed for the map)

| ID | Feature | Cursor does | Arbos today | Design section | Size | Pri |
| --- | --- | --- | --- | --- | --- | --- |
| F-01 | Messages as inbox files; steer durable | Follow-ups queue in the thread | In-memory steer queue, inbox nodes | "Messaging", "Wake" | L | P0 |
| F-02 | `GOALS.md` master file, root-owned, injected first | Shared context files ([changelog](https://cursor.com/changelog/projects)) | `AGENTS.md` head only | "`GOALS.md`" | M | P0 |
| F-03 | Plans and crons as `plan/NNNN.md` with `every`/`at` | Scheduled subscriptions | `plan.jsonl` nodes | "Plans", "Crons" | L | P0 |
| F-04 | `journal.md` + checkpoints; user-scannable status | `notes.md` in the store | `plan.md` render | "Journal", "Checkpoints" | M | P0 |
| F-05 | Durable asks/approvals in `waiting/` | Questions persist in thread | oneshot channels | "`waiting/`" | M | P0 |
| F-06 | Remote attach: clients are views; kernel is the writer | Remote desktop / attach | TCP loopback + ssh tunnel | "Remote attach and sharing" | L | P1 |

### Desktop UI (parity report top gaps; agent-window redesign owns U-01..U-04)

| ID | Feature | Cursor does (source) | Arbos today | Gap | Size | Pri | Deps |
| --- | --- | --- | --- | --- | --- | --- | --- |
| U-01 | **Sub-agent status line + right task panel**: inline "N Working — title" under the parent status; right panel lists tasks (circle, glyph, title, check when done); finished sub-agents stay; **no raw ids in labels** | Parity report row 5 | **Shipped, PR #21**: `children_lines`, Tasks section in the context rail, `who_label`, no reap/hide of kernel children | Open beside the parent (tabs) stays with the redesign (U-03) | L | P1 | — |
| U-02 | **"Working N" / "PRs N" pills above the composer** | Parity row 4 | **Shipped, PR #23**: `detail.rs::pills`, PR URLs scanned from tool output | K-12 would make the PR count exact | M | P1 | — |
| U-03 | **Tab strip + right panel**: a sub-agent opens beside its parent | Parity row 14 | Single header row, `…` menu (`root.rs`) | Tabs over the chat column; right panel toggle | L | P1 | redesign |
| U-04 | **Right-aligned user message card** (white, shadow, capped width, pencil on hover) | Parity row 7 | **Shipped, PR #27**: `surface_card` plane, 82 % cap, hover pencil | — | S | P1 | — |
| U-05 | Under-composer row: machine label + spinner; split mic from send | Parity row 1 | Missing | S | P2 | K-05 |
| U-06 | "Worked 12s" elapsed, "Thought Ns", verb vocabulary | Parity row 3 | "Worked +10" | S | P2 | — |
| U-07 | Sidebar time buckets, status dots, Search | Parity row 6 | Folder grouping | M | P2 | — |
| U-08 | Thumbs up/down + relative time under a turn | Parity row 8 | copy + branch only | S | P3 | — |
| U-09 | Queued follow-up shown under the active turn, reorder, Enter=queue / ⌘Enter=send | Parity row 10 | Queue bar only | M | P2 | F-01 |
| U-10 | Settings › Model: masked key field (typed entry), Custom base URL field, live model search | PR #6 uses clipboard paste only | `TextField` has no masking (`vendor/bezel-ui/src/input.rs`) | Add a `masked` mode to bezel `TextField` | S | P2 | M-01 |

### Voice, artifacts, attach

| ID | Feature | Cursor does | Arbos today | Gap | Size | Pri | Deps |
| --- | --- | --- | --- | --- | --- | --- | --- |
| V-01 | Voice dictation on Linux and desktop (kernel-side capture), errors inline in the composer | Parity row 2 | macOS Swift helper only (`voice.rs`, `voice_dictate.swift`); door `voice_*` uses `rec`/`sox` | Provider-agnostic STT (open-source first per project-context) | L | P0 (benchmark #1) | M-01 |
| A-01 | Artifacts in chat: screenshots/recordings the agent produced, click to open, saved under the place | Screenshots/videos on PRs ([capabilities](https://cursor.com/docs/cloud-agent/capabilities)) | `Tool.images` shown inline for browser shots | Recording (`ffmpeg x11grab`/`screencapture -v`) + gallery row | M | P1 | K-06 |
| A-02 | Try Live: attach the desktop to a remote agent's screen | Remote desktop control | ssh tunnel to kernel only | VNC/noVNC or the phone/desktop attach from F-06 | L | P2 | F-06 |

## Tier 2: premier-agent capabilities (beyond Cursor Projects)

What Hermes, Codex, Cursor (editor agent), and Claude Code have that a coordinator product does not spell out. Marked P2 unless the benchmark needs them.

| ID | Feature | Who has it | Arbos today | Gap | Size | Pri |
| --- | --- | --- | --- | --- | --- | --- |
| P-01 | **Hooks with policy**: pre/post tool hooks that can block, rewrite args, or inject context (JSON in/out, per tool matcher) | Claude Code hooks; Codex `notify`; Cursor rules | `hooks/before-tool`, `hooks/after-turn` executables (`file_hooks.rs`) | Matchers, exit-code semantics (block/allow/ask), JSON payload with tool + args | M | P2 |
| P-02 | **Permission modes**: plan-only / ask-on-write / auto-accept edits / full auto, switchable mid-session | Claude Code modes; Codex approval policies; Cursor YOLO | `readonly` flag + bash approval (`hooks.rs:912-922`) | Mode enum on `agent.md`, composer switch | S/M | P1 |
| P-03 | **Skills / commands**: `/name` slash commands from `skills/*/SKILL.md` with arguments, auto-loaded when relevant | Claude Code skills + slash commands; Codex skills | Skill names in prompt (`prompt.rs`, `skill_names`) | Load SKILL body on `/name`, argument passing, registry of skills per place | S/M | P2 |
| P-04 | **Memory across sessions**: durable notes per project + per user, auto-recalled | Claude Code `CLAUDE.md` + auto-memory; Hermes memory; Cursor memories | `AGENTS.md`; compaction summaries in JSONL | `.arbos/memory.md` written by the agent with a `remember` tool; injected after GOALS | S | P2 |
| P-05 | **Session resume / fork / rewind**: `--resume`, `/fork` (exists), rewind to a turn with file state | Claude Code `--resume`, rewind checkpoints; Codex resume | `/fork` exists; `undo` via git checkpoint (`tools/git.rs:45-62`) | Rewind = F-05/F-01 git-saved `.arbos/` + checkout | M | P2 (F design) |
| P-06 | **Headless / CLI chat**: `arbos-kernel run "<prompt>"` prints the answer, `--json` events; usable from scripts and CI | Claude Code `-p`, Codex `exec`, `cursor agent` CLI | **Shipped, PR #15**: `run` + `attach`, exit codes 0/1/2/3/4, `Event.seq` on the wire | Follow-ups: `answer` subcommand for scripted asks (P-06b); default timeout | S | P0 |
| P-07 | **Sandboxing**: bash in a sandbox (bubblewrap/seatbelt), network allow-list per agent | Codex sandbox; Claude Code sandbox | none; `readonly` restricts tools only | `bwrap` on Linux, `sandbox-exec` on macOS, config per agent | M | P2 |
| P-08 | **MCP servers per project**, HTTP + stdio, OAuth | All four | One stdio server via `ARBOS_MCP_CMD` (`doors.rs`, `tools.rs:460-514`) | `.arbos/mcp.toml` list; HTTP transport | M | P2 |
| P-09 | **Background bash with output streaming into the transcript** | Claude Code background tasks; Codex | Jobs (`jobs.rs`), `await`/`jobs`, detached after `bash_wait_ms` | Live tail into the UI job row; `Frame::Job` | S | P2 |
| P-10 | **Plan mode with approval**: agent proposes a plan file, user approves, then it executes | Claude Code plan mode; Cursor plan | `plan` tool + `plan.md` (`tools.rs:139-253`) | "Plan only" permission (P-02) + approve gate frame | S | P2 |
| P-11 | **Image + PDF input, vision** | All four | Attachments as `image_url` (`provider.rs:148-150`) | PDF to text/images; drag-drop in desktop exists | S | P3 |
| P-12 | **Prompt caching + cost accounting per session** | Claude Code, Codex | Anthropic cache breakpoints (`provider.rs:944-949`) | OpenRouter `cache_control` for more vendors; $ in usage (M-03) | S | P3 |
| P-13 | **Web search with citations** | Hermes, Cursor, Codex | `fetch`, `search` (`tools/web.rs`) with `search_url` config | Default search backend via OpenRouter `:online` or Exa; cite URLs in answers | S | P2 |
| P-14 | **Agent-to-agent protocol for outside agents** (ACP server, A2A) so Claude Code / Codex can be workers | Cursor runs custom subagents; Hermes multi-agent | Desktop is an ACP *client* (`desktop/src/agent/acp.rs`) | `spawn agent=claude-code` launching an ACP agent as a child with the same folder contract | M | P2 |
| P-15 | **Telemetry / tracing for rollouts** (QA loop): every turn as a replayable trace | Codex/Claude Code logs; Cursor transcripts | `trace: true` per-call JSON (`provider.rs` `Trace`) | `--provider replay` from the F design; export a rollout bundle | M | P1 (QA) |

## Shipped

| Date | ID | PR | Notes |
| --- | --- | --- | --- |
| 2026-09-12 | M-01 | [#6](https://github.com/unarbos/arbos/pull/6) | OpenRouter default; `provider` config; `arbos-kernel setup`; Settings › Model paste-key flow; no-key turns show a notice. Screenshots in `media/features/openrouter-settings-model-*.png`. |
| 2026-09-12 | K-03 | [#8](https://github.com/unarbos/arbos/pull/8) | `say mode=steer`: into a running agent's current turn at the next tool boundary; idle → a turn; leftovers re-queued at turn end. Capture in `media/features/steer-running-child-capture.md`. |
| 2026-09-12 | K-01 | [#9](https://github.com/unarbos/arbos/pull/9) | `spawn isolate=worktree`: child gets `.arbos/worktrees/<id>` on branch `arbos/<id>`; clean refusals; prompt note; removal receipt. Capture in `media/features/spawn-worktree-isolation-capture.md`. |
| 2026-09-12 | P-06 | [#15](https://github.com/unarbos/arbos/pull/15) | `arbos-kernel run` / `attach`: headless prompt with exit codes, `--json` transcript lines, auto-start kernel; `Event.seq` on the wire separates records from live emits. Capture in `media/features/headless-kernel-run-capture.md`. |
| 2026-09-13 | K-06 | [#16](https://github.com/unarbos/arbos/pull/16) | `screenshot` tool: screen/window PNG to `images/`, attached as pixels; platform backends; clear no-display errors. PNG in `media/features/screenshot-tool-desktop-capture.png`. |
| 2026-09-13 | K-01b | [#17](https://github.com/unarbos/arbos/pull/17) | File tools confine a worktree child to its worktree; grep scoped and de-duplicated per side. Capture in `media/features/worktree-cwd-confinement-capture.md`. |
| 2026-09-13 | K-07 | [#18](https://github.com/unarbos/arbos/pull/18) | Git guard before bash: identity, protected branches, `gh pr create --base`; `.arbos/git.toml`; prompt line. Capture in `media/features/git-guard-capture.md`. |
| 2026-09-13 | U-01 | [#21](https://github.com/unarbos/arbos/pull/21) | Sub-agent lines under the status, Tasks rail, finished children stay, titles instead of ids. Parity suite run `media/parity/arbos/u01-after/`; before/after in `media/features/subagent-task-panel-*.png`. |
| 2026-09-13 | U-02 | [#23](https://github.com/unarbos/arbos/pull/23) | "Working N" / "PRs N" pills above the composer. `media/features/composer-pills-*.png`. |
| 2026-09-13 | U-04 | [#27](https://github.com/unarbos/arbos/pull/27) | User prompt as a right-aligned card. `media/features/user-message-card-{before,after}.png`. |
| 2026-09-13 | Q-01 | [#29](https://github.com/unarbos/arbos/pull/29) | QA proposal: CONTRACT rules for "show me" → image and fix-on-branch → commit; browser description; `changes` opens with a branch status line. `bench-*` pass; kickoff replay inconclusive here (qa-007). |
| 2026-09-13 | K-03+ | [#8](https://github.com/unarbos/arbos/pull/8) | Reconciled with QA's #28: `take_steers()` drains every waiting steer at a boundary; `steer-storm` 25/25. |
| 2026-09-13 | K-08 | [#30](https://github.com/unarbos/arbos/pull/30) | `secret` tool + `.arbos/secrets.toml`; values into bash's env, `[REDACTED:NAME]` in every tool result; model key protected. `media/features/secrets-door-capture.md`. |
| 2026-09-13 | K-04 | [#32](https://github.com/unarbos/arbos/pull/32) | GitHub door: `subscribe add/list/remove`, poller wakes the subscriber on PR changes. Stacked on #30. `media/features/github-door-capture.md`. |
| 2026-09-13 | P-06b | [#15](https://github.com/unarbos/arbos/pull/15) | `arbos-kernel answer` (+ `--follow`, `--approve/--deny`) added to the CLI PR. `media/features/headless-answer-capture.md`. |
| 2026-09-13 | K-05+K-02 | [#33](https://github.com/unarbos/arbos/pull/33) | `machines.toml` registry + `spawn host=<name>` over SSH: sync, install, start, tunnel, relay; verified on ArbosLife under `/home/const/arbos-remote`. `media/features/remote-spawn-capture.md`. |
| 2026-09-13 | M-03 | [#34](https://github.com/unarbos/arbos/pull/34) | Fallback on provider-own errors; OpenRouter default fallback list; `["none"]` opt-out. `media/features/fallback-models-capture.md`. |
| 2026-09-13 | K-09 | [#35](https://github.com/unarbos/arbos/pull/35) | `spawn wait=true` returns the child's first report; `wait_secs`. `media/features/spawn-wait-capture.md`. |

## Next up (in order)

1. **K-02b per-child remote places + macOS worker** (M, P0, benchmark #6 "Jacob's Mac for iOS builds"): `<dir>/<project>-<child>/`, `Darwin arm64` binary from a release build.
2. **A-01 recording artifacts** (M, P1): `ffmpeg x11grab` / `screencapture -v` alongside the screenshot tool.
3. **P-02 permission modes** (S/M, P1): plan-only / ask-on-write / auto, switchable mid-session.
4. **K-10 custom agent definitions** (S, P1): `.arbos/agents-defs/<name>.md` → `spawn kind=<name>`.
5. **M-03b cost per turn** (S, P2) and same-model retry without `reasoning_details`.
6. U-03 tab strip + right panel: on hold until Jacob decides the agent-window layout.

Check `internal/features-inbox/` every turn for QA proposals.

## Learned (2026-09-13, remote batch)

- ArbosLife already runs the Go-era Arbos with `~/.config/arbos/{identity,secrets,settings.json}` and QA's loop uses `~/.cache/arbos`. Remote kernels therefore run with `XDG_CONFIG_HOME`/`XDG_CACHE_HOME` under the dedicated `/home/const/arbos-remote` so nothing of ours lands in the account's dot-folders.
- A kernel started in the background rewrites `kernel.json` a moment after start; a start script that reads the file must delete the stale one first, or a tunnel to a dead port looks like a link that "closed at once". The relay now also waits for the kernel's greeting frame before calling a link attached.
- `pkill -f '<pattern>'` from a shell whose own command line contains the pattern kills the shell. Use `[p]attern`. (Third time; now in the QA notes too.)

## Learned (2026-09-13, UI batch)
