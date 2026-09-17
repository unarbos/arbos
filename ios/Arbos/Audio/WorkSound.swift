import AVFoundation
import Foundation

/// The working sound for a call: a soft tick every 2.5 s while the kernel
/// is at work on the caller's behalf, and one tick the moment a command
/// starts. Same tick as the desktop's "ticks" sound (`desktop/src/voice_ws.rs`:
/// 880 Hz, 45 ms, damped), so both surfaces sound like one product.
///
/// It is driven by the gateway's `agent.activity` frames and by nothing
/// else: no timer, no guess. It plays through its own player, not the reply
/// path — reply playback is what the phone reports as `client.speaking` and
/// what a barge-in stops, and the working sound must be neither.
final class WorkSound {
    static let sampleRate = 24_000
    static let tickHz = 880.0
    static let tickMs = 45
    static let tickLevel: Float = 0.05
    static let periodS = 2.5

    private let player: AVAudioPlayer?
    /// The gateway says work is running.
    private(set) var on = false
    /// A voice (the reply, or the caller) has the floor: the sound waits.
    private(set) var ducked = false

    init() {
        player = try? AVAudioPlayer(data: Self.loopWAV())
        player?.numberOfLoops = -1
        player?.volume = 1
        player?.prepareToPlay()
    }

    func set(on: Bool) {
        guard on != self.on else { return }
        self.on = on
        if on { player?.currentTime = 0 }
        apply()
    }

    func duck(_ ducked: Bool) {
        guard ducked != self.ducked else { return }
        self.ducked = ducked
        apply()
    }

    /// A command just started: one tick now, not at the next period.
    func tick() {
        guard on, !ducked, let player else { return }
        player.currentTime = 0
        if !player.isPlaying { player.play() }
    }

    private func apply() {
        guard let player else { return }
        if on, !ducked {
            if !player.isPlaying { player.play() }
        } else if player.isPlaying {
            player.pause()
        }
    }

    /// One period of the loop as a WAV file in memory: the tick, then
    /// silence to the period. Looping it forever gives the periodic tick.
    private static func loopWAV() -> Data {
        let total = Int(Double(sampleRate) * periodS)
        let tickLength = sampleRate * tickMs / 1000
        var samples = [Int16](repeating: 0, count: total)
        for i in 0..<tickLength {
            let x = Double(i) / Double(sampleRate)
            let envelope = exp(-x * 60)
            let value = sin(2 * .pi * tickHz * x) * envelope * Double(tickLevel)
            samples[i] = Int16(max(-1, min(1, value)) * 32767)
        }
        let pcm = samples.withUnsafeBufferPointer { Data(buffer: $0) }

        var wav = Data()
        func put<T: FixedWidthInteger>(_ value: T) {
            withUnsafeBytes(of: value.littleEndian) { wav.append(contentsOf: $0) }
        }
        wav.append(contentsOf: Array("RIFF".utf8))
        put(UInt32(36 + pcm.count))
        wav.append(contentsOf: Array("WAVE".utf8))
        wav.append(contentsOf: Array("fmt ".utf8))
        put(UInt32(16))
        put(UInt16(1))                       // PCM
        put(UInt16(1))                       // mono
        put(UInt32(sampleRate))
        put(UInt32(sampleRate * 2))          // bytes per second
        put(UInt16(2))                       // block align
        put(UInt16(16))                      // bits per sample
        wav.append(contentsOf: Array("data".utf8))
        put(UInt32(pcm.count))
        wav.append(pcm)
        return wav
    }
}
