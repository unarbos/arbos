import AVFoundation
import Combine
import Foundation

struct TranscriptLine: Identifiable, Equatable {
    enum Speaker: Equatable {
        case user
        case arbos
        /// A tool call or a sub-agent report: one thin line.
        case system
    }

    let id = UUID()
    let speaker: Speaker
    var text: String
}

/// The call, end to end: mic → speech server → (reply) → speaker. One
/// state machine drives the screen.
///
/// Two server shapes. `duplex`: one speech model listens, answers, and
/// calls Arbos tools by itself; the app plays audio and shows text.
/// `pipeline` with no reply hop: the app sends each final transcript to
/// the main chat (kernel) and hands the reply back with `speak`.
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
    /// Short status under the label: engine, latency, "kernel offline".
    @Published private(set) var note: String?

    private let settings: AppSettings
    private let chat: ChatStore
    private let link: VoiceLink
    private let audio = AudioEngine()
    private var subscription: UUID?
    private var busyWatch: AnyCancellable?
    private var server = VoiceServerInfo()
    /// The speech side finished sending the reply; playback may still be
    /// draining.
    private var responseDone = true
    /// The kernel is mid-turn on our behalf (pipeline shape only).
    private var kernelBusy = false
    private var openUtterance = false
    private var speechEndedAt: Date?
    private var replyLatency: TimeInterval?

    init(settings: AppSettings, chat: ChatStore, link: VoiceLink) {
        self.settings = settings
        self.chat = chat
        self.link = link
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
        replyLatency = nil
        Task { await connect() }
    }

    func endCall() {
        teardown()
        phase = idlePhase
    }

    // MARK: - Connect

    private var idlePhase: Phase {
        settings.isConfigured ? .idle : .unconfigured(settings.provider.unconfiguredLabel(settings))
    }

    private func connect() async {
        guard await AVAudioApplication.requestRecordPermission() else {
            phase = .failed("Microphone access is off.")
            return
        }
        subscription = link.subscribe { [weak self] event in self?.handle(event) }
        audio.onPlaybackDrained = { [weak self] in
            Task { @MainActor in self?.playbackDrained() }
        }
        do {
            var captureMic = true
            #if DEBUG
            captureMic = UserDefaults.standard.string(forKey: "injectWav") == nil
            #endif
            try audio.start(captureMic: captureMic)
            try await link.connect()
        } catch {
            fail(error.localizedDescription)
            return
        }
        guard phase.inCall else { return }
        if let info = link.info { server = info }
        audio.onCapture = link.audioSink()
        startedAt = Date()
        phase = .listening
        await joinChat()
        updateNote()
        #if DEBUG
        injectWavIfAsked()
        #endif
    }

    /// The main chat mirrors the kernel. Only in the pipeline shape does
    /// the app route transcripts through it and speak what comes back.
    private func joinChat() async {
        await chat.connect()
        guard !server.answersItself else { return }
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
    }

    private func updateNote() {
        var parts: [String] = []
        if !server.engine.isEmpty { parts.append(server.engine) }
        if server.answersItself {
            if server.kernel { parts.append("kernel tools") }
        } else {
            switch chat.mode {
            case .server, .live: break
            case .mock: parts.append("demo chat answers")
            case .offline, .connecting: parts.append("kernel offline")
            }
        }
        if let replyLatency { parts.append(String(format: "reply %.1fs", replyLatency)) }
        note = parts.isEmpty ? nil : parts.joined(separator: " · ")
    }

    private func speak(_ text: String) {
        guard phase.inCall else { return }
        lines.append(TranscriptLine(speaker: .arbos, text: text))
        trimLines()
        responseDone = false
        phase = .thinking
        link.speak(text)
    }

    // MARK: - Speech events

    private func handle(_ event: VoiceEvent) {
        guard phase.inCall else { return }
        switch event {
        case .connected(let info):
            server = info
            if phase == .connecting { phase = .listening }
        case .userSpeechStarted:
            // Barge-in: whatever Arbos was saying stops now.
            if audio.isPlaying || phase == .speaking {
                audio.stopPlayback()
                link.interrupt()
                responseDone = true
            }
            speechEndedAt = nil
            phase = .listening
        case .userSpeechEnded:
            speechEndedAt = Date()
            if server.answersItself { phase = .thinking }
        case .userTranscript(let text, let final):
            appendUserTranscript(text, final: final)
            if final {
                if text.trimmingCharacters(in: .whitespaces).isEmpty {
                    settle()
                } else if !server.answersItself {
                    forwardToKernel(text)
                } else {
                    phase = .thinking
                }
            }
        case .thinking:
            responseDone = false
            if phase != .speaking { phase = .thinking }
        case .assistantAudio(let pcm):
            if let speechEndedAt {
                replyLatency = Date().timeIntervalSince(speechEndedAt)
                self.speechEndedAt = nil
                updateNote()
            }
            responseDone = false
            phase = .speaking
            audio.play(pcm16: pcm)
        case .assistantTranscript(let delta):
            append(delta, to: .arbos)
        case .responseDone:
            responseDone = true
            settle()
        case .toolCall(let name, let summary):
            appendSystem("\(name)\(summary.isEmpty ? "" : " · \(summary)")")
        case .agentDone(_, let text):
            appendSystem(text)
        case .error(let message):
            fail(message)
        case .closed:
            fail("Connection closed.")
        case .textDelta, .textDone, .toolResult, .agentEvent, .agentTurn, .agentTree:
            break
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

    // MARK: - Kernel (pipeline shape)

    private func forwardToKernel(_ text: String) {
        guard chat.mode != .offline else { return }
        kernelBusy = true
        phase = .thinking
        chat.send(text)
    }

    // MARK: - Transcript

    /// Deltas are increments to the open line; `final` replaces the whole
    /// line with the server's cleaned-up text and closes it.
    private func appendUserTranscript(_ text: String, final: Bool) {
        if final {
            if let index = lines.indices.last, lines[index].speaker == .user, openUtterance {
                if text.isEmpty { lines.remove(at: index) } else { lines[index].text = text }
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
            let current = lines[index].text
            // Sentence-sized chunks arrive without a joining space.
            let needsSpace = !current.isEmpty && !(current.last?.isWhitespace ?? true)
                && !(delta.first?.isWhitespace ?? true) && !(delta.first?.isPunctuation ?? false)
            lines[index].text = current + (needsSpace ? " " : "") + delta
        } else {
            lines.append(TranscriptLine(speaker: speaker, text: delta))
        }
        trimLines()
    }

    private func appendSystem(_ text: String) {
        guard !text.isEmpty else { return }
        lines.append(TranscriptLine(speaker: .system, text: text))
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
        #if DEBUG
        injectTask?.cancel()
        injectTask = nil
        #endif
        link.unsubscribe(subscription)
        subscription = nil
        busyWatch = nil
        chat.onAgentMessage = nil
        audio.onCapture = nil
        audio.stop()
        link.disconnect()
        startedAt = nil
        kernelBusy = false
        speechEndedAt = nil
    }

    #if DEBUG
    /// `-previewPhase listening` shows a screen state without a server.
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

    /// `-injectWav /path/to/24k-mono-pcm16.wav` plays a file into the
    /// session as if it were the microphone, paced in real time. The
    /// simulator has no usable mic; this is how a round trip is tested.
    private func injectWavIfAsked() {
        guard let path = UserDefaults.standard.string(forKey: "injectWav"),
              let data = FileManager.default.contents(atPath: path) else { return }
        let pcm = Self.pcmPayload(of: data)
        let sink = link.audioSink()
        let frame = Int(AudioEngine.sampleRate) * 2 / 25   // 40 ms
        injectTask = Task.detached {
            try? await Task.sleep(for: .milliseconds(800))
            var offset = 0
            while offset < pcm.count, !Task.isCancelled {
                let end = min(offset + frame, pcm.count)
                sink(pcm.subdata(in: offset..<end))
                offset = end
                try? await Task.sleep(for: .milliseconds(40))
            }
            // Then silence for the rest of the call: a full-duplex model
            // only advances while audio keeps arriving, like a real mic.
            let silence = Data(count: frame)
            while !Task.isCancelled {
                sink(silence)
                try? await Task.sleep(for: .milliseconds(40))
            }
        }
    }

    private var injectTask: Task<Void, Never>?

    private static func pcmPayload(of wav: Data) -> Data {
        guard let range = wav.range(of: Data("data".utf8)), range.upperBound + 4 <= wav.count else { return wav }
        return wav.subdata(in: (range.upperBound + 4)..<wav.count)
    }
    #endif
}
