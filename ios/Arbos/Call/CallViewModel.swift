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
    /// The live level, 0…1: the microphone while Jacob talks, the reply
    /// while Arbos speaks. Drives the ring on the call screen.
    @Published private(set) var level: Float = 0
    /// The microphone is muted: silence goes to the server instead.
    @Published var muted = false {
        didSet { mutedNow = muted }
    }
    /// Read off the audio thread; mirrors `muted`.
    nonisolated(unsafe) private var mutedNow = false
    /// Where the sound goes: `speaker`, `AirPods`, `headphones`, …
    var outputRoute: String { route }

    private let settings: AppSettings
    private let chat: ChatStore
    private let link: VoiceLink
    private let audio = AudioEngine()
    private var subscription: UUID?
    private var busyWatch: AnyCancellable?
    private(set) var server = VoiceServerInfo()
    /// The speech side finished sending the reply; playback may still be
    /// draining.
    private var responseDone = true
    /// Whether the response now open has played anything. A response closed
    /// without audio never reached the caller's ears and should not be drawn
    /// as one that finished.
    private var responseHadAudio = false
    /// The kernel is mid-turn on our behalf (pipeline shape only).
    private var kernelBusy = false
    /// The working sound and what drives it: the gateway's `agent.activity`
    /// per agent (state), the tool named on the note, and a watch that
    /// stops the sound when no frame has come for a while — a dropped
    /// link must not tick forever.
    private let work = WorkSound()
    private var activity: [String: String] = [:]
    private var workDetail: String?
    private var activityWatch: Task<Void, Never>?
    private static let activityStale: TimeInterval = 12
    /// The caller is talking: the working sound waits for them too.
    private var userTalking = false
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
        trace("startCall")
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
        audio.onInputLevel = { [weak self] value in
            guard let self, self.phase != .speaking else { return }
            self.meter(value)
        }
        audio.onOutputLevel = { [weak self] value in
            guard let self, self.phase == .speaking || value > 0 else { return }
            self.meter(value)
        }
        audio.onRouteChange = { [weak self] route in
            guard let self else { return }
            self.route = route
            self.trace("route change outputs=[\(self.audio.outputPorts)] volume=\(self.audio.systemVolume)")
            self.updateNote()
        }
        #if DEBUG
        let defaults = UserDefaults.standard
        if defaults.bool(forKey: "forceSpeaker") { audio.preferSpeaker = true }
        switch defaults.string(forKey: "audioMode") {
        case "voiceChat": audio.mode = .voiceChat
        case "videoChat": audio.mode = .videoChat
        case "default": audio.mode = .default
        case "measurement": audio.mode = .measurement
        default: break
        }
        if defaults.object(forKey: "normalise") != nil { audio.normalise = defaults.bool(forKey: "normalise") }
        #endif
        // The microphone opens before the socket does, and connecting takes
        // about two thirds of a second. Until this cycle the captured frames
        // went nowhere for that whole window, because `onCapture` was only
        // wired to the sink after `connect()` returned — so a person who
        // taps call and starts talking loses their first word. Hold the
        // frames instead and send them the moment there is somewhere to send
        // them; the newest two seconds are worth keeping, and anyone silent
        // for longer than that has said nothing to lose.
        var held: [Data] = []
        var heldBytes = 0
        audio.onCapture = { frame in
            held.append(frame)
            heldBytes += frame.count
            while heldBytes > Self.heldCaptureLimit, let oldest = held.first {
                held.removeFirst()
                heldBytes -= oldest.count
            }
        }
        do {
            try audio.start(captureMic: captureMic)
            #if DEBUG
            startMicClipIfAsked()
            #endif
            try await link.connect()
        } catch {
            audio.onCapture = nil
            fail(error.localizedDescription)
            return
        }
        guard phase.inCall else { return }
        if let info = link.info { server = info }
        let sink = link.audioSink()
        // Muted: the same frames go out as silence, so the duplex model
        // keeps its clock and nothing of the room is heard.
        let send: (Data) -> Void = { [weak self] frame in
            #if DEBUG
            self?.framesSent += 1
            #endif
            sink(self?.mutedNow == true ? Data(count: frame.count) : frame)
        }
        for frame in held { send(frame) }
        held.removeAll()
        audio.onCapture = send
        route = audio.outputRoute
        startedAt = Date()
        phase = .listening
        metric("connect", since: connectStarted, detail: "\(server.engine) route=\(route)")
        trace("audio mode=\(audio.mode.rawValue) outputs=[\(audio.outputPorts)] volume=\(audio.systemVolume) normalise=\(audio.normalise)")
        #if DEBUG
        if UserDefaults.standard.bool(forKey: "toneTest") {
            // A 1 kHz tone at -3 dBFS through the same path as a reply:
            // if this is loud and TTS is quiet, the source is quiet.
            Task {
                try? await Task.sleep(for: .seconds(3))
                link.interrupt()
                audio.stopPlayback()
                try? await Task.sleep(for: .milliseconds(600))
                trace("toneTest start")
                phase = .speaking
                audio.play(pcm16: Self.tone(seconds: 2))
            }
        }
        if let text = UserDefaults.standard.string(forKey: "speakTest") {
            Task {
                // Cut the model's opening line so the measurement hears
                // only the fixed sentence.
                try? await Task.sleep(for: .seconds(3))
                link.interrupt()
                audio.stopPlayback()
                try? await Task.sleep(for: .milliseconds(600))
                trace("speakTest start")
                // Underscores stand in for spaces on the launch line.
                link.speak(text.replacingOccurrences(of: "_", with: " "))
            }
        }
        #endif
        #if DEBUG
        // Before the chat joins: on a hub-attached kernel the join can take
        // longer than the model's first reply, and the barge clip must be
        // armed by then.
        startInjectionIfAsked()
        #endif
        await joinChat()
        updateNote()
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

    private var route = ""

    /// A voice envelope for the ring: rises at once, falls over ~0.3 s
    /// (updates arrive every 40–45 ms), so the disc breathes with the
    /// words instead of flickering between silence and peaks.
    private func meter(_ value: Float) {
        // Rise at once, fall over ~0.2 s: word gaps show, syllables do not flicker.
        level = value >= level ? value : max(value, level - 0.3)
    }

    /// Words typed in the pulled-down composer: to the project's chat, as
    /// a typed turn would be. In the pipeline shape the reply is spoken.
    func sendTyped(_ text: String, attachments: [PendingAttachment] = []) {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty || !attachments.isEmpty else { return }
        let shown = attachments.isEmpty ? trimmed : (trimmed.isEmpty ? "" : trimmed + " ") + "📎 " + attachments.map(\.name).joined(separator: ", ")
        lines.append(TranscriptLine(speaker: .user, text: shown))
        trimLines()
        if !server.answersItself { kernelBusy = true; phase = .thinking }
        chat.send(trimmed, attachments: attachments)
    }

    /// Phone speaker instead of a connected headset, and back.
    func toggleSpeaker() {
        audio.preferSpeaker.toggle()
        route = audio.outputRoute
        updateNote()
    }

    /// `speaker · 100%`: where the sound goes and the system volume there.
    private var routeBadge: String {
        guard !route.isEmpty else { return "" }
        return "\(route) · \(Int((audio.systemVolume * 100).rounded()))%"
    }

    private func updateNote() {
        var parts: [String] = []
        if !server.engine.isEmpty { parts.append(server.engine) }
        if !routeBadge.isEmpty { parts.append(routeBadge) }
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
        if let workDetail { parts.append(workDetail) }
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
                markSpeaking(false, after: 0)
                responseDone = true
                metric("barge_in_speech_started", since: bargeStartedAt)
            }
            speechEndedAt = nil
            phase = .listening
            userTalking = true
            updateDuck()
        case .userSpeechEnded:
            speechEndedAt = Date()
            if server.answersItself { phase = .thinking }
            userTalking = false
            updateDuck()
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
            responseHadAudio = true
            if phase != .speaking {
                trace("playback start outputs=[\(audio.outputPorts)] volume=\(audio.systemVolume)")
            }
            phase = .speaking
            markSpeaking(true)
            audio.play(pcm16: pcm)
            updateDuck()
        case .assistantTranscript(let delta):
            trace("reply: \(delta)")
            append(delta, to: .arbos)
        case .responseDone(let end):
            let levels = audio.replyLevelsAndReset()
            trace("event response.done reason=\(end.rawValue) playing=\(audio.isPlaying) reply peak=\(Int(levels.peak))dBFS rms=\(Int(levels.rms))dBFS out=\(Int(levels.out))dBFS")
            if end == .interrupted { metric("barge_in_response_done", since: bargeStartedAt) }
            responseDone = true
            // Only a reply the caller could have heard is an answer ending.
            // The server says which now; the silence check stays behind it,
            // because a reply that reached nobody must not take the screen
            // back to listening whatever the frame calls itself.
            if end.reachedTheCaller, responseHadAudio {
                settle()
            } else {
                trace("response.done \(end.rawValue), nothing heard — staying in \(phase.label)")
            }
            responseHadAudio = false
        case .toolCall(let name, let summary):
            appendSystem("\(name)\(summary.isEmpty ? "" : " · \(summary)")")
        case .agentDone(_, let text):
            appendSystem(text)
        case .agentActivity(let agent, let state, let tool, let detail):
            noteActivity(agent: agent, state: state, tool: tool, detail: detail)
        case .error(let message):
            fail(message)
        case .closed:
            fail("Connection closed.")
        case .textDelta, .textDone, .toolResult, .agentEvent, .agentTurn, .agentTree:
            break
        }
    }

    private func playbackDrained() {
        markSpeaking(false, after: 0.3)
        updateDuck()
        settle()
    }

    // MARK: - Working sound

    /// The gateway says what the call's agent is doing. The sound is on while
    /// any agent is not idle; a command starting on the main agent is one
    /// tick; the tool's name goes on the note so the silence has a reason.
    private func noteActivity(agent: String, state: String, tool: String?, detail: String?) {
        let wasTool = activity[agent] == "tool"
        activity[agent] = state
        let active = activity.values.contains { $0 != "idle" }
        if state == "tool", let tool {
            workDetail = detail.map { "\(tool) · \($0)" } ?? tool
        } else if active, agent == "root" {
            workDetail = "working"
        } else if !active {
            workDetail = nil
        }
        trace("event agent.activity agent=\(agent) state=\(state) tool=\(tool ?? "-") on=\(active)")
        work.set(on: active)
        if state == "tool", !wasTool, agent == "root" { work.tick() }
        updateNote()

        activityWatch?.cancel()
        activityWatch = nil
        guard active else { return }
        activityWatch = Task { [weak self] in
            try? await Task.sleep(for: .seconds(Self.activityStale))
            guard let self, !Task.isCancelled else { return }
            self.trace("agent.activity stale — sound off")
            self.activity.removeAll()
            self.workDetail = nil
            self.work.set(on: false)
            self.updateNote()
        }
    }

    /// The sound waits while a voice has the floor: the reply, or the caller.
    private func updateDuck() {
        work.duck(audio.isPlaying || userTalking)
    }

    private func stopWork() {
        activityWatch?.cancel()
        activityWatch = nil
        activity.removeAll()
        workDetail = nil
        userTalking = false
        work.set(on: false)
    }

    private var speakingMarked = false
    private var speakingOff: Task<Void, Never>?

    /// `client.speaking` to the server: on with the first reply chunk, off
    /// a beat after the speaker drains (echo tails the audio). New audio
    /// in that beat cancels the off.
    private func markSpeaking(_ speaking: Bool, after delay: TimeInterval = 0) {
        speakingOff?.cancel()
        speakingOff = nil
        if speaking {
            guard !speakingMarked else { return }
            speakingMarked = true
            link.setSpeaking(true, route: audio.gateRoute)
        } else if speakingMarked {
            speakingOff = Task { [weak self] in
                if delay > 0 { try? await Task.sleep(for: .seconds(delay)) }
                guard let self, !Task.isCancelled else { return }
                self.speakingMarked = false
                self.link.setSpeaking(false, route: self.audio.gateRoute)
            }
        }
    }

    /// Back to listening once nobody is working and nothing is playing.
    private func settle() {
        guard phase.inCall, responseDone, !kernelBusy, !audio.isPlaying else { return }
        if phase != .listening { trace("phase listening") }
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

    /// Two seconds of captured audio, at the wire format (24 kHz, mono,
    /// 16-bit): what is held while the socket is still connecting.
    private static let heldCaptureLimit = Int(AudioEngine.sampleRate) * 2 * MemoryLayout<Int16>.size

    #if DEBUG
    /// `-micWav <file>`: play a clip down the **capture** path, starting the
    /// moment the engine is up and so before the socket is.
    ///
    /// `-injectWav` cannot test this. It writes straight into the socket's
    /// sink, bypassing the microphone altogether, which is right for
    /// measuring round trips and useless for asking what happens to audio
    /// captured before there is a socket to send it to. That question is
    /// what dropped a caller's first word, and nothing in the rig could see
    /// it.
    private func startMicClipIfAsked() {
        guard let clip = DebugInjector.clip(named: "micWav") else { return }
        let frameBytes = Int(AudioEngine.sampleRate) * 2 * DebugInjector.frameMilliseconds / 1000
        let silence = Data(count: frameBytes)
        micClipTask = Task.detached { [weak self] in
            var offset = 0
            // Paced against a fixed start, not by sleeping a frame's worth
            // each time round: sleeping accumulates the loop's own cost, the
            // stream falls behind real time, and the server's voice
            // detection reads the shortfall as pauses and cuts the sentence
            // up. A rig that mis-paces invents the faults it reports.
            let started = ContinuousClock.now
            var sent = 0
            // The clip, then silence for as long as the call lasts: a real
            // microphone does not stop producing frames when someone stops
            // talking, and a duplex model only advances while audio arrives.
            while !Task.isCancelled {
                let frame: Data
                if offset < clip.count {
                    let end = min(offset + frameBytes, clip.count)
                    frame = clip.subdata(in: offset..<end)
                    offset = end
                } else {
                    frame = silence
                }
                guard let self else { return }
                await MainActor.run {
                    self.framesFromClip += 1
                    self.audio.onCapture?(frame)
                    // Periodically, because a scripted run ends by killing
                    // the process and teardown never gets to say anything.
                    if self.framesFromClip % 50 == 0 {
                        print("metric mic_frames clip=\(self.framesFromClip) sent=\(self.framesSent)")
                    }
                }
                sent += 1
                let due = started.advanced(by: .milliseconds(DebugInjector.frameMilliseconds * sent))
                try? await Task.sleep(until: due, clock: .continuous)
            }
        }
    }
    private var micClipTask: Task<Void, Never>?
    /// Frames the rig produced, and frames that reached the socket. They
    /// should match while `-micWav` runs; a gap means the engine is feeding
    /// the same sink and the stream has two producers.
    private var framesFromClip = 0
    var framesSent = 0

    private func stopMicClip() { micClipTask?.cancel(); micClipTask = nil }
    #endif

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
        if framesFromClip > 0 { print("metric mic_frames clip=\(framesFromClip) sent=\(framesSent)") }
        stopMicClip()
        #endif
        link.unsubscribe(subscription)
        subscription = nil
        speakingOff?.cancel()
        speakingOff = nil
        speakingMarked = false
        busyWatch = nil
        chat.onAgentMessage = nil
        audio.onCapture = nil
        audio.stop()
        link.disconnect()
        startedAt = nil
        kernelBusy = false
        stopWork()
        speechEndedAt = nil
        level = 0
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

    /// Level of a PCM16 frame, as the engine measures the microphone.
    nonisolated private static func level(of data: Data) -> Float {
        let frames = data.count / 2
        guard frames > 0 else { return 0 }
        var squares: Double = 0
        data.withUnsafeBytes { raw in
            let samples = raw.bindMemory(to: Int16.self)
            for i in 0..<frames { let v = Float(Int16(littleEndian: samples[i])) / 32768; squares += Double(v * v) }
        }
        return AudioEngine.level(rms: Float(squares / Double(frames)).squareRoot())
    }

    /// 1 kHz sine at -3 dBFS, PCM16 mono at the wire rate.
    private static func tone(seconds: Int) -> Data {
        let rate = Int(AudioEngine.sampleRate)
        let count = rate * seconds
        var samples = [Int16](repeating: 0, count: count)
        let amplitude: Double = 0.707 * 32767
        let step: Double = 2 * Double.pi * 1000 / Double(rate)
        for i in 0..<count {
            samples[i] = Int16(amplitude * sin(step * Double(i)))
        }
        return samples.withUnsafeBufferPointer { Data(buffer: $0) }
    }

    /// `-injectWav` replaces the microphone with a clip (see
    /// `DebugInjector`); `-bargeWav` fires a second clip 1.5 s into the
    /// reply to exercise barge-in.
    private func startInjectionIfAsked() {
        guard DebugInjector.socketInjectionRequested() else { return }
        let sink = link.audioSink()
        let injector = DebugInjector(sink: { [weak self] data in
            sink(data)
            let level = Self.level(of: data)
            Task { @MainActor in
                guard let self, self.phase != .speaking else { return }
                self.meter(level)
            }
        })
        self.injector = injector
        bargeClip = DebugInjector.clip(named: "bargeWav")
        injector.start()
        Task {
            try? await Task.sleep(for: .milliseconds(800))
            if let clip = DebugInjector.clip(named: "injectWav") { injector.play(clip) }
        }
    }

    private func scheduleBargeIn() {
        guard let clip = bargeClip, let injector else {
            print("metric barge_in_unarmed clip=\(bargeClip != nil) injector=\(injector != nil)")
            return
        }
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
