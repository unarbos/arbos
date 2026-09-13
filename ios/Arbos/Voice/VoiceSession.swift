import Foundation

/// One full-duplex voice conversation with a speech server.
///
/// Audio in both directions is 16-bit little-endian PCM, mono, 24 kHz.
/// The session owns the network connection. The caller owns the microphone
/// and the speaker (`AudioEngine`) and drives the UI from `events`.
///
/// Two shapes of provider fit behind this:
///   - speech-only (the self-hosted server): it hears, transcribes, and
///     voices text we hand it with `speak`. The reply itself comes from the
///     Arbos kernel.
///   - speech-to-speech (OpenAI Realtime): it replies on its own; `speak`
///     is a no-op there.
protocol VoiceSession: AnyObject {
    /// Every event the server sends, in order. Finishes after `close()`.
    var events: AsyncStream<VoiceEvent> { get }

    /// Open the connection. Returns once the socket is up; `.connected`
    /// arrives on `events` when the server is ready for audio.
    func connect() async throws

    /// One frame of microphone audio. Safe to call from the audio thread.
    func send(audio frame: Data)

    /// Voice this text. Audio comes back as `.assistantAudio` chunks and the
    /// turn closes with `.responseDone`.
    func speak(_ text: String)

    /// A typed turn. The answer streams back as `.textDelta` then `.textDone`.
    func sendText(_ text: String)
    func cancelText()

    /// Tell the server whether the phone's speaker is playing a reply, so
    /// its echo gate can tighten while it is.
    func setSpeaking(_ speaking: Bool)

    /// The user started talking over the reply. Drop the rest of it.
    func interrupt()

    func close()
}

/// What the server said about itself in `session.ready`.
struct VoiceServerInfo: Equatable {
    /// `duplex` (one speech-to-speech model that answers on its own) or
    /// `pipeline` (ASR → optional reply hop → TTS).
    var engine: String = ""
    /// Who answers spoken turns: `none` means the app must.
    var reply: String = ""
    var tools: [String] = []
    /// The server is attached to an Arbos kernel and mirrors its chat.
    var kernel = false
    var text = ""

    /// True when the server produces the spoken reply itself, so the app
    /// must not forward transcripts to the kernel (that would answer twice).
    var answersItself: Bool {
        engine == "duplex" || (!reply.isEmpty && reply != "none")
    }
}

enum VoiceEvent {
    case connected(VoiceServerInfo)
    /// Server voice-activity detection heard the user start speaking.
    case userSpeechStarted
    case userSpeechEnded
    /// Text of what the user said. A non-final `text` is an increment to
    /// append; a final one replaces the line (and may be empty).
    case userTranscript(text: String, final: Bool)
    /// The provider itself is producing a reply; no audio yet.
    case thinking
    /// One chunk of speech to play.
    case assistantAudio(Data)
    case assistantTranscript(delta: String)
    case responseDone(interrupted: Bool)
    /// Text channel.
    case textDelta(String)
    case textDone(text: String, cancelled: Bool)
    /// Agent bridge: the voice model acted.
    case toolCall(name: String, summary: String)
    case toolResult(name: String, output: String)
    /// A dispatched sub-agent finished; the server speaks `text` itself.
    case agentDone(agent: String, text: String)
    /// Mirror of the kernel's main chat.
    case agentEvent(agent: String, kind: String, text: String, from: String?)
    case agentTurn(agent: String, running: Bool)
    case agentTree([KernelAgent])
    case error(String)
    case closed
}

enum VoiceSessionError: LocalizedError {
    case missingAPIKey
    case missingServer
    case missingToken
    case badURL(String)

    var errorDescription: String? {
        switch self {
        case .missingAPIKey: return "No API key set."
        case .missingServer: return "No voice server set."
        case .missingToken: return "No server token set."
        case .badURL(let url): return "Bad URL: \(url)"
        }
    }
}

/// Shared plumbing: an `AsyncStream` plus the continuation that feeds it.
final class VoiceEventSink {
    let stream: AsyncStream<VoiceEvent>
    private let continuation: AsyncStream<VoiceEvent>.Continuation

    init() {
        var held: AsyncStream<VoiceEvent>.Continuation!
        stream = AsyncStream { held = $0 }
        continuation = held
    }

    func emit(_ event: VoiceEvent) {
        continuation.yield(event)
    }

    func finish() {
        continuation.finish()
    }
}

/// A WebSocket whose outbound messages keep their order. Concurrent
/// `send` calls on a bare `URLSessionWebSocketTask` do not.
final class OrderedWebSocket {
    private let socket: URLSessionWebSocketTask
    private let outbox: AsyncStream<URLSessionWebSocketTask.Message>
    private let outboxContinuation: AsyncStream<URLSessionWebSocketTask.Message>.Continuation
    private var sendTask: Task<Void, Never>?
    private var receiveTask: Task<Void, Never>?

    init(request: URLRequest, session: URLSession = URLSession(configuration: .default)) {
        socket = session.webSocketTask(with: request)
        var held: AsyncStream<URLSessionWebSocketTask.Message>.Continuation!
        outbox = AsyncStream { held = $0 }
        outboxContinuation = held
    }

    /// `onMessage` runs for each inbound frame; `onFailure` once, when the
    /// socket dies for any reason other than `close()`.
    func open(
        onMessage: @escaping (URLSessionWebSocketTask.Message) -> Void,
        onFailure: @escaping (Error) -> Void
    ) {
        socket.resume()
        sendTask = Task { [outbox, socket] in
            for await message in outbox {
                if Task.isCancelled { return }
                try? await socket.send(message)
            }
        }
        receiveTask = Task { [socket] in
            while !Task.isCancelled {
                do {
                    onMessage(try await socket.receive())
                } catch {
                    if !Task.isCancelled { onFailure(error) }
                    return
                }
            }
        }
    }

    func send(json object: [String: Any]) {
        guard let data = try? JSONSerialization.data(withJSONObject: object),
              let text = String(data: data, encoding: .utf8) else { return }
        outboxContinuation.yield(.string(text))
    }

    func send(binary data: Data) {
        outboxContinuation.yield(.data(data))
    }

    func close() {
        receiveTask?.cancel()
        sendTask?.cancel()
        outboxContinuation.finish()
        socket.cancel(with: .normalClosure, reason: nil)
    }
}

/// Parse one inbound text frame as a JSON object with a `type` tag.
func decodeTypedJSON(_ message: URLSessionWebSocketTask.Message) -> (type: String, object: [String: Any])? {
    let data: Data
    switch message {
    case .string(let text): data = Data(text.utf8)
    case .data(let raw): data = raw
    @unknown default: return nil
    }
    guard let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
          let type = object["type"] as? String else { return nil }
    return (type, object)
}
