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

    /// Tell the server whether the phone is playing a reply, and out of
    /// what: the echo gate tightens for `speaker` and stays off for
    /// `airpods`, `headset`, `headphones`, `bluetooth`, `wired`, `earpiece`,
    /// where no echo path exists (gateway PR #56).
    func setSpeaking(_ speaking: Bool, route: String)

    /// The user started talking over the reply. Drop the rest of it.
    func interrupt()

    func close()
}

/// What the server said about itself in `session.ready`.
struct VoiceServerInfo: Equatable {
    /// `duplex` (one speech-to-speech model that answers on its own),
    /// `openai` (GPT-Live speaks; the kernel answers project questions
    /// through client delegation) or `pipeline` (ASR → optional reply hop → TTS).
    var engine: String = ""
    /// Who answers spoken turns: `none` means the app must.
    var reply: String = ""
    var tools: [String] = []
    /// The server is attached to an Arbos kernel and mirrors its chat.
    var kernel = false
    var text = ""
    /// The project the call runs against (`session.ready.project`, gateway
    /// PR #56): the roster's name and icon, for the line under the orb.
    var project: CallProject?

    /// True when the server produces the spoken reply itself, so the app
    /// must not forward transcripts to the kernel (that would answer twice).
    var answersItself: Bool {
        engine == "duplex" || engine == "openai" || (!reply.isEmpty && reply != "none")
    }
}

/// Why a reply ended, as the server says rather than as the client guesses.
///
/// The phone used to read the bare presence of `response.done` as "the answer
/// is over", which sent the call back to listening for a reply that had never
/// made a sound. The server says why now, so the client reads it.
enum ResponseEnd: String {
    /// It finished saying what it had to say.
    case completed
    /// The caller spoke over it. It had been playing.
    case interrupted
    /// More speech arrived and the question was re-asked; this reply was
    /// abandoned before it reached anyone. Nothing happened.
    case superseded
    case failed

    /// Older servers send `interrupted: true|false` and no reason.
    init(reason: String?, interrupted: Bool) {
        self = reason.flatMap(ResponseEnd.init(rawValue:)) ?? (interrupted ? .interrupted : .completed)
    }

    /// Whether a caller could have heard any of it. A reply nobody heard is
    /// not an answer ending and must not be drawn as one.
    var reachedTheCaller: Bool {
        switch self {
        case .completed, .interrupted: return true
        case .superseded, .failed: return false
        }
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
    case responseDone(ResponseEnd)
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
    /// The gateway's word on work: `working` (a turn is running), `tool`
    /// (inside a tool call: `tool` and `detail` say which), or `idle`. The
    /// working sound follows this and only this; a timer would lie.
    case agentActivity(agent: String, state: String, tool: String?, detail: String?)
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

    /// What the other end said as it closed, when it said anything. A server
    /// that refuses a connection puts its reason here; without it a client
    /// has only the transport's own words, which describe the socket and not
    /// the refusal, and it ends up inventing an explanation for the user.
    var closeReason: String? {
        guard let data = socket.closeReason, !data.isEmpty,
              let text = String(data: data, encoding: .utf8)?
                  .trimmingCharacters(in: .whitespacesAndNewlines),
              !text.isEmpty
        else { return nil }
        return text
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

/// The kernel a scoped call was dialled into, as the gateway names it.
struct CallProject: Equatable {
    var machine: String
    var project: String
    var name: String
    var icon: String
}
