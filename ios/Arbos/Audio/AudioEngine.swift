import AVFoundation
import Foundation

/// Microphone in, model speech out, both as PCM16 mono 24 kHz.
///
/// `.playAndRecord` + `.voiceChat` turns on the system echo canceller, so
/// the model does not hear itself through the speaker. The `audio`
/// background mode (Info.plist, set in the project) keeps the engine alive
/// with the screen locked.
final class AudioEngine {
    static let sampleRate: Double = 24_000

    /// Called on the audio thread with one frame of PCM16 mic audio.
    var onCapture: ((Data) -> Void)?
    /// Called (on the audio thread) when every scheduled reply chunk has
    /// been heard.
    var onPlaybackDrained: (() -> Void)?

    private let engine = AVAudioEngine()
    private let player = AVAudioPlayerNode()
    private let wireFormat = AVAudioFormat(
        commonFormat: .pcmFormatInt16, sampleRate: sampleRate, channels: 1, interleaved: true
    )!
    private let playFormat = AVAudioFormat(standardFormatWithSampleRate: sampleRate, channels: 1)!
    private var converter: AVAudioConverter?
    private let lock = NSLock()
    private var scheduled = 0
    private var generation = 0
    private var observers: [NSObjectProtocol] = []

    var isPlaying: Bool {
        lock.lock(); defer { lock.unlock() }
        return scheduled > 0
    }

    func start() throws {
        try configureSession()
        engine.attach(player)
        engine.connect(player, to: engine.mainMixerNode, format: playFormat)

        let input = engine.inputNode
        let inputFormat = input.outputFormat(forBus: 0)
        guard inputFormat.sampleRate > 0, inputFormat.channelCount > 0 else {
            throw AudioEngineError.noInput
        }
        converter = AVAudioConverter(from: inputFormat, to: wireFormat)
        input.installTap(onBus: 0, bufferSize: 1024, format: inputFormat) { [weak self] buffer, _ in
            self?.capture(buffer)
        }
        engine.prepare()
        try engine.start()
        player.play()
        observeInterruptions()
    }

    func stop() {
        observers.forEach(NotificationCenter.default.removeObserver)
        observers.removeAll()
        engine.inputNode.removeTap(onBus: 0)
        player.stop()
        engine.stop()
        try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
    }

    /// Queue one chunk of model speech.
    func play(pcm16 data: Data) {
        let frames = data.count / MemoryLayout<Int16>.size
        guard frames > 0,
              let buffer = AVAudioPCMBuffer(pcmFormat: playFormat, frameCapacity: AVAudioFrameCount(frames)),
              let channel = buffer.floatChannelData?[0] else { return }
        buffer.frameLength = AVAudioFrameCount(frames)
        data.withUnsafeBytes { raw in
            let samples = raw.bindMemory(to: Int16.self)
            for i in 0..<frames {
                channel[i] = Float(Int16(littleEndian: samples[i])) / 32768
            }
        }
        lock.lock()
        scheduled += 1
        let current = generation
        lock.unlock()
        player.scheduleBuffer(buffer) { [weak self] in
            self?.consumed(generation: current)
        }
        if !player.isPlaying { player.play() }
    }

    /// Barge-in: throw away everything queued and go quiet at once.
    func stopPlayback() {
        lock.lock()
        generation += 1
        scheduled = 0
        lock.unlock()
        player.stop()
        player.play()
    }

    // MARK: - Private

    private func configureSession() throws {
        let session = AVAudioSession.sharedInstance()
        try session.setCategory(
            .playAndRecord,
            mode: .voiceChat,
            options: [.allowBluetoothHFP, .defaultToSpeaker]
        )
        try session.setPreferredSampleRate(Self.sampleRate)
        try session.setPreferredIOBufferDuration(0.02)
        try session.setActive(true)
    }

    private func capture(_ buffer: AVAudioPCMBuffer) {
        guard let converter, buffer.frameLength > 0 else { return }
        let ratio = wireFormat.sampleRate / buffer.format.sampleRate
        let capacity = AVAudioFrameCount(Double(buffer.frameLength) * ratio) + 32
        guard let out = AVAudioPCMBuffer(pcmFormat: wireFormat, frameCapacity: capacity) else { return }
        var handedOver = false
        var error: NSError?
        converter.convert(to: out, error: &error) { _, status in
            if handedOver {
                status.pointee = .noDataNow
                return nil
            }
            handedOver = true
            status.pointee = .haveData
            return buffer
        }
        guard error == nil, out.frameLength > 0, let channel = out.int16ChannelData?[0] else { return }
        onCapture?(Data(bytes: channel, count: Int(out.frameLength) * MemoryLayout<Int16>.size))
    }

    private func consumed(generation: Int) {
        lock.lock()
        guard generation == self.generation else {
            lock.unlock()
            return
        }
        scheduled = max(0, scheduled - 1)
        let drained = scheduled == 0
        lock.unlock()
        if drained { onPlaybackDrained?() }
    }

    /// A phone call or Siri takes the session; restart when it hands back.
    private func observeInterruptions() {
        let center = NotificationCenter.default
        let token = center.addObserver(
            forName: AVAudioSession.interruptionNotification,
            object: AVAudioSession.sharedInstance(),
            queue: .main
        ) { [weak self] note in
            guard let self,
                  let raw = note.userInfo?[AVAudioSessionInterruptionTypeKey] as? UInt,
                  let kind = AVAudioSession.InterruptionType(rawValue: raw) else { return }
            switch kind {
            case .began:
                self.stopPlayback()
            case .ended:
                try? AVAudioSession.sharedInstance().setActive(true)
                try? self.engine.start()
                self.player.play()
            @unknown default:
                break
            }
        }
        observers.append(token)
    }
}

enum AudioEngineError: LocalizedError {
    case noInput

    var errorDescription: String? {
        switch self {
        case .noInput: return "No microphone input."
        }
    }
}
