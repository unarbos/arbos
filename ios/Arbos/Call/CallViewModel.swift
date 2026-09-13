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
        var captureMic = true
        #if DEBUG
        captureMic = !DebugInjector.isRequested()
        #endif
        if captureMic {
            guard await AVAudioApplication.requestRecordPermission() else {
                phase = .failed("Microphone access is off.")
                return
            }
        }
        let connectStarted = Date()
        subscription = link.subscribe { [weak self] event in self?.handle(event) }
        audio.onPlaybackDrained = { [weak self] in
            Task { @MainActor in self?.playbackDrained() }
        }
        do {
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
        metric("connect", since: connectStarted, detail: server.engine)
        await joinChat()
        updateNote()
        #if DEBUG
        startInjectionIfAsked()
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
            trace("event speech.started playing=\(audio.isPlaying) phase=\(phase.label)")
            // Barge-in: whatever Arbos was saying stops now.
            if audio.isPlaying || phase == .speaking {
                audio.stopPlayback()
                link.interrupt()
                responseDone = true
                metric("barge_in_speech_started", since: bargeStartedAt)
            }
            speechEndedAt = nil
            phase = .listening
        case .userSpeechEnded:
            speechEndedAt = Date()
            if server.answersItself { phase = .thinking }
        case .userTranscript(let text, let final):
            appendUserTranscript(text, final: final)
            if final {
                trace("transcript: \(text)")
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
                metric("reply_first_audio", since: speechEndedAt)
                #if DEBUG
                scheduleBargeIn()
                #endif
            }
            responseDone = false
            phase = .speaking
            audio.play(pcm16: pcm)
        case .assistantTranscript(let delta):
            trace("reply: \(delta)")
            append(delta, to: .arbos)
        case .responseDone(let interrupted):
            trace("event response.done interrupted=\(interrupted) playing=\(audio.isPlaying)")
            if interrupted { metric("barge_in_response_done", since: bargeStartedAt) }
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

    /// One line on the console per measured hop, for scripted runs.
    private func metric(_ name: String, since start: Date?, detail: String = "") {
        #if DEBUG
        guard let start else { return }
        let ms = Int(Date().timeIntervalSince(start) * 1000)
        print("metric \(name) \(ms)ms \(detail)".trimmingCharacters(in: .whitespaces))
        #endif
    }

    private func trace(_ line: @autoclosure () -> String) {
        #if DEBUG
        print(line())
        #endif
    }

    private var bargeStartedAt: Date?

    private func teardown() {
        #if DEBUG
        injector?.stop()
        injector = nil
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

    private var injector: DebugInjector?
    private var bargeClip: Data?

    /// `-injectWav` replaces the microphone with a clip (see
    /// `DebugInjector`); `-bargeWav` fires a second clip 1.5 s into the
    /// reply to exercise barge-in.
    private func startInjectionIfAsked() {
        guard DebugInjector.isRequested() else { return }
        let injector = DebugInjector(sink: link.audioSink())
        self.injector = injector
        bargeClip = DebugInjector.clip(named: "bargeWav")
        injector.start()
        Task {
            try? await Task.sleep(for: .milliseconds(800))
            if let clip = DebugInjector.clip(named: "injectWav") { injector.play(clip) }
        }
    }

    private func scheduleBargeIn() {
        guard let clip = bargeClip, let injector else { return }
        bargeClip = nil
        Task {
            try? await Task.sleep(for: .milliseconds(1500))
            guard phase == .speaking else {
                print("metric barge_in_skipped reply already over")
                return
            }
            bargeStartedAt = Date()
            injector.play(clip)
        }
    }
    #endif
}
