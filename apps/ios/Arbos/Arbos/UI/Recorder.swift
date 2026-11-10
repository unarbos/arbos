// Hold-to-record voice memo: AVAudioRecorder to .m4a with live metering for
// the waveform in the composer. Callers run STT on the file and send or draft
// the transcript; the audio itself is not posted to the chat.

import AVFoundation
import Foundation

@MainActor
final class MemoRecorder: ObservableObject {
    @Published private(set) var recording = false
    @Published private(set) var seconds: Double = 0
    @Published private(set) var levels: [Float] = Array(repeating: 0.05, count: 28)
    @Published var permissionDenied = false

    private var permissionWork = WorkLifetime()
    private var recorder: AVAudioRecorder?
    private var timer: Timer?
    private var startedAt: Date?
    private var url: URL?

    func start(isCurrent: @escaping @MainActor () -> Bool = { true }) {
        guard !Task.isCancelled, isCurrent() else { return }
        let request = permissionWork.reopen()
        let session = AVAudioSession.sharedInstance()
        session.requestRecordPermission { [weak self] granted in
            Task { @MainActor in
                guard let self, !Task.isCancelled,
                      self.permissionWork.accepts(request), isCurrent() else { return }
                self.permissionWork.close()
                if !granted { self.permissionDenied = true; return }
                self.begin()
            }
        }
    }

    private func begin() {
        let session = AVAudioSession.sharedInstance()
        try? session.setCategory(.playAndRecord, mode: .default, options: [.defaultToSpeaker])
        try? session.setActive(true)
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("memo-\(UUID().uuidString).m4a")
        // 16 kHz mono is enough for speech and keeps the upload/STT payload small.
        let settings: [String: Any] = [
            AVFormatIDKey: Int(kAudioFormatMPEG4AAC),
            AVSampleRateKey: 16_000,
            AVNumberOfChannelsKey: 1,
            AVEncoderAudioQualityKey: AVAudioQuality.medium.rawValue,
        ]
        guard let rec = try? AVAudioRecorder(url: url, settings: settings) else {
            try? FileManager.default.removeItem(at: url)
            return
        }
        rec.isMeteringEnabled = true
        guard rec.record() else {
            rec.stop()
            try? FileManager.default.removeItem(at: url)
            return
        }
        recorder = rec
        self.url = url
        startedAt = Date()
        seconds = 0
        recording = true
        timer = Timer.scheduledTimer(withTimeInterval: 0.08, repeats: true) { [weak self] _ in
            Task { @MainActor in self?.tick() }
        }
    }

    private func tick() {
        guard let rec = recorder, let startedAt else { return }
        rec.updateMeters()
        let db = rec.averagePower(forChannel: 0)
        let level = max(0.05, min(1, (db + 50) / 50))
        levels.removeFirst()
        levels.append(level)
        seconds = Date().timeIntervalSince(startedAt)
    }

    /// Stops and returns the file data when the memo lasted at least a second.
    func stop() -> (name: String, data: Data)? {
        permissionWork.close()
        timer?.invalidate()
        timer = nil
        recorder?.stop()
        recording = false
        let finishedURL = url
        defer {
            recorder = nil
            url = nil
            if let finishedURL { try? FileManager.default.removeItem(at: finishedURL) }
        }
        guard let finishedURL, seconds >= 1, let data = try? Data(contentsOf: finishedURL) else { return nil }
        // POSIX stamp (ASCII only) — same idea as photo names; relay blob keys reject locale letters.
        let stamp = Self.memoStamp.string(from: Date())
        return ("Voice memo \(stamp).m4a", data)
    }

    private static let memoStamp: DateFormatter = {
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.dateFormat = "yyyy-MM-dd HH.mm.ss"
        return f
    }()

    func cancel() {
        permissionWork.close()
        timer?.invalidate()
        timer = nil
        recorder?.stop()
        recorder = nil
        recording = false
        if let url { try? FileManager.default.removeItem(at: url) }
        url = nil
    }

    var timeLabel: String {
        let s = Int(seconds)
        return String(format: "%d:%02d", s / 60, s % 60)
    }
}
