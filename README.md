# Arbos

Arbos is an open-source agent coordinator whose state lives on the file system. A project is a folder; its `.arbos/` directory holds the agents, their inboxes, transcripts, notes, and subscriptions, so anything that can read files can see what the agents are doing and wake them. On top of the kernel sit a Cursor-style desktop, a full-duplex voice call into any project, an iPhone app, and a mesh that lets every Arbos you run reach every other.

<p align="center">
  <img src="docs/img/project-page.png" alt="Arbos desktop: a project tab, its notes page, files, and context, with the workers in the right panel" width="900">
</p>

<p align="center">
  <img src="docs/img/call-mode.png" alt="Arbos desktop during a call: spoken words as user cards, the agent's highlights read aloud, a worker finishing in the panel" width="900">
</p>

## Install

The Mac app is the quick path. The rest is optional: build from source, put Arbos on your iPhone, run your own voice server, or lend a machine to the mesh. The same steps, with more detail, are at [arbos.life/install](https://arbos.life/install/).

### macOS (desktop)

1. Download `Arbos-<version>-<arch>.dmg` for your Mac (`arm64` for Apple silicon, `x86_64` for Intel) from [Releases](https://github.com/unarbos/arbos/releases). macOS 11 or newer.
2. Open the DMG and drag **Arbos** into Applications.
3. Open Arbos. The build is not notarized yet, so macOS may say it cannot verify the developer: right-click the app and choose **Open**, or go to System Settings › Privacy & Security and click **Open Anyway**. Once.
4. **Microphone**: the first time you dictate or start a call, macOS asks; click Allow. Settings › Permissions shows the state and has a **Test the microphone** row.
5. **Model key**: Settings › Model, paste an OpenRouter key, pick a model (see [Configure](#configure)).
6. Press ⌘T and choose a folder. That folder is now a project with its own agent. Type, or hold **fn** to speak.

The desktop starts one `arbos-kernel` per open project. It looks for the binary at `~/.cargo/bin/arbos-kernel`, then on `PATH`, or at `ARBOS_KERNEL_BIN=/path/to/arbos-kernel`. Until a Mac kernel build is attached to the release, install it from source (next section).

### From source (macOS, Linux)

Rust **nightly**; `rust-toolchain.toml` selects it, so `rustup` picks it up on first build. On macOS, the Xcode command line tools.

```bash
git clone https://github.com/unarbos/arbos.git && cd arbos
cargo install --path crates/arbos-kernel     # the kernel: runs the agents; lands in ~/.cargo/bin
cd desktop && cargo install --path .         # the desktop app; finds arbos-kernel in ~/.cargo/bin
arbos-desktop
```

`cargo build --release -p arbos-kernel` and `cd desktop && cargo build --release` build without installing. On macOS, `cd desktop && make bundle` produces `Arbos.app` and `make dmg` the disk image (ad-hoc signed unless `.env.release` names a certificate).

Linux packages for the desktop (Debian/Ubuntu names; `build-essential` is what a stock machine lacks — `g++` owns the `libstdc++.so` the link step needs). The why of each, and the `-lstdc++` failure a fresh Ubuntu 24.04 hits, are in [`desktop/BUILDING.md`](desktop/BUILDING.md):

```bash
sudo apt install build-essential pkg-config clang cmake libssl-dev libasound2-dev libxkbcommon-dev \
  libxkbcommon-x11-dev libwayland-dev libx11-dev libx11-xcb-dev libxcb1-dev \
  libfontconfig1-dev libfreetype6-dev libvulkan-dev libgl1-mesa-dev libegl1-mesa-dev \
  libudev-dev libdbus-1-dev
```

### iPhone

One long full-duplex call with your project, plus the main chat one swipe up. Not on the App Store yet: build it with Xcode and install it on your own phone.

1. Xcode 16 or newer, an iPhone on iOS 17 or newer, and a free Apple developer team for signing.
2. Open `ios/Arbos.xcodeproj`. Under **Signing & Capabilities**, pick your team (bundle id `com.unarbos.arbos.ios`).
3. Endpoints and tokens live in `ios/Arbos/Secrets.plist` (gitignored): `voiceServerURL`, `voiceToken`, `kernelURL`, `kernelToken`, `hubURL`, `hubToken`. `ios/scripts/gen-secrets.sh` writes it from a 1Password vault; write it by hand if you keep them elsewhere. The app's Settings override it.
4. Select your phone and press Run, or from a shell:

```bash
cd ios && ./scripts/gen-secrets.sh
xcodebuild -scheme Arbos -destination 'platform=iOS,id=<device-id>' -allowProvisioningUpdates build
```

On first launch iOS asks for the microphone; allow it. The kernel picker lists the machines on your hub; pick one and the call starts.

### Voice server

Speech runs on a server you control (`voice-server/`, Python 3.11+, `uv`). One WebSocket per call: raw audio both ways, a text channel, the tools that let the voice model dispatch agents, and the call-mode narrator. Two engines behind one protocol:

- **duplex**: one full-duplex speech-to-speech model (NVIDIA NemotronLabs VoiceChat). A GPU with about 24 GB. No turn-taking, natural interruption.
- **pipeline**: Silero VAD → faster-whisper → the model → Kokoro TTS. Any GPU, or a CPU.

```bash
cd voice-server
VOICE_HOME=$PWD VOICE_TOKEN=<a long random string> deploy/run.sh            # --engine duplex|pipeline (default: auto)
```

`run.sh` installs `uv`, makes a virtual environment, fetches the models, and starts the server on port 8765. Nothing is written outside `VOICE_HOME`. `deploy/stack.sh up` runs the server, a kernel, and a Cloudflare tunnel together (`deploy/cloudflare-tunnel.sh` alone fronts the server). Then tell the desktop where it is, in the same file as the model key, and put the same values into the phone's `Secrets.plist`:

```toml
# ~/.config/arbos/config.toml
voice_url = "wss://voice.example.com/ws"
voice_token_env = "VOICE_TOKEN"        # or voice_token = "..."
```

### Mesh (many machines)

Any machine running the Arbos worker can be used by any of your other Arbos instances: a cloud agent can build iOS on your Mac, your laptop can run a long job on a server. The worker connects **outbound** to a hub, so no ports need opening.

On the machine you want to offer, with `arbos-kernel` built and a model key in `~/arbos-hub/config/arbos/config.toml`:

```bash
ARBOS_HUB=wss://hub.example ARBOS_HUB_TOKEN=<token> \
  ~/arbos-hub/bin/arbos-kernel worker --dir ~/arbos-hub/projects --machine <name>
```

Add `--cap xcode --cap ios` on a Mac with Xcode so agents can find it. Clone the repositories the mesh may work on into `~/arbos-hub/projects/<name>`. `deploy/hub/worker.sh` wraps the same command in a restart loop; `deploy/hub/hub-run.sh` runs your own hub (`arbos-hub`, config in `deploy/hub/hub-server.example.toml`). Kernels register with `arbos-kernel serve <project> --hub <url> --machine <name>`, or from `~/.config/arbos/hub.toml` (`deploy/hub/hub.example.toml`). Agents then use `spawn host=<machine>` and `say to=<machine>/<project>/<agent>`; `arbos-kernel attach --hub <machine>/<project>` follows a kernel elsewhere by name.

## Configure

Arbos uses [OpenRouter](https://openrouter.ai) by default; OpenAI and any OpenAI-compatible endpoint sit behind the same setting.

```bash
arbos-kernel setup                       # asks for provider, key, model; makes one test call
arbos-kernel setup --provider openrouter --model openai/gpt-4.1-mini --key-stdin < key.txt
```

The result is `~/.config/arbos/config.toml`. The desktop offers the same flow in **Settings › Model** on first launch. The key can also stay in an environment variable (`api_key_env = "OPENROUTER_API_KEY"`) instead of the file. Other secrets agents may need go in `.arbos/secrets.toml` as `op://`, `env:`, or `file:` references and reach bash without entering the transcript.

Headless use, for scripts and CI:

```bash
arbos-kernel run "summarise the failing tests"   # exit 0 ok, 2 failed turn, 3 waiting on you, 4 timeout
arbos-kernel run --json "…" | jq                  # one JSON event per line
arbos-kernel answer --approve                     # answer a parked question or approval
```

## Concepts

- **Project**: a folder with a `.arbos/` directory. One tab in the desktop, one main chat.
- **Root coordinator**: the project's main agent, `root`. It does not edit code; it spawns workers, steers them, and reports. Its system prompt carries the project's context document first.
- **Workers**: sub-agents in `.arbos/agents/<id>/`, each with its own transcript. They can run in a git worktree, on another machine (`spawn host=`), or as an outside ACP agent. The kernel writes a `done` message to the parent when a worker's turn ends.
- **notes.md**: the project's status page, kept by root after every change: a `<tldr>`, topical sections, checkbox items with a link and a one-line readout. The desktop renders it as the Project page.
- **Subscriptions**: files under `agents/<id>/subscriptions/` are the only scheduler: timers, GitHub PR and CI events, webhooks, shell jobs, inbox watches. A subscription that fires writes an inbox file.
- **Inbox files**: every message to an agent is a file in its `inbox/` (user prompt, steer, worker report, answer). A turn starts when a file lands. Typed and spoken input interleave there by time.
- **Rewind**: `.arbos/` is a nested git repository with a commit per turn. `arbos-kernel rewind --back N` cuts the transcript and restores the tree from that checkpoint; the desktop offers "Rewind here".
- **Call mode**: call a project from the desktop or the phone. A narrator in the voice gateway reads short highlights of the main agent's work, answers "why exactly?" from the record, and never reads code or diffs aloud. Speaking during a turn steers it, like typing.

Design documents: [file-system state](docs/design/filesystem-state-design.md), [agent model (Cursor vs Arbos)](docs/design/cursor-vs-arbos-agent-model.md), [mesh](docs/design/arbos-mesh-design.md), [desktop call mode](docs/design/desktop-call-mode-design.md). The kernel writes the full on-disk protocol into every project as `.arbos/PROTOCOL.md`; `arbos-kernel prompt` prints what an agent is told.

## Repository

| Path | What |
| --- | --- |
| `crates/arbos-core` | Shared types: places, frames, inbox files, host config |
| `crates/arbos-engine` | The turn loop, providers, tools |
| `crates/arbos-kernel` | `arbos-kernel`: serve, run, setup, worker, rewind, check |
| `crates/arbos-hub` | `arbos-hub`: registry and router for the mesh |
| `desktop/` | `arbos-desktop`, built on gpui |
| `ios/` | The iPhone app |
| `voice-server/` | Speech gateway and the call-mode narrator |
| `harness/` | Arbos as a Prime Intellect verifiers harness (SWE-bench) |
| `deploy/` | Hub and worker scripts |

## License

[MIT](LICENSE)
