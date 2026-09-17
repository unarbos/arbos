import Foundation

/// `VoiceSession` over our own speech server (`voice-server/` in this repo).
///
/// One WebSocket. Audio is binary frames of raw PCM16 mono 24 kHz in both
/// directions. Control is JSON text frames tagged `type`. The full protocol
/// is `python -m voice_server --print-protocol`; the frames used here:
///
///   client → server
///     `session.start { format: { type: "audio/pcm", rate: 24000 }, project?: { machine, project } }`
///     <binary>                       microphone audio
///     `speak { text }`               voice this text (gateway TTS)
///     `client.speaking { speaking, route }` the phone is (not) playing a reply, and out of what
///     `interrupt`                    drop the current reply
///     `text.input { text }`          a typed turn
///     `text.cancel`
///     `session.end`
///
///   server → client
///     `session.ready { engine, reply, tools, kernel, text, project?, project_info? }`  — project: the kernel on the line
///     `error { code: project_unknown|project_offline|project_unreachable|no_hub, message }` then close 4404: the target cannot be reached, no fallback
///     `speech.started` / `speech.stopped`      server VAD
///     `transcript.delta { text }`   append;  `transcript.final { text }`  replace, may be ""
///     `response.started`            only when the server replies on its own
///     <binary>                      reply audio
///     `response.transcript { text }`
///     `response.done { interrupted? }`
///     `text.delta { text }` / `text.done { text, cancelled }`
///     `tool.call { name, arguments }` / `tool.result { name, output }`
///     `agent.done { agent, text }`
///     `agent.event { agent, kind, text }` / `agent.turn { agent, state }` / `agent.tree { agents }`
///     `agent.activity { agent, state: working|tool|idle, tool?, detail? }`  the kernel is at work; the working sound follows it
///     `error { message }`
///
/// With the `duplex` engine (NemotronLabs VoiceChat) or the `openai` engine
/// (GPT-Live, the kernel behind it by client delegation) the server answers
/// spoken turns itself and calls Arbos tools mid-conversation; the app only
/// plays audio, shows text, and flushes playback on `speech.started`.
final class SelfHostedVoiceSession: VoiceSession {
    private let serverURL: String
    private let token: String
    private let sink = VoiceEventSink()
    private var socket: OrderedWebSocket?
    private var closed = false

    var events: AsyncStream<VoiceEvent> { sink.stream }

    /// `mode`: `voice` (the call) or `dictation` (words only: the server
    /// transcribes and answers nothing).
    private let mode: String
    /// The project the call was opened from; the gateway dials its kernel.
    private let project: KernelTarget?

    init(serverURL: String, token: String, mode: String = "voice", project: KernelTarget? = nil) {
        self.serverURL = serverURL
        self.token = token
        self.mode = mode
        self.project = project
    }

    func connect() async throws {
        guard !serverURL.isEmpty else { throw VoiceSessionError.missingServer }
        guard !token.isEmpty else { throw VoiceSessionError.missingToken }
        guard var components = URLComponents(string: serverURL),
              let scheme = components.scheme, ["ws", "wss"].contains(scheme.lowercased()) else {
            throw VoiceSessionError.badURL(serverURL)
        }
        if components.path.isEmpty || components.path == "/" { components.path = "/ws" }
        var query = components.queryItems ?? []
        query.removeAll { $0.name == "token" }
        query.append(URLQueryItem(name: "token", value: token))
        components.queryItems = query
        guard let url = components.url else { throw VoiceSessionError.badURL(serverURL) }

        let socket = OrderedWebSocket(request: URLRequest(url: url))
        self.socket = socket
        socket.open(
            onMessage: { [weak self] message in self?.handle(message) },
            onFailure: { [weak self] error in
                guard let self, !self.closed else { return }
                self.sink.emit(.error(error.localizedDescription))
                self.close()
            }
        )
        var start: [String: Any] = [
            "type": "session.start",
            "format": ["type": "audio/pcm", "rate": Int(AudioEngine.sampleRate)],
            "agents": mode == "voice",
        ]
        if mode != "voice" { start["mode"] = mode }
        // Dictation is words for the field, nobody's turn: the gateway's
        // kernel routing (PR #56) must not run on them. Observed 2026-09-16:
        // a dictated line landed in the gateway's own project, unsent.
        if mode == "dictation" { start["answerer"] = "model" }
        // The call is scoped to the project it was opened from (JB-3): the
        // gateway attaches to that kernel for the call's life and refuses,
        // never falls back, when it cannot. `.pod` is the server's default.
        if mode == "voice", case .hub(let machine, let name)? = project {
            start["project"] = ["machine": machine, "project": name]
        }
        socket.send(json: start)
    }

    func send(audio frame: Data) {
        guard !closed, !frame.isEmpty else { return }
        socket?.send(binary: frame)
    }

    func speak(_ text: String) {
        guard !closed, !text.isEmpty else { return }
        socket?.send(json: ["type": "speak", "text": text])
    }

    func sendText(_ text: String) {
        guard !closed, !text.isEmpty else { return }
        socket?.send(json: ["type": "text.input", "text": text])
    }

    func cancelText() {
        socket?.send(json: ["type": "text.cancel"])
    }

    func setSpeaking(_ speaking: Bool, route: String) {
        guard !closed else { return }
        socket?.send(json: ["type": "client.speaking", "speaking": speaking, "route": route])
    }

    func interrupt() {
        socket?.send(json: ["type": "interrupt"])
    }

    func close() {
        guard !closed else { return }
        closed = true
        socket?.send(json: ["type": "session.end"])
        socket?.close()
        socket = nil
        sink.emit(.closed)
        sink.finish()
    }

    // MARK: - Inbound

    private func handle(_ message: URLSessionWebSocketTask.Message) {
        // Binary is always audio; control frames are text.
        if case .data(let pcm) = message {
            if !pcm.isEmpty { sink.emit(.assistantAudio(pcm)) }
            return
        }
        guard let (type, object) = decodeTypedJSON(message) else { return }
        let text = object["text"] as? String ?? ""
        switch type {
        case "session.ready":
            var info = VoiceServerInfo()
            info.engine = object["engine"] as? String ?? ""
            info.reply = object["reply"] as? String ?? ""
            info.tools = object["tools"] as? [String] ?? []
            info.kernel = object["kernel"] as? Bool ?? false
            info.text = object["text"] as? String ?? ""
            // The gateway names the project as `project: "machine/project"` with
            // the roster's name and icon beside it in `project_info`; older
            // builds put the dict in `project` itself. Read all three shapes.
            if let p = (object["project_info"] as? [String: Any]) ?? (object["project"] as? [String: Any]) {
                let machine = p["machine"] as? String ?? ""
                let name = p["project"] as? String ?? ""
                info.project = CallProject(machine: machine, project: name, name: p["name"] as? String ?? name, icon: p["icon"] as? String ?? "folder")
            } else if let label = object["project"] as? String, !label.isEmpty {
                let parts = label.split(separator: "/", maxSplits: 1).map(String.init)
                let machine = parts.count == 2 ? parts[0] : ""
                let name = parts.count == 2 ? parts[1] : label
                info.project = CallProject(machine: machine, project: name, name: name, icon: "folder")
            }
            sink.emit(.connected(info))
        case "speech.started":
            sink.emit(.userSpeechStarted)
        case "speech.stopped":
            sink.emit(.userSpeechEnded)
        case "transcript.delta":
            sink.emit(.userTranscript(text: text, final: false))
        case "transcript.final":
            sink.emit(.userTranscript(text: text, final: true))
        case "response.started":
            sink.emit(.thinking)
        case "response.transcript":
            sink.emit(.assistantTranscript(delta: text))
        case "response.done":
            sink.emit(.responseDone(ResponseEnd(reason: object["reason"] as? String,
                                                interrupted: object["interrupted"] as? Bool ?? false)))
        case "text.delta":
            sink.emit(.textDelta(text))
        case "text.done":
            sink.emit(.textDone(text: text, cancelled: object["cancelled"] as? Bool ?? false))
        case "tool.call":
            let name = object["name"] as? String ?? "tool"
            let arguments = object["arguments"] as? [String: Any] ?? [:]
            let summary = (arguments["task"] ?? arguments["question"] ?? arguments.values.first)
                .map { "\($0)" } ?? ""
            sink.emit(.toolCall(name: name, summary: summary))
        case "tool.result":
            sink.emit(.toolResult(
                name: object["name"] as? String ?? "tool",
                output: object["output"] as? String ?? ""
            ))
        case "agent.done":
            sink.emit(.agentDone(agent: object["agent"] as? String ?? "", text: text))
        case "agent.event":
            sink.emit(.agentEvent(
                agent: object["agent"] as? String ?? "",
                kind: object["kind"] as? String ?? "",
                text: text,
                from: object["from"] as? String
            ))
        case "agent.turn":
            sink.emit(.agentTurn(
                agent: object["agent"] as? String ?? "",
                running: (object["state"] as? String) == "running"
            ))
        case "agent.activity":
            sink.emit(.agentActivity(
                agent: object["agent"] as? String ?? "root",
                state: object["state"] as? String ?? "idle",
                tool: object["tool"] as? String,
                detail: object["detail"] as? String
            ))
        case "agent.tree":
            let rows = object["agents"] as? [[String: Any]] ?? []
            sink.emit(.agentTree(rows.compactMap { row in
                guard let id = row["id"] as? String else { return nil }
                return KernelAgent(
                    id: id,
                    name: row["name"] as? String ?? id,
                    parent: row["parent"] as? String,
                    paused: false,
                    model: "",
                    step: row["step"] as? String
                )
            }))
        case "error":
            let message = object["message"] as? String ?? "Voice server error"
            let code = object["code"] as? String ?? ""
            if ["project_unknown", "project_offline", "project_unreachable", "no_hub"].contains(code) {
                let who = (object["project"] as? String).map { $0.split(separator: "/").last.map(String.init) ?? $0 } ?? "this project"
                let why: String
                switch code {
                case "project_unknown": why = "the hub knows no project by that name"
                case "project_offline": why = "its kernel is not running"
                case "project_unreachable": why = "its kernel did not answer"
                default: why = "the voice server has no hub"
                }
                sink.emit(.error("Can't call \(who) — \(why). \(message)"))
            } else {
                sink.emit(.error(message))
            }
        default:
            break
        }
    }
}
