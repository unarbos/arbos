import Foundation

/// `VoiceSession` over our own speech server: open-source ASR and TTS on a
/// box we run (the kernel host, or a rented GPU). The server is speech
/// only. The reply comes from the Arbos kernel, which the phone talks to
/// through `ArbosKernelClient`; the phone hands the reply text back here
/// with `speak`.
///
/// Wire protocol, one WebSocket. Audio is binary frames of raw PCM16 mono
/// 24 kHz in both directions. Control is JSON text frames tagged `type`.
///
///   client → server
///     `session.start { format: { type: "audio/pcm", rate: 24000 } }`
///     <binary>                       microphone audio
///     `speak { text }`               voice this reply
///     `interrupt`                    drop the current reply
///     `session.end`
///
///   server → client
///     `session.ready`
///     `speech.started` / `speech.stopped`      server VAD
///     `transcript.delta { text }` / `transcript.final { text }`
///     `response.started`             only if the server replies on its own
///     <binary>                       reply audio
///     `response.transcript { text }` what the audio says
///     `response.done`
///     `error { message }`
///
/// Any server that speaks this (Whisper or Moonshine in, Kokoro or Piper
/// out, a few hundred lines of Python) works unchanged.
final class SelfHostedVoiceSession: VoiceSession {
    private let serverURL: String
    private let sink = VoiceEventSink()
    private var socket: OrderedWebSocket?
    private var closed = false

    var events: AsyncStream<VoiceEvent> { sink.stream }

    init(serverURL: String) {
        self.serverURL = serverURL
    }

    func connect() async throws {
        guard !serverURL.isEmpty else { throw VoiceSessionError.missingServer }
        guard let url = URL(string: serverURL), let scheme = url.scheme,
              ["ws", "wss"].contains(scheme.lowercased()) else {
            throw VoiceSessionError.badURL(serverURL)
        }
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
        socket.send(json: [
            "type": "session.start",
            "format": ["type": "audio/pcm", "rate": Int(AudioEngine.sampleRate)],
        ])
    }

    func send(audio frame: Data) {
        guard !closed, !frame.isEmpty else { return }
        socket?.send(binary: frame)
    }

    func speak(_ text: String) {
        guard !closed, !text.isEmpty else { return }
        socket?.send(json: ["type": "speak", "text": text])
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
        switch type {
        case "session.ready":
            sink.emit(.connected)
        case "speech.started":
            sink.emit(.userSpeechStarted)
        case "speech.stopped":
            sink.emit(.userSpeechEnded)
        case "transcript.delta":
            sink.emit(.userTranscript(text: object["text"] as? String ?? "", final: false))
        case "transcript.final":
            sink.emit(.userTranscript(text: object["text"] as? String ?? "", final: true))
        case "response.started":
            sink.emit(.thinking)
        case "response.transcript":
            sink.emit(.assistantTranscript(delta: object["text"] as? String ?? ""))
        case "response.done":
            sink.emit(.responseDone)
        case "error":
            sink.emit(.error(object["message"] as? String ?? "Voice server error"))
        default:
            break
        }
    }
}
