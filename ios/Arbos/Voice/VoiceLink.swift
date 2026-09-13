import Foundation

/// The one connection to the speech server, shared by the call (audio)
/// and the chat sheet (text channel, agent mirror). Opens on demand,
/// fans every event out to whoever subscribed, and survives the call
/// ending: the chat can reopen it by sending text.
@MainActor
final class VoiceLink: ObservableObject {
    enum State: Equatable {
        case disconnected
        case connecting
        case connected(VoiceServerInfo)
        case failed(String)
    }

    @Published private(set) var state: State = .disconnected

    var info: VoiceServerInfo? {
        if case .connected(let info) = state { return info }
        return nil
    }

    private let settings: AppSettings
    private var session: VoiceSession?
    private var pump: Task<Void, Never>?
    private var subscribers: [UUID: (VoiceEvent) -> Void] = [:]

    init(settings: AppSettings) {
        self.settings = settings
    }

    @discardableResult
    func subscribe(_ handler: @escaping (VoiceEvent) -> Void) -> UUID {
        let id = UUID()
        subscribers[id] = handler
        return id
    }

    func unsubscribe(_ id: UUID?) {
        guard let id else { return }
        subscribers.removeValue(forKey: id)
    }

    /// Connect if not already, and return once the server said
    /// `session.ready`. Idempotent.
    func connect() async throws {
        if case .connected = state { return }
        if state != .connecting {
            state = .connecting
            let session = settings.provider.makeSession(settings)
            self.session = session
            pump = Task { [weak self] in
                for await event in session.events {
                    guard let self, !Task.isCancelled else { return }
                    self.dispatch(event)
                }
            }
            do {
                try await session.connect()
            } catch {
                fail(error.localizedDescription)
                throw error
            }
        }
        let deadline = Date().addingTimeInterval(20)
        while Date() < deadline {
            switch state {
            case .connected: return
            case .failed(let reason): throw VoiceLinkError.failed(reason)
            case .disconnected: throw VoiceLinkError.failed("closed")
            case .connecting: try await Task.sleep(for: .milliseconds(50))
            }
        }
        fail("server did not answer")
        throw VoiceLinkError.failed("server did not answer")
    }

    func disconnect() {
        pump?.cancel()
        pump = nil
        session?.close()
        session = nil
        state = .disconnected
    }

    /// A closure safe to call from the audio thread.
    func audioSink() -> (Data) -> Void {
        let session = self.session
        return { frame in session?.send(audio: frame) }
    }

    func speak(_ text: String) { session?.speak(text) }
    func setSpeaking(_ speaking: Bool) { session?.setSpeaking(speaking) }
    func sendText(_ text: String) { session?.sendText(text) }
    func cancelText() { session?.cancelText() }
    func interrupt() { session?.interrupt() }

    // MARK: - Private

    private func dispatch(_ event: VoiceEvent) {
        switch event {
        case .connected(let info):
            state = .connected(info)
        case .error(let message):
            if case .connecting = state { state = .failed(message) }
        case .closed:
            if case .failed = state {} else { state = .disconnected }
            session = nil
        default:
            break
        }
        for handler in subscribers.values {
            handler(event)
        }
    }

    private func fail(_ reason: String) {
        pump?.cancel()
        pump = nil
        session?.close()
        session = nil
        state = .failed(reason)
    }
}

enum VoiceLinkError: LocalizedError {
    case failed(String)

    var errorDescription: String? {
        switch self {
        case .failed(let reason): return reason
        }
    }
}
