import AVFoundation
import Combine
import Foundation

struct TranscriptLine: Identifiable, Equatable {
    enum Speaker: Equatable {
        case user
        case arbos
    }

    let id = UUID()
    let speaker: Speaker
    var text: String
}

/// The call, end to end: mic → speech server → transcript → kernel →
/// reply text → speech server → speaker. One state machine drives the
/// screen.
@MainActor
final class CallViewModel: ObservableObject {
    enum Phase: Equatable {
        case idle
        case unconfigured(String)
        case connecting
        case listening
        case thinking
        case speaking
        case failed(String)

        var label: String {
            switch self {
            case .idle: return "arbos"
            case .unconfigured(let why): return why
            case .connecting: return "connecting"
            case .listening: return "listening"
            case .thinking: return "thinking"
            case .speaking: return "speaking"
            case .failed: return "call failed"
            }
        }

        var inCall: Bool {
            switch self {
            case .connecting, .listening, .thinking, .speaking: return true
            case .idle, .unconfigured, .failed: return false
            }
        }
    }

    @Published private(set) var phase: Phase = .idle
    @Published private(set) var lines: [TranscriptLine] = []
    @Published private(set) var startedAt: Date?
    /// Short status under the label: "kernel offline" and the like.
    @Published private(set) var note: String?

    private let settings: AppSettings
    /// The main chat: where transcripts go and replies come from. Shared
    /// with the chat sheet, so typed and spoken turns land in one place.
    private let chat: ChatStore
    private let audio = AudioEngine()
    private var session: VoiceSession?
    private var eventTask: Task<Void, Never>?
    private var busyWatch: AnyCancellable?
    /// The speech side finished sending the reply; playback may still be
    /// draining.
    private var responseDone = true
    /// The kernel is mid-turn on our behalf.
    private var kernelBusy = false
    private var openUtterance = false

    init(settings: AppSettings, chat: ChatStore) {
        self.settings = settings
        self.chat = chat
        refreshIdle()
        #if DEBUG
        applyPreviewPhase()
        #endif
    }

    /// Idle shows why a call cannot start until the provider is set up.
    func refreshIdle() {
        guard !phase.inCall else { return }
        if case .failed = phase { return }
        phase = idlePhase
    }

    func startCall() {
        guard !phase.inCall else { return }
        guard settings.isConfigured else {
            phase = idlePhase
            return
        }
        phase = .connecting
        note = nil
        lines.removeAll()
        responseDone = true
        kernelBusy = false
        openUtterance = false
        Task { await connect() }
    }

    func endCall() {
        teardown()
        phase = idlePhase
    }

    // MARK: - Connect

    private var idlePhase: Phase {
        settings.isConfigured ? .idle : .unconfigured(settings.provider.unconfiguredLabel)
    }

    private func connect() async {
        guard await AVAudioApplication.requestRecordPermission() else {
            phase = .failed("Microphone access is off.")
            return
        }
        let session = settings.provider.makeSession(settings)
        self.session = session
        audio.onCapture = { [weak session] frame in session?.send(audio: frame) }
        audio.onPlaybackDrained = { [weak self] in
            Task { @MainActor in self?.playbackDrained() }
        }
        do {
            try audio.start()
            try await session.connect()
        } catch {
            fail(error.localizedDescription)
            return
        }
        startedAt = Date()
        eventTask = Task { [weak self] in
            for await event in session.events {
                guard let self, !Task.isCancelled else { return }
                self.handle(event)
            }
        }
        await joinChat()
    }

    /// Replies come from the main chat. A live kernel is best; the scripted
    /// stand-in still lets the loop run end to end.
    private func joinChat() async {
        await chat.connect()
        chat.onAgentMessage = { [weak self] text in self?.speak(text) }
        busyWatch = chat.$busy.sink { [weak self] running in
            guard let self else { return }
            self.kernelBusy = running
            if running {
                if self.phase == .listening { self.phase = .thinking }
            } else {
                self.settle()
            }
        }
        switch chat.mode {
        case .live: note = nil
        case .mock: note = "no kernel · demo chat answers"
        case .offline, .connecting: note = "kernel offline"
        }
    }

    private func speak(_ text: String) {
        guard phase.inCall else { return }
        lines.append(TranscriptLine(speaker: .arbos, text: text))
        trimLines()
        responseDone = false
        phase = .thinking
        session?.speak(text)
    }

    // MARK: - Speech events

    private func handle(_ event: VoiceEvent) {
        switch event {
        case .connected:
            phase = .listening
        case .userSpeechStarted:
            // Barge-in: whatever Arbos was saying stops now.
            if audio.isPlaying || phase == .speaking {
                audio.stopPlayback()
                session?.interrupt()
                responseDone = true
            }
            phase = .listening
        case .userSpeechEnded:
            phase = .thinking
        case .userTranscript(let text, let final):
            appendUserTranscript(text, final: final)
            if final { forwardToKernel(text) }
        case .thinking:
            responseDone = false
            if phase != .speaking { phase = .thinking }
        case .assistantAudio(let pcm):
            responseDone = false
            phase = .speaking
            audio.play(pcm16: pcm)
        case .assistantTranscript(let delta):
            append(delta, to: .arbos)
        case .responseDone:
            responseDone = true
            settle()
        case .error(let message):
            fail(message)
        case .closed:
            if phase.inCall { fail("Connection closed.") }
        }
    }

    private func playbackDrained() {
        settle()
    }

    /// Back to listening once nobody is working and nothing is playing.
    private func settle() {
        guard phase.inCall, responseDone, !kernelBusy, !audio.isPlaying else { return }
        phase = .listening
    }

    // MARK: - Kernel

    private func forwardToKernel(_ text: String) {
        guard !text.trimmingCharacters(in: .whitespaces).isEmpty, chat.mode != .offline else { return }
        kernelBusy = true
        phase = .thinking
        chat.send(text)
    }

    // MARK: - Transcript

    /// Transcription deltas come mid-utterance; `final` replaces the whole
    /// line with the server's cleaned-up text and closes it.
    private func appendUserTranscript(_ text: String, final: Bool) {
        if final {
            if let index = lines.indices.last, lines[index].speaker == .user, openUtterance {
                lines[index].text = text
            } else if !text.isEmpty {
                lines.append(TranscriptLine(speaker: .user, text: text))
            }
            openUtterance = false
        } else {
            if !openUtterance {
                lines.append(TranscriptLine(speaker: .user, text: ""))
                openUtterance = true
            }
            append(text, to: .user)
        }
        trimLines()
    }

    private func append(_ delta: String, to speaker: TranscriptLine.Speaker) {
        guard !delta.isEmpty else { return }
        if let index = lines.indices.last, lines[index].speaker == speaker,
           speaker == .arbos || openUtterance {
            lines[index].text += delta
        } else {
            lines.append(TranscriptLine(speaker: speaker, text: delta))
        }
        trimLines()
    }

    private func trimLines() {
        if lines.count > 12 { lines.removeFirst(lines.count - 12) }
    }

    // MARK: - Teardown

    private func fail(_ message: String) {
        teardown()
        phase = .failed(message)
    }

    private func teardown() {
        eventTask?.cancel()
        eventTask = nil
        busyWatch = nil
        chat.onAgentMessage = nil
        session?.close()
        session = nil
        audio.stop()
        startedAt = nil
        kernelBusy = false
    }

    #if DEBUG
    /// `xcrun simctl launch booted com.unarbos.arbos.ios -previewPhase listening`
    /// shows a screen state without a server, for design review.
    private func applyPreviewPhase() {
        guard let raw = UserDefaults.standard.string(forKey: "previewPhase") else { return }
        switch raw {
        case "listening":
            phase = .listening
            startedAt = Date().addingTimeInterval(-192)
            lines = [
                TranscriptLine(speaker: .user, text: "What's left on the kernel branch before I can merge it?"),
                TranscriptLine(speaker: .arbos, text: "Two things: the attach test and the changelog."),
                TranscriptLine(speaker: .user, text: "Okay, start on the attach test and"),
            ]
            openUtterance = true
        case "thinking":
            phase = .thinking
            startedAt = Date().addingTimeInterval(-40)
        case "speaking":
            phase = .speaking
            startedAt = Date().addingTimeInterval(-40)
        default:
            break
        }
    }
    #endif
}
