import AVFoundation
import Foundation

/// Microphone in, model speech out, both as PCM16 mono 24 kHz.
///
/// `.playAndRecord` + `.voiceChat` turns on the hardware echo canceller, so
/// the model mostly does not hear itself through the speaker. On top of
/// that an energy gate zeroes mic frames while a reply plays unless they
/// are clearly louder than the echo, so a full-duplex model keeps getting
/// a continuous stream (silence, not a gap) yet the user can still cut in.
/// The `audio` background mode keeps the engine alive with the screen
/// locked.
final class AudioEngine {
    static let sampleRate: Double = 24_000

    /// Called on the audio thread with one frame of PCM16 mic audio.
    var onCapture: ((Data) -> Void)?
    /// Called (on the audio thread) when every scheduled reply chunk has
    /// been heard.
    var onPlaybackDrained: (() -> Void)?
    /// Called on the main thread when the output route changes.
    var onRouteChange: ((String) -> Void)?

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
    private var lastPlaybackEnd: Date = .distantPast
    private var observers: [NSObjectProtocol] = []

    // Echo gate state, audio thread only.
    private var echoFloor: Double = 0
    private var speechHoldUntil: Date = .distantPast

    var isPlaying: Bool {
        lock.lock(); defer { lock.unlock() }
        return scheduled > 0
    }

    /// Where the reply comes out: `speaker`, `AirPods`, `headphones`, …
    var outputRoute: String {
        let outputs = AVAudioSession.sharedInstance().currentRoute.outputs
        guard let port = outputs.first else { return "no output" }
        switch port.portType {
        case .builtInSpeaker: return "speaker"
        case .builtInReceiver: return "earpiece"
        case .headphones: return "headphones"
        case .bluetoothHFP, .bluetoothA2DP, .bluetoothLE: return port.portName
        case .carAudio: return "car"
        default: return port.portName
        }
    }

    /// `captureMic: false` runs playback only (a test feeds audio itself).
    func start(captureMic: Bool = true) throws {
        try configureSession()
        engine.attach(player)
        engine.connect(player, to: engine.mainMixerNode, format: playFormat)
        engine.mainMixerNode.outputVolume = 1

        if captureMic {
            let input = engine.inputNode
            let inputFormat = input.outputFormat(forBus: 0)
            guard inputFormat.sampleRate > 0, inputFormat.channelCount > 0 else {
                throw AudioEngineError.noInput
            }
            converter = AVAudioConverter(from: inputFormat, to: wireFormat)
            input.installTap(onBus: 0, bufferSize: 1024, format: inputFormat) { [weak self] buffer, _ in
                self?.capture(buffer)
            }
        }
        engine.prepare()
        try engine.start()
        player.play()
        observeSession()
    }

    func stop() {
        observers.forEach(NotificationCenter.default.removeObserver)
        observers.removeAll()
        if converter != nil {
            engine.inputNode.removeTap(onBus: 0)
            converter = nil
        }
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
        lastPlaybackEnd = Date()
        lock.unlock()
        player.stop()
        player.play()
    }

    // MARK: - Session

    private func configureSession() throws {
        let session = AVAudioSession.sharedInstance()
        // `.defaultToSpeaker`: a call app is held in front of the face, not
        // to the ear. Bluetooth (AirPods) still wins when connected.
        try session.setCategory(
            .playAndRecord,
            mode: .voiceChat,
            options: [.defaultToSpeaker, .allowBluetoothHFP]
        )
        try session.setPreferredSampleRate(Self.sampleRate)
        try session.setPreferredIOBufferDuration(0.02)
        try session.setActive(true)
        routeToSpeakerIfEarpiece()
    }

    /// `.defaultToSpeaker` is honoured on activation, but a route change
    /// (unplugging headphones) can land the output back on the earpiece.
    private func routeToSpeakerIfEarpiece() {
        let session = AVAudioSession.sharedInstance()
        if session.currentRoute.outputs.contains(where: { $0.portType == .builtInReceiver }) {
            try? session.overrideOutputAudioPort(.speaker)
        }
    }

    private func observeSession() {
        let center = NotificationCenter.default
        let session = AVAudioSession.sharedInstance()
        observers.append(center.addObserver(
            forName: AVAudioSession.interruptionNotification, object: session, queue: .main
        ) { [weak self] note in
            guard let self,
                  let raw = note.userInfo?[AVAudioSessionInterruptionTypeKey] as? UInt,
                  let kind = AVAudioSession.InterruptionType(rawValue: raw) else { return }
            switch kind {
            case .began:
                self.stopPlayback()
            case .ended:
                try? session.setActive(true)
                try? self.engine.start()
                self.player.play()
            @unknown default:
                break
            }
        })
        observers.append(center.addObserver(
            forName: AVAudioSession.routeChangeNotification, object: session, queue: .main
        ) { [weak self] _ in
            guard let self else { return }
            self.routeToSpeakerIfEarpiece()
            self.onRouteChange?(self.outputRoute)
        })
    }

    // MARK: - Capture

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
        let count = Int(out.frameLength)
        if shouldMute(channel, count: count) {
            onCapture?(Data(count: count * MemoryLayout<Int16>.size))
        } else {
            onCapture?(Data(bytes: channel, count: count * MemoryLayout<Int16>.size))
        }
    }

    /// While a reply plays (and for a short tail after), only frames well
    /// above the running echo level go out; the rest is replaced by
    /// silence. Once the user is heard, the gate stays open for half a
    /// second so a sentence is not chopped.
    private func shouldMute(_ samples: UnsafeMutablePointer<Int16>, count: Int) -> Bool {
        lock.lock()
        let playing = scheduled > 0
        let recentlyPlaying = Date().timeIntervalSince(lastPlaybackEnd) < 0.3
        lock.unlock()
        guard playing || recentlyPlaying else {
            echoFloor *= 0.98
            return false
        }
        var sum: Double = 0
        for i in 0..<count {
            let s = Double(samples[i])
            sum += s * s
        }
        let rms = (sum / Double(max(count, 1))).squareRoot()
        let now = Date()
        if now < speechHoldUntil { return false }
        let threshold = max(Self.minSpeechRMS, echoFloor * 2.5)
        if rms > threshold {
            speechHoldUntil = now.addingTimeInterval(0.5)
            return false
        }
        // Track the echo we are hearing so a louder reply raises the bar.
        echoFloor = echoFloor == 0 ? rms : echoFloor * 0.9 + rms * 0.1
        return true
    }

    /// About -27 dBFS in Int16 units: quieter than speech at arm's length,
    /// louder than the residue the canceller leaves behind.
    private static let minSpeechRMS: Double = 1500

    private func consumed(generation: Int) {
        lock.lock()
        guard generation == self.generation else {
            lock.unlock()
            return
        }
        scheduled = max(0, scheduled - 1)
        let drained = scheduled == 0
        if drained { lastPlaybackEnd = Date() }
        lock.unlock()
        if drained { onPlaybackDrained?() }
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
