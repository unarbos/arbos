import Foundation

/// `VoiceSession` over the OpenAI Realtime API (GA WebSocket interface).
/// Speech-to-speech: the model replies on its own, so `speak` is a no-op
/// and the kernel is not in the loop. Kept behind
/// `FeatureFlags.openAIRealtime` as a fallback provider.
///
/// Wire shape:
///   client → `input_audio_buffer.append { audio: base64 pcm16 }`
///   server → `input_audio_buffer.speech_started` / `speech_stopped`
///            `conversation.item.input_audio_transcription.delta|completed`
///            `response.created`
///            `response.output_audio.delta { delta: base64 pcm16 }`
///            `response.output_audio_transcript.delta`
///            `response.done`, `error`
/// Server VAD runs turn taking. With `interrupt_response: true` the server
/// cancels its own reply when the user talks over it; the client only has
/// to stop local playback.
final class OpenAIRealtimeSession: VoiceSession {
    private let apiKey: String
    private let model: String
    private let instructions: String
    private let sink = VoiceEventSink()
    private var socket: OrderedWebSocket?
    private var closed = false

    var events: AsyncStream<VoiceEvent> { sink.stream }

    init(apiKey: String, model: String, instructions: String) {
        self.apiKey = apiKey
        self.model = model
        self.instructions = instructions
    }

    func connect() async throws {
        guard !apiKey.isEmpty else { throw VoiceSessionError.missingAPIKey }
        let raw = "wss://api.openai.com/v1/realtime?model=\(model)"
        guard let url = URL(string: raw) else { throw VoiceSessionError.badURL(raw) }
        var request = URLRequest(url: url)
        request.setValue("Bearer \(apiKey)", forHTTPHeaderField: "Authorization")
        let socket = OrderedWebSocket(request: request)
        self.socket = socket
        socket.open(
            onMessage: { [weak self] message in self?.handle(message) },
            onFailure: { [weak self] error in
                guard let self, !self.closed else { return }
                self.sink.emit(.error(error.localizedDescription))
                self.close()
            }
        )
    }

    func send(audio frame: Data) {
        guard !closed, !frame.isEmpty else { return }
        socket?.send(json: ["type": "input_audio_buffer.append", "audio": frame.base64EncodedString()])
    }

    func speak(_ text: String) {}

    /// A typed turn as a conversation item; the reply comes back as speech.
    func sendText(_ text: String) {
        guard !closed, !text.isEmpty else { return }
        socket?.send(json: [
            "type": "conversation.item.create",
            "item": [
                "type": "message",
                "role": "user",
                "content": [["type": "input_text", "text": text]],
            ],
        ])
        socket?.send(json: ["type": "response.create"])
    }

    func cancelText() {
        interrupt()
    }

    func setSpeaking(_ speaking: Bool, route: String) {}

    func interrupt() {
        // Server VAD already cancelled the reply on `speech_started`; this
        // covers a manual interrupt where it did not.
        socket?.send(json: ["type": "response.cancel"])
    }

    func close() {
        guard !closed else { return }
        closed = true
        socket?.close()
        socket = nil
        sink.emit(.closed)
        sink.finish()
    }

    // MARK: - Session setup

    private func configureSession() {
        // Input transcription is what gives the UI Jacob's words as he
        // speaks. `near_field` noise reduction suits earbuds on a run.
        let session: [String: Any] = [
            "type": "realtime",
            "instructions": instructions,
            "output_modalities": ["audio"],
            "audio": [
                "input": [
                    "format": ["type": "audio/pcm", "rate": 24_000],
                    "noise_reduction": ["type": "near_field"],
                    "transcription": ["model": "gpt-4o-mini-transcribe"],
                    "turn_detection": [
                        "type": "server_vad",
                        "threshold": 0.6,
                        "prefix_padding_ms": 300,
                        "silence_duration_ms": 700,
                        "create_response": true,
                        "interrupt_response": true,
                    ],
                ],
                "output": [
                    "format": ["type": "audio/pcm", "rate": 24_000],
                    "voice": "marin",
                ],
            ],
        ]
        socket?.send(json: ["type": "session.update", "session": session])
    }

    // MARK: - Inbound

    private func handle(_ message: URLSessionWebSocketTask.Message) {
        guard let (type, object) = decodeTypedJSON(message) else { return }
        switch type {
        case "session.created":
            configureSession()
            var info = VoiceServerInfo()
            info.engine = "openai-realtime"
            info.reply = model
            sink.emit(.connected(info))
        case "input_audio_buffer.speech_started":
            sink.emit(.userSpeechStarted)
        case "input_audio_buffer.speech_stopped":
            sink.emit(.userSpeechEnded)
        case "conversation.item.input_audio_transcription.delta":
            sink.emit(.userTranscript(text: object["delta"] as? String ?? "", final: false))
        case "conversation.item.input_audio_transcription.completed":
            sink.emit(.userTranscript(text: object["transcript"] as? String ?? "", final: true))
        case "response.created":
            sink.emit(.thinking)
        case "response.output_audio.delta", "response.audio.delta":
            if let b64 = object["delta"] as? String, let pcm = Data(base64Encoded: b64) {
                sink.emit(.assistantAudio(pcm))
            }
        case "response.output_audio_transcript.delta", "response.audio_transcript.delta":
            sink.emit(.assistantTranscript(delta: object["delta"] as? String ?? ""))
        case "response.done":
            sink.emit(.responseDone(interrupted: false))
        case "response.cancelled":
            sink.emit(.responseDone(interrupted: true))
        case "error":
            let detail = object["error"] as? [String: Any]
            sink.emit(.error(detail?["message"] as? String ?? "Realtime error"))
        default:
            break
        }
    }
}
