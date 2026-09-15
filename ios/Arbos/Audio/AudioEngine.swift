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
    /// Live levels for the call screen, 0…1 (−50 dBFS … −10 dBFS): the
    /// microphone per captured buffer, the reply per buffer as it plays.
    var onInputLevel: ((Float) -> Void)?
    var onOutputLevel: ((Float) -> Void)?
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
        // The reply's level as it actually plays, ~43 ms at a time.
        player.installTap(onBus: 0, bufferSize: 1024, format: playFormat) { [weak self] buffer, _ in
            guard let self, let onOutputLevel = self.onOutputLevel,
                  let channel = buffer.floatChannelData?[0] else { return }
            let count = Int(buffer.frameLength)
            var squares: Double = 0
            for i in 0..<count { squares += Double(channel[i] * channel[i]) }
            let level = Self.level(rms: Float(squares / Double(max(count, 1))).squareRoot())
            DispatchQueue.main.async { onOutputLevel(level) }
        }

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
        player.removeTap(onBus: 0)
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
        var peak: Float = 0
        var squares: Double = 0
        data.withUnsafeBytes { raw in
            let samples = raw.bindMemory(to: Int16.self)
            for i in 0..<frames {
                let s = Float(Int16(littleEndian: samples[i])) / 32768
                channel[i] = s
                peak = max(peak, abs(s))
                squares += Double(s * s)
            }
        }
        replyPeak = max(replyPeak, peak)
        replySquares += squares
        replySamples += frames

        if normalise { applyGain(channel, count: frames, chunkPeak: peak) }
        var sent: Float = 0
        for i in 0..<frames { sent = max(sent, abs(channel[i])) }
        outPeak = max(outPeak, sent)
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
        DispatchQueue.main.async { [weak self] in self?.onOutputLevel?(0) }
        player.stop()
        player.play()
    }

    // MARK: - Gain

    private var runningPeak: Float = 0
    private static let targetPeak: Float = 0.707   // -3 dBFS
    private static let maxGain: Float = 6           // +15.6 dB

    /// Gain follows the loudest recent chunk (fast up, slow down) so a
    /// whole reply sits near the target; a tanh knee catches overshoot.
    private func applyGain(_ channel: UnsafeMutablePointer<Float>, count: Int, chunkPeak: Float) {
        if chunkPeak > runningPeak {
            runningPeak = chunkPeak
        } else {
            runningPeak = runningPeak * 0.995 + chunkPeak * 0.005
        }
        guard runningPeak > 0.001 else { return }
        let gain = min(Self.maxGain, max(1, Self.targetPeak / runningPeak))
        guard gain > 1.01 else { return }
        for i in 0..<count {
            let v = channel[i] * gain
            channel[i] = abs(v) > 0.6 ? (v > 0 ? 1 : -1) * (0.6 + 0.4 * tanhf((abs(v) - 0.6) / 0.4)) : v
        }
    }

    // MARK: - Session

    private func configureSession() throws {
        let session = AVAudioSession.sharedInstance()
        // `.defaultToSpeaker`: a call app is held in front of the face, not
        // to the ear. Bluetooth (AirPods) still wins when connected.
        try session.setCategory(
            .playAndRecord,
            mode: mode,
            options: preferSpeaker ? [.defaultToSpeaker] : [.defaultToSpeaker, .allowBluetoothHFP]
        )
        try session.setPreferredSampleRate(Self.sampleRate)
        try session.setPreferredIOBufferDuration(0.02)
        try session.setActive(true)
        if preferSpeaker {
            try? session.overrideOutputAudioPort(.speaker)
        } else {
            routeToSpeakerIfEarpiece()
        }
    }

    /// The user wants the phone's own speaker even with AirPods connected.
    var preferSpeaker = false {
        didSet { applyRoutePreference() }
    }

    /// Session mode. `.voiceChat` runs the phone's voice processing: echo
    /// cancellation, but also a quieter, ducked speaker. `.videoChat`
    /// keeps the canceller and is tuned for the speaker.
    var mode: AVAudioSession.Mode = .videoChat

    /// Make-up gain for the reply: TTS often peaks well under full scale.
    /// Peaks are brought toward -3 dBFS with a soft limiter, never above.
    var normalise = true

    private var replyPeak: Float = 0
    private var replySquares: Double = 0
    private var replySamples: Int = 0
    private var outPeak: Float = 0

    /// Peak and RMS of the reply audio as received, and the peak actually
    /// sent to the speaker after gain, in dBFS, since the last call.
    func replyLevelsAndReset() -> (peak: Double, rms: Double, out: Double) {
        func dB(_ v: Float) -> Double { v > 0 ? 20 * log10(Double(v)) : -120 }
        let rms = replySamples > 0 ? 10 * log10(replySquares / Double(replySamples)) : -120
        defer {
            replyPeak = 0
            replySquares = 0
            replySamples = 0
            outPeak = 0
        }
        return (dB(replyPeak), rms, dB(outPeak))
    }

    /// `speaker, AirPods Pro` — every current output port.
    var outputPorts: String {
        AVAudioSession.sharedInstance().currentRoute.outputs
            .map { "\($0.portType.rawValue):\($0.portName)" }
            .joined(separator: ", ")
    }

    /// −50 dBFS reads 0, −6 dBFS reads 1, on a curve that spends most of
    /// its range where speech sits (−35 … −15), so a voice breathes
    /// instead of pegging.
    static func level(rms: Float) -> Float {
        guard rms > 0 else { return 0 }
        let db = 20 * log10(rms)
        let linear = min(1, max(0, (db + 50) / 44))
        return pow(linear, 1.6)
    }

    /// The system output volume for this session's route, 0…1.
    var systemVolume: Float { AVAudioSession.sharedInstance().outputVolume }

    /// `.defaultToSpeaker` is honoured on activation, but a route change
    /// (unplugging headphones) can land the output back on the earpiece.
    private func routeToSpeakerIfEarpiece() {
        let session = AVAudioSession.sharedInstance()
        if session.currentRoute.outputs.contains(where: { $0.portType == .builtInReceiver }) {
            try? session.overrideOutputAudioPort(.speaker)
        }
    }

    /// Forcing the speaker also moves the mic to the phone (a Bluetooth
    /// headset carries both or neither).
    private func applyRoutePreference() {
        let session = AVAudioSession.sharedInstance()
        if preferSpeaker {
            try? session.setCategory(.playAndRecord, mode: mode, options: [.defaultToSpeaker])
            try? session.overrideOutputAudioPort(.speaker)
        } else {
            try? session.setCategory(.playAndRecord, mode: mode, options: [.defaultToSpeaker, .allowBluetoothHFP])
            try? session.overrideOutputAudioPort(.none)
            routeToSpeakerIfEarpiece()
        }
        onRouteChange?(outputRoute)
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
        if let onInputLevel {
            var squares: Double = 0
            for i in 0..<count { let v = Float(channel[i]) / 32768; squares += Double(v * v) }
            let level = Self.level(rms: Float(squares / Double(max(count, 1))).squareRoot())
            DispatchQueue.main.async { onInputLevel(level) }
        }
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
        // The bar sits a little above the echo the canceller leaves, never
        // above a normal voice: the server has its own gate and hears
        // `client.speaking`, so this one only has to catch the obvious.
        let threshold = min(Self.maxSpeechRMS, max(Self.minSpeechRMS, echoFloor * 1.6))
        gateLog(rms: rms, threshold: threshold)
        if now < speechHoldUntil { return false }
        if rms > threshold {
            speechHoldUntil = now.addingTimeInterval(0.6)
            return false
        }
        // Track the echo we are hearing so a louder reply raises the bar.
        echoFloor = echoFloor == 0 ? rms : echoFloor * 0.9 + rms * 0.1
        return true
    }

    /// About -27 dBFS in Int16 units: quieter than speech at arm's length,
    /// louder than the residue the canceller leaves behind.
    private static let minSpeechRMS: Double = 1200
    /// About -18 dBFS: a voice a step away from the phone always clears it.
    private static let maxSpeechRMS: Double = 4000

    private var lastGateLog: Date = .distantPast

    /// Once a second while the gate is active, for tuning on a device.
    private func gateLog(rms: Double, threshold: Double) {
        #if DEBUG
        let now = Date()
        guard now.timeIntervalSince(lastGateLog) > 1 else { return }
        lastGateLog = now
        print("gate rms=\(Int(rms)) floor=\(Int(echoFloor)) bar=\(Int(threshold))")
        #endif
    }

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
