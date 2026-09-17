import Foundation

/// The composer's microphone: words spoken into the field. The speech
/// server transcribes (`session.start {mode: dictation}`) and answers
/// nothing; `transcript.delta` appends to the open segment, `transcript.final`
/// settles it. The words show in the field while he speaks, and go as one
/// line when he taps the microphone again (Jacob, build 956: a voice note
/// should land in the chat by itself).
@MainActor
final class Dictation: ObservableObject {
    @Published private(set) var active = false
    /// Everything transcribed so far in this take.
    @Published private(set) var text = ""
    @Published private(set) var level: Float = 0
    @Published private(set) var problem: String?

    private var committed = ""
    private var partial = ""
    private var session: SelfHostedVoiceSession?
    private var pump: Task<Void, Never>?
    private let audio = AudioEngine()
    private var startedAt: Date?
    #if DEBUG
    private var injector: DebugInjector?
    /// Transcript segments seen in this take, and how many *deltas* made
    /// the text shorter. A take whose deltas only grow has appended; a delta
    /// that shortens it has replaced, which is the build-956 fault. Finals
    /// are excluded: a final is the server settling the segment and is
    /// allowed to tidy it shorter.
    private var segments = 0
    private var deltaShrinks = 0
    #endif

    func start(settings: AppSettings) {
        guard !active else { return }
        problem = nil
        committed = ""
        partial = ""
        text = ""
        #if DEBUG
        segments = 0
        deltaShrinks = 0
        #endif
        guard settings.provider == .selfHosted, !settings.selfHostedURL.isEmpty, !settings.voiceToken.isEmpty else {
            problem = "Set the speech server in Settings to dictate."
            return
        }
        let session = SelfHostedVoiceSession(serverURL: settings.selfHostedURL, token: settings.voiceToken, mode: "dictation")
        self.session = session
        active = true
        startedAt = Date()
        pump = Task { [weak self] in
            for await event in session.events {
                guard let self, !Task.isCancelled else { return }
                self.handle(event)
            }
        }
        Task {
            do {
                var captureMic = true
                #if DEBUG
                captureMic = UserDefaults.standard.string(forKey: "dictateWav") == nil
                #endif
                try await session.connect()
                try audio.start(captureMic: captureMic)
                audio.onInputLevel = { [weak self] value in self?.level = value }
                audio.onCapture = { [weak session] frame in session?.send(audio: frame) }
                #if DEBUG
                if !captureMic { injectClip(into: session) }
                #endif
            } catch {
                fail(error.localizedDescription)
            }
        }
    }

    /// Stop listening; the words stay.
    func stop() {
        guard active else { return }
        pump?.cancel()
        pump = nil
        audio.onCapture = nil
        audio.stop()
        session?.close()
        session = nil
        #if DEBUG
        injector?.stop()
        injector = nil
        #endif
        active = false
        level = 0
        if !partial.isEmpty {
            committed = join(committed, partial)
            partial = ""
            text = committed
        }
        #if DEBUG
        print("metric dictation_take segments=\(segments) delta_shrinks=\(deltaShrinks) chars=\(text.count)")
        #endif
    }

    /// The field took the words: start clean next time.
    func consume() -> String {
        let out = text
        committed = ""
        partial = ""
        text = ""
        return out
    }

    private func handle(_ event: VoiceEvent) {
        switch event {
        case .userTranscript(let words, let final):
            if final {
                // The final is the whole segment; an empty one means the
                // server gave up on it, and the deltas already heard stay
                // rather than vanish (build 956 kept only the last delta).
                committed = join(committed, words.isEmpty ? partial : words)
                partial = ""
            } else {
                partial = join(partial, words)
            }
            #if DEBUG
            // Counted rather than eyeballed. The build-956 fault was a
            // delta replacing the take instead of extending it, which looks
            // like a plausible short sentence on screen and is only obvious
            // as a length that goes down.
            //
            // Only a *delta* shrinking is that fault. A final legitimately
            // makes the text shorter: it is the server's settled reading of
            // the segment, and settling tidies. The first run of this metric
            // reported shrinks=1 on a final and would have had me chasing a
            // bug that was the server doing its job.
            let was = text.count
            defer {
                segments += 1
                if text.count < was, !final { deltaShrinks += 1 }
                print("metric dictation seg=\(segments) final=\(final) was=\(was) now=\(text.count) delta_shrinks=\(deltaShrinks)")
            }
            #endif
            text = join(committed, partial)
        case .error(let message):
            fail(message)
        case .closed:
            if active { stop() }
        default:
            break
        }
    }

    private func join(_ a: String, _ b: String) -> String {
        let b = b.trimmingCharacters(in: .whitespaces)
        if a.isEmpty { return b }
        if b.isEmpty { return a }
        return a + (a.hasSuffix(" ") ? "" : " ") + b
    }

    /// The take stopped before it was meant to. `stop()` keeps whatever was
    /// heard in the field, so this says so: the words are the part Jacob
    /// cares about, and a bare error string does not mention them.
    private func fail(_ message: String) {
        stop()
        problem = heard.isEmpty
            ? "The speech server stopped listening, and nothing was transcribed. Tap the microphone to try again."
            : "The speech server stopped listening early. What it heard is in the box — read it before you send."
        #if DEBUG
        print("metric dictation_failed chars=\(heard.count) reason=\(message)")
        #endif
    }

    /// What the take has captured, for deciding what a failure should say.
    private var heard: String { text }

    #if DEBUG
    /// `-dictateWav <path>`: a clip in place of the microphone, for the
    /// simulator (no mic) and scripted runs.
    private func injectClip(into session: SelfHostedVoiceSession) {
        let injector = DebugInjector(sink: { [weak session] frame in session?.send(audio: frame) })
        self.injector = injector
        injector.start()
        if let clip = DebugInjector.clip(named: "dictateWav") {
            Task {
                try? await Task.sleep(for: .milliseconds(600))
                injector.play(clip)
            }
        }
    }
    #endif
}
