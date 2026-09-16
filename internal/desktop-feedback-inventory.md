---
cursor:
  subagentId: "bc-42446f90-dcf3-5b7b-a230-88a5cf278068"
---

# Desktop in-app feedback — existing plumbing inventory

Exploration of `/workspace` (unarbos/arbos) for reusing kernel logs, transcripts, versions, API surface, redaction, and screenshots. Read-only audit; no secrets printed.

---

## 1. Kernel logging

### Where it writes

| Item | Path / mechanism |
|------|------------------|
| **Primary log** | `<place>/.arbos/runtime/kernel.log` |
| **Path helper** | `klog::log_path_for(arbos_dir)` → `arbos_dir.join("runtime").join("kernel.log")` |
| **Init** | `klog::init(path)` called once from `serve::run` |
| **Pointer in metadata** | `kernel.json` field `"log"` set to the full path (see `write_kernel_json`) |
| **Legacy** | Remote spawns may still use `<place>/.arbos/kernel.log` (stdout redirect in `remote.rs`); runtime layout is canonical since Phase 1 |

```1:11:crates/arbos-kernel/src/klog.rs
//! The kernel's own log: `.arbos/kernel.log`, one JSON object per line.
//!
//! The transcript records what agents did. This records what the kernel
//! did around them: start and stop (pid, version, git sha), every turn's
//! start and end, every frame it refused and why, every error it used to
//! print to stderr and nowhere else. A rollout that has the transcript and
//! this file can say why a prompt did nothing.
//!
//! Lines: `{"ts":<ms>,"level":"info|warn|error","event":"<name>",
//! "agent":"<id>"?, "detail":"<text>"}`. Warnings and errors also go to
//! stderr, as before.
```

(Comment says `.arbos/kernel.log`; implementation uses `runtime/kernel.log` — see lines 106–108.)

```106:108:crates/arbos-kernel/src/klog.rs
pub fn log_path_for(arbos_dir: &Path) -> PathBuf {
    arbos_dir.join("runtime").join("kernel.log")
}
```

### Format

- **JSON Lines** (one object per line), not plain text or `tracing_subscriber` to file.
- Fields: `ts` (ms), `level` (`info`|`warn`|`error`), `event` (name), optional `agent`, `detail` (string).
- Warnings/errors also echo to stderr.

### Rotation

- On kernel **start**, if file size **> 16 MiB**, rename to `kernel.log.1` (single generation).

```20:44:crates/arbos-kernel/src/klog.rs
const ROTATE_AT: u64 = 16 * 1024 * 1024;
// ...
    if meta.len() > ROTATE_AT {
        let _ = std::fs::rename(&path, path.with_extension("log.1"));
    }
```

### HTTP / CLI to read recent log

| Mechanism | What it actually does |
|-----------|------------------------|
| **`GET /` or `GET /healthz`** on attach port | JSON health only (`kernel`, `git_sha`, `built_at`, `protocol`, `attach`, `auth`) — **no log body** |
| **`arbos-kernel log [-n N] [place]`** | **`git log` of `.arbos/` repo**, not `kernel.log` (`snapshot::log`) |
| **Attach `Frame::Tail`** | Client → kernel: tail bytes of any file under `.arbos/`, including `runtime/kernel.log` |
| **Attach `Frame::Read`** | Whole file up to 1 MiB (`READ_CAP`); truncated flag if larger |
| **Direct filesystem** | Desktop on local places can read the path; SWE harness copies it (`arbos-swe-run`) |

**No dedicated `/logs` route or `arbos-kernel logs` subcommand** for operational `kernel.log`.

### Fetching last N lines / last N minutes

1. **Last N lines (remote client):** send `{"type":"tail","path":"runtime/kernel.log","from":0,"limit":<bytes>}` — default limit 256 KiB, max 1 MiB per chunk; paginate with `from` = previous `Chunk.to`. Lines are cut at newline boundaries.
2. **Last N lines (local desktop):** read file directly or use `tail` frame over loopback attach.
3. **Last N minutes:** no built-in filter — parse JSONL and filter on `ts` (milliseconds since epoch).

```245:292:crates/arbos-kernel/src/files.rs
fn tail(place: &Place, rel: &str, from: u64, limit: u64) -> Frame {
    // ...
    let limit = if limit == 0 {
        TAIL_DEFAULT
    } else {
        limit.min(TAIL_MAX)
    };
    // ... line-boundary trim when not at EOF
}
```

---

## 2. Transcript / trajectory storage

### Primary store

| Item | Location |
|------|----------|
| **Per-agent transcript** | `<place>/.arbos/agents/<id>/transcript.jsonl` |
| **Archive rolls** | `<place>/.arbos/agents/<id>/transcript-archive/NNNN.jsonl` when line count exceeds `transcript_roll_lines` (default 10_000) at turn end |
| **Finished workers** | `<place>/.arbos/archive/agents/<id>/transcript.jsonl` |
| **Per-agent thumbs** | `<place>/.arbos/agents/<id>/feedback.jsonl` (desktop vote UI, not user bug reports) |
| **Rollout export** | `arbos-kernel rollout export` copies transcript + feedback + trace |

```47:49:crates/arbos-core/src/files.rs
    pub fn transcript(&self) -> PathBuf {
        self.dir.join("transcript.jsonl")
    }
```

### Schema (`Event` / `EventKind`)

Defined in `crates/arbos-core/src/event.rs`. Each JSONL line is one `Event`:

- Top-level: `ts`, `kind` (tagged enum), optional `seq` (1-based line number when loaded/replayed).
- **Tool calls:** `kind: "tool"` → `ToolRec` with `name`, `call_id`, `args` (JSON), `body` (full result text on disk), `error`, `paths`, `images`, `started`/`ended`, `label`, `child`, etc.

```153:191:crates/arbos-core/src/event.rs
pub struct ToolRec {
    pub name: String,
    pub call_id: String,
    // ...
    pub body: Option<String>,
    pub args: Option<serde_json::Value>,
    pub child: Option<String>,
    // ...
}
```

Tool results are persisted with **arguments and full body** (redacted before write — see §5):

```106:121:crates/arbos-engine/src/batch.rs
        Event::new(EventKind::Tool(ToolRec {
            name: call.name.clone(),
            call_id: call.id.clone(),
            // ...
            body: Some(body),
            args: Some(call.arguments.clone()),
            // ...
        }))
```

### Agent tools (coordinator read, not user feedback)

| Tool | File | Purpose |
|------|------|---------|
| **`transcript`** | `crates/arbos-kernel/src/tools.rs` ~220–427 | `mode: tail|full`, `max_turns` — prose rendering; **does not include full args/body** in render (name, paths, error only) |
| **`agents`** | same file ~70–187 | Worker status (running/idle/archived, last turn summary) — **no delete** |

**Note:** Project notes reference PR #207 `agents/transcript/delete` tools; **no `delete` op on agents/transcript exists in this tree**. Chat deletion is desktop `rm -rf` on agent folder (see `deleted_mid_turn_e2e.rs`).

### Live mirroring / HTTP for clients

**Transport:** WebSocket (or TCP) attach protocol — newline-delimited `Frame` JSON (`crates/arbos-core/src/wire.rs`), not REST.

| Client → kernel | Kernel → client |
|-----------------|-----------------|
| `history` (agent, since/before, limit) | `replayed` + `history_end` |
| `read` / `tail` / `list` | `file` / `chunk` / `listing` |
| (desktop local) reads `transcript.jsonl` from disk | `event` frames every 200 ms for new lines |
| | `changed` ~1 Hz when watched files change |

**Attach replay:** last **200** lines of focused agent on connect (`ATTACH_TAIL`).

**Phone (iOS):** `ArbosKernelClient.history(agent:limit:)` — uses `history` frame; worker chats replay once (`ios/Arbos/Chat/LiveKernelChat.swift`).

**Desktop:** primary path is **local file read** (`desktop/src/kernel.rs` `load_transcript`, `desktop/src/agent/acp.rs` ignores `replayed` when it already has files). Remote places use attach + optional `put` for attachments.

**Watch list** includes `agents/*/transcript.jsonl` and `feedback.jsonl` (`crates/arbos-kernel/src/watch.rs`).

### Trajectory for “the turn user complained about”

Reuse options:

1. Parse `transcript.jsonl` from last `user` / `wake` through matching `turn_complete` or `interrupted`.
2. Send `history` with `before: <seq>` and `limit` to page backward from UI-held seq.
3. Agent `transcript` tool tail mode — prose only, capped 12k chars inline.
4. `rollout export` — full JSONL copy.

---

## 3. Version + commit reporting

### Kernel binary

| Source | Mechanism |
|--------|-----------|
| **Semver** | `env!("CARGO_PKG_VERSION")` → `klog::version()` |
| **Git SHA** | `build.rs` → `ARBOS_GIT_SHA` (12-char short); `klog::git_sha()` |
| **Built at** | `build.rs` → `ARBOS_BUILT_AT` (`YYYY-MM-DDTHH:MMZ`); `klog::built_at()` |
| **CLI** | `arbos-kernel --version` / `version` subcommand prints version, sha, protocol |
| **Wire** | `Frame::Hello` fields `kernel`, `git_sha`, `built_at` |
| **HTTP health** | `GET /healthz` same JSON as hello snippet |
| **Registration** | `runtime/kernel.json`: `version`, `git_sha`, `log`, `pid`, `url`, … |
| **Hub roster** | Registrant carries `version`, `git_sha`, `built_at` |

```1:24:crates/arbos-kernel/build.rs
//! Bake the git sha into the binary so `kernel.log` and `kernel.json` say
//! which build wrote them.
// ...
    println!("cargo:rustc-env=ARBOS_GIT_SHA={sha}");
    println!("cargo:rustc-env=ARBOS_BUILT_AT={}", iso_minute(secs));
```

```90:104:crates/arbos-kernel/src/klog.rs
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
pub fn git_sha() -> &'static str {
    option_env!("ARBOS_GIT_SHA").unwrap_or("unknown")
}
pub fn built_at() -> &'static str {
    option_env!("ARBOS_BUILT_AT").unwrap_or("unknown")
}
```

### Desktop app

| Source | Mechanism |
|--------|-----------|
| **Marketing version** | `CARGO_PKG_VERSION` of desktop crate |
| **Build number** | `ARBOS_BUILD` env at packager time, else `git rev-list --count HEAD` (`desktop/build.rs`) |
| **Display** | `build::version_label()` → `"0.2.0 (879)"` style (`Version::human()`) |
| **Commit badge** | `build::COMMIT` (7-char + `-dirty`) for settings badge |
| **Kernel version in bundle** | `build::KERNEL_VERSION` from sibling `arbos-kernel/Cargo.toml` |
| **Status bar** | `desktop/src/view/status_bar.rs` uses `build::version_label()` |

```27:37:desktop/src/build.rs
pub fn version() -> arbos_update::Version {
    arbos_update::Version::parse(env!("CARGO_PKG_VERSION"))
        .unwrap_or_else(|_| arbos_update::Version::new(0, 0, 0, 0))
        .with_build(BUILD.parse().unwrap_or(0))
}
pub fn version_label() -> String {
    version().human()
}
```

### Kernel self-update (staleness by commit)

- **`arbos-updatectl kernel [--install]`** (not `arbos-kernel update` in `main.rs`) compares running binary via `Running::read` → parses `arbos-kernel --version` line; **commit SHA is the staleness signal**.
- Feed ordering uses build number + semver (`crates/arbos-update/src/version.rs`).

### iOS

- Settings shows `CFBundleShortVersionString` + `CFBundleVersion` (`ios/Arbos/Settings/SettingsView.swift`).
- Kernel version from attach `hello` → `kernelVersion` on client.

---

## 4. Kernel HTTP / attach API surface

### Listener

- Single TCP listener per place (`serve.rs` accept loop).
- Default bind: loopback `127.0.0.1:0`; optional `ARBOS_ATTACH_BIND` / `--bind 0.0.0.0:PORT` for tunnel/phone.
- Address written to `.arbos/runtime/kernel.json` (`url`, optional `ws`).

### Plain HTTP (before WebSocket upgrade)

| Method / path | Handler | Response |
|---------------|---------|----------|
| `GET /`, `GET /healthz` | `attach::answer_http` | 200 JSON: kernel, git_sha, built_at, protocol, attach=websocket, auth |
| Other GET | same | 426 Upgrade Required |
| `POST /hook/<agent>` | `serve::webhook` | Inbox injection (Slack/CI); not for desktop |

```415:458:crates/arbos-kernel/src/attach.rs
pub async fn answer_http(/* ... */) {
    let (status, body) = if path_only == "/" || path_only == "/healthz" {
        ( "200 OK", format!("{{\"kernel\":\"{kernel}\",\"git_sha\":\"{}\",\"built_at\":\"{}\", ..."))
    } else {
        ( "426 Upgrade Required", ... )
    };
}
```

### WebSocket / TCP attach (primary)

After `admit()` (`serve.rs` ~1638–1700):

| Peer | Auth |
|------|------|
| **Loopback plain TCP** | Trusted as **owner** (desktop, CLI) — no token |
| **WebSocket** (incl. tunnel) | Token required: `Authorization: Bearer`, `?token=`, or first `{"type":"auth","token"}` |
| **Token source** | `<place>/.arbos/access.toml` `[[client]]` rows |

**Roles** (`access.rs`):

- `owner` / `writer`: all frames except writers cannot `configure` (API key).
- `reader`: only `history`, `auth`, `read`, `tail`, `list`.

### Client → kernel frames (desktop `acp.rs` sends)

`user`, `steer`, `put`, `seen`, `list` (probe), `stop`, `kickoff`, `approve`, `answer`, `configure`, `set_model`, `undo`, `compact`, `rewind`, `pause`, `screen`, `plan_op`, `set_mode`, `focus`, `pty_in`, `voice_start/stop`, `refresh`, `history`, `read`, `tail`, `auth`.

Handled in `serve::handle_frame` (~718+) and per-connection loop for `history`/`read`/`tail`/`list`/`put` (~1909–1954).

### Kernel → client (selection)

`hello`, `snapshot`, `provider`, `replayed`, `history_end`, `event`, `assistant_delta`, `thinking_delta`, `turn`, `ask`, `notify`, `seen`, `error`, `changed`, `job`, `screenshot`, `rewound`, `plan`, `board`, `browser`, `pty`, `working`, `status`, `tree`, …

### Hub (separate process)

`crates/arbos-hub`: routes `/register`, `/attach/<machine>[/<project>]`, `/claim/<machine>` — transparent WebSocket proxy to kernel attach; auth via hub tokens.

### Where to add a feedback endpoint

- **Attach frame** (e.g. `report` client → kernel → hub forward) fits existing auth and desktop `send_frame` pattern.
- **Plain HTTP** only exists for health + webhooks today; new POST would go alongside `webhook` in accept loop (~297–312) or on hub.

---

## 5. Redaction / secret-scrubbing

### Canonical helper

**`arbos_engine::secrets::Store::redact`** (`crates/arbos-engine/src/secrets.rs`):

- Replaces tracked secret values with `[REDACTED:NAME]`.
- Handles base64/hex encodings and 12+ char pieces.
- Applied to **tool bodies/errors before transcript write** (`batch.rs`).
- Applied to job journal tails, chat door posts, configure frame logging (api_key never logged), live `Job` deltas (`serve.rs`).

### Secret door

- Tool: `secret` (`secret_tool.rs`) — grant/revoke; values never shown to model.
- Protocol rule: `[REDACTED:NAME]` in tool results, job streams, subscription output.

### Environment scrubbing

- **`arbos_core::envsafe`**: allowlist for kernel/job env; `scrub_prologue()` shell snippet strips secret-looking vars from login shells unless `ARBOS_GRANTED`.
- **`stray_secrets()`**: warns on unmanaged credential-like env vars at kernel start (names only in `kernel.json`).

### Not secret redaction

- **`scrub_child_claims`** (`files.rs`): strips bogus `child` field on tool events for broadcast — not credential scrubbing.

### Feedback implication

Transcript and kernel log should already have secrets redacted in tool output; **kernel log `detail` strings are generally operational** — still avoid echoing user paste. No dedicated “feedback report sanitizer” exists yet.

---

## 6. Screenshot capability

### Kernel agent tool (`screenshot`)

File: `crates/arbos-kernel/src/screenshot.rs`

| Target | Backend |
|--------|---------|
| `screen` / `window` | **macOS:** `screencapture` (-x, -o, optional -D display); **Linux:** `grim` (Wayland), `import`/`scrot`/`gnome-screenshot` (X11) |
| `text` | Headless Chrome renders terminal-styled PNG (no display needed) |

Output: `.arbos/agents/<id>/images/*.png` — attached to model as pixels.

**Try Live:** `Frame::Screen` → `grab_screen()` → `Frame::Screenshot` (base64 PNG/JPEG, scaled via ImageMagick/sips).

**macOS window capture note:** `frontmost_window_id()` returns `None` — window target often falls back to full screen.

### Kernel `record` tool

`crates/arbos-kernel/src/record.rs` — screen **video** + last frame PNG in `recordings/`.

### Desktop app window capture

**Driver API** (`desktop/src/driver.rs`) — QA/pen-test only (`ARBOS_DRIVER=1`):

- Method `screenshot` → macOS `screencapture -l<window_id>`; Linux X11 `import -window root` or `xwd|convert`.
- **Not** wired to in-app user feedback UI today.

**Permissions** (`desktop/src/permissions.rs`):

- macOS: ScreenCaptureKit / `CGPreflightScreenCaptureAccess` for permission row.
- Linux: portal on first capture (Wayland); X11 granted if DISPLAY set.

### Desktop vs kernel for feedback UI

For “screenshot of the app window” in desktop feedback:

- **Reuse driver-style capture** (window-scoped on macOS) or platform screen APIs from desktop process — **not** kernel `screenshot` tool (captures agent's display context, not necessarily Arbos window).
- Kernel `Screen` frame captures **whole screen** for Try Live, not app chrome specifically.

---

## Gaps vs stated feedback goals (Jacob's ask)

| Need | Exists? |
|------|---------|
| Kernel log attachment | Path + Tail/Read frames; no packaged “recent log” helper |
| Trajectory with tool calls | Full JSONL on disk; history/replayed frames; render tool omits args/body |
| Version + commit in report | Hello, kernel.json, build.rs stamps — easy to assemble |
| Hub delivery endpoint | Hub has register/attach/claim only — **no feedback POST** |
| User review + strip credentials | Secret redaction on transcript; no feedback-specific UI/API |
| iOS feedback loop | TestFlight poller mentioned in notes; **no in-repo feedback submit API found** |
| Screenshot of app | Driver + kernel screen capture exist; **no product feedback integration** |

---

## Key file index

| Area | Paths |
|------|-------|
| Kernel log | `crates/arbos-kernel/src/klog.rs`, `serve.rs` (`write_kernel_json`) |
| Transcript schema | `crates/arbos-core/src/event.rs`, `files.rs` (`append_events`, `roll_transcript`) |
| Wire protocol | `crates/arbos-core/src/wire.rs`, `crates/arbos-kernel/src/serve.rs`, `attach.rs`, `files.rs` |
| Auth | `crates/arbos-kernel/src/access.rs` |
| Desktop client | `desktop/src/agent/acp.rs`, `desktop/src/kernel.rs` |
| iOS client | `ios/Arbos/Kernel/ArbosKernelClient.swift` |
| Versions | `crates/arbos-kernel/build.rs`, `desktop/build.rs`, `desktop/src/build.rs`, `crates/arbos-update/` |
| Redaction | `crates/arbos-engine/src/secrets.rs`, `crates/arbos-core/src/envsafe.rs` |
| Screenshots | `crates/arbos-kernel/src/screenshot.rs`, `desktop/src/driver.rs`, `desktop/src/permissions.rs` |
