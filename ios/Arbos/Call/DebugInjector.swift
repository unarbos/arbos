#if DEBUG
import Foundation

/// Test-only stand-in for the microphone. Streams silence to the server
/// at mic pace (a full-duplex model only advances while audio arrives) and
/// plays PCM clips into that stream on request. Drives scripted round
/// trips on the simulator (no usable mic) and on a device nobody is
/// talking into.
///
/// Launch arguments: `-injectWav <file>` for the opening utterance and
/// `-bargeWav <file>` for a second one fired while the reply plays.
/// Bare file names resolve inside the app's Documents folder, where
/// `xcrun devicectl device copy to` can put them.
final class DebugInjector {
    static let frameMilliseconds = 40

    private let sink: (Data) -> Void
    private let frameBytes: Int
    private let lock = NSLock()
    private var queued = Data()
    private var task: Task<Void, Never>?

    init(sink: @escaping (Data) -> Void) {
        self.sink = sink
        frameBytes = Int(AudioEngine.sampleRate) * 2 * Self.frameMilliseconds / 1000
    }

    /// Either stand-in for the microphone is in play, so the real one stays
    /// shut: `-injectWav` writes to the socket, `-micWav` to the capture
    /// path (see `CallViewModel.startMicClipIfAsked`).
    static func isRequested() -> Bool {
        UserDefaults.standard.string(forKey: "injectWav") != nil
            || UserDefaults.standard.string(forKey: "micWav") != nil
    }

    /// Only `-injectWav` drives the socket-side injector; with `-micWav` the
    /// frames arrive through capture instead and a second stream would give
    /// the duplex model two clocks.
    static func socketInjectionRequested() -> Bool {
        UserDefaults.standard.string(forKey: "injectWav") != nil
    }

    static func clip(named key: String) -> Data? {
        guard let raw = UserDefaults.standard.string(forKey: key) else { return nil }
        let path: String
        if raw.hasPrefix("/") {
            path = raw
        } else {
            let documents = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
            path = documents.appendingPathComponent(raw).path
        }
        guard let wav = FileManager.default.contents(atPath: path) else {
            print("metric inject_missing \(path)")
            return nil
        }
        let payload = pcmPayload(of: wav)
        if payload == nil { print("metric inject_unreadable \(path) bytes=\(wav.count)") }
        return payload
    }

    func start() {
        let silence = Data(count: frameBytes)
        task = Task.detached { [weak self] in
            while let self, !Task.isCancelled {
                self.sink(self.nextFrame() ?? silence)
                try? await Task.sleep(for: .milliseconds(Self.frameMilliseconds))
            }
        }
    }

    func stop() {
        task?.cancel()
        task = nil
    }

    /// Queue a clip; it goes out frame by frame in place of silence.
    func play(_ pcm: Data) {
        lock.lock()
        queued.append(pcm)
        lock.unlock()
    }

    private func nextFrame() -> Data? {
        lock.lock(); defer { lock.unlock() }
        guard !queued.isEmpty else { return nil }
        let take = min(frameBytes, queued.count)
        var frame = queued.prefix(take)
        queued.removeFirst(take)
        if frame.count < frameBytes { frame.append(Data(count: frameBytes - frame.count)) }
        return Data(frame)
    }

    private static func pcmPayload(of wav: Data) -> Data {
        guard let range = wav.range(of: Data("data".utf8)), range.upperBound + 4 <= wav.count else { return wav }
        return wav.subdata(in: (range.upperBound + 4)..<wav.count)
    }
}
#endif
