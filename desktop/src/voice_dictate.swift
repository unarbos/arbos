// arbos voice dictation helper.
//
// Captures the host machine's microphone and transcribes it on-device with
// Apple's Speech framework, then writes the transcript to a file. It is a tiny
// app bundle launched by arbos via `open` (so it is its own TCC-responsible
// process and macOS reads its Info.plist for the mic / speech prompts). Control
// is by files, since an `open`-launched app has no controlling terminal:
//
//   --out  <path>   live partials are written here as speech arrives;
//                   <path>.done marks completion, <path>.err a failure
//   --stop <path>   arbos creates this file to end the recording
//
// Nothing blocks the main thread: authorization, capture, and the stop-file
// watch are all driven off the run loop, so a stop that lands while the macOS
// permission prompt is still up is handled cleanly.

import AVFoundation
import Foundation
import Speech

func argValue(_ name: String) -> String? {
    let args = CommandLine.arguments
    guard let i = args.firstIndex(of: name), i + 1 < args.count else { return nil }
    return args[i + 1]
}

guard let outPath = argValue("--out"), let stopPath = argValue("--stop") else {
    FileHandle.standardError.write(Data("usage: dictate --out <path> --stop <path>\n".utf8))
    exit(2)
}

let lock = NSLock()

// A trace of every result the recognizer hands back, for diagnosing takes
// that lose words. One file, overwritten per take, beside the helper.
let traceURL = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask).first?
    .appendingPathComponent("arbos/voice/last-take.log")
let traceStart = Date()
if let traceURL = traceURL {
    try? "".write(to: traceURL, atomically: true, encoding: .utf8)
}
func trace(_ line: String) {
    guard let traceURL = traceURL, let handle = try? FileHandle(forWritingTo: traceURL) else { return }
    defer { try? handle.close() }
    handle.seekToEndOfFile()
    let stamp = String(format: "%7.2f ", Date().timeIntervalSince(traceStart))
    handle.write(Data((stamp + line + "\n").utf8))
}
// Segments the recognizer has closed, joined with spaces. Apple's recognizer
// — on-device especially — ends an utterance at a pause and reports it final;
// the words then belong here, and a fresh request takes over the microphone so
// the take reads as one sentence rather than whichever segment came last.
var committed = ""
// The segment still being recognized, and when it last changed.
var live = ""
var liveAt = Date()
var finished = false
var stopping = false

// Held for the process lifetime so the OS does not tear capture down.
var engine: AVAudioEngine?
var recognizer: SFSpeechRecognizer?
var request: SFSpeechAudioBufferRecognitionRequest?
var task: SFSpeechRecognitionTask?

// Words as the recognizer means them, for comparing two partials: lowercase,
// punctuation off.
func words(_ s: String) -> [String] {
    s.lowercased()
        .components(separatedBy: CharacterSet.alphanumerics.inverted)
        .filter { !$0.isEmpty }
}

// Whether `next` is a revision of `prev` — the recognizer redrawing the same
// utterance as more audio lands — rather than a fresh utterance. A revision
// keeps the opening word; the recognizer's restart after a pause does not.
func continues(_ prev: String, _ next: String) -> Bool {
    let a = words(prev)
    let b = words(next)
    if a.isEmpty || b.isEmpty { return true }
    if a.count <= 1 { return true }
    return a[0] == b[0] || (a.count >= 2 && b.count >= 2 && a[1] == b[1])
}

// The partial has stood this long before a fresh-looking one is taken as a new
// utterance rather than a rewrite.
let settleAfter: TimeInterval = 0.6

func join(_ a: String, _ b: String) -> String {
    let a = a.trimmingCharacters(in: .whitespaces)
    let b = b.trimmingCharacters(in: .whitespaces)
    if a.isEmpty { return b }
    if b.isEmpty { return a }
    return a + " " + b
}

// finish writes the result (or an error) exactly once and exits. arbos waits
// for the .done marker, then reads the transcript (or .err).
func finish(text: String? = nil, error: String? = nil) {
    lock.lock()
    if finished {
        lock.unlock()
        return
    }
    finished = true
    lock.unlock()

    trace("finish text=[\(text ?? "")] error=[\(error ?? "")]")
    if let error = error {
        try? error.write(toFile: outPath + ".err", atomically: true, encoding: .utf8)
    } else {
        writeTranscript(text ?? "")
    }
    FileManager.default.createFile(atPath: outPath + ".done", contents: Data())
    exit(error == nil ? 0 : 1)
}

func writeTranscript(_ text: String) {
    try? text.write(toFile: outPath, atomically: true, encoding: .utf8)
}

func currentTranscript() -> String {
    lock.lock()
    defer { lock.unlock() }
    return join(committed, live)
}

// stopAndFlush ends capture and gives the recognizer a beat to deliver its
// final result. Safe to call before capture even started (an early stop).
func stopAndFlush() {
    trace("stop requested")
    lock.lock()
    stopping = true
    let eng = engine
    let req = request
    lock.unlock()
    if eng == nil && req == nil {
        finish(text: "")
        return
    }
    eng?.stop()
    eng?.inputNode.removeTap(onBus: 0)
    req?.endAudio()
    DispatchQueue.main.asyncAfter(deadline: .now() + 2.0) {
        finish(text: currentTranscript())
    }
}

// Watch for arbos's stop file off the run loop.
let stopTimer = DispatchSource.makeTimerSource(queue: .main)
stopTimer.schedule(deadline: .now() + 0.2, repeating: 0.2)
stopTimer.setEventHandler {
    if FileManager.default.fileExists(atPath: stopPath) {
        stopTimer.cancel()
        stopAndFlush()
    }
}
stopTimer.resume()

// A hard ceiling so a forgotten recording can never run forever.
DispatchQueue.main.asyncAfter(deadline: .now() + 300) {
    stopAndFlush()
}

// One recognition request over the shared microphone. Called at capture start
// and again each time the recognizer closes a segment while the user is still
// holding the key.
func startRequest() {
    guard let rec = recognizer else { return }
    let req = SFSpeechAudioBufferRecognitionRequest()
    req.shouldReportPartialResults = true
    // Keep it on the machine — no audio leaves the host.
    if rec.supportsOnDeviceRecognition {
        req.requiresOnDeviceRecognition = true
    }

    var closed = false
    let recognition = rec.recognitionTask(with: req) { result, error in
        lock.lock()
        // A result from an earlier request, after its segment was committed.
        if request !== req || closed {
            lock.unlock()
            return
        }
        var isFinal = false
        if let result = result {
            let spoken = result.bestTranscription.formattedString
            isFinal = result.isFinal
            trace("result final=\(isFinal) live=[\(live)] spoken=[\(spoken)]")
            let settled = Date().timeIntervalSince(liveAt) >= settleAfter
            if spoken.trimmingCharacters(in: .whitespaces).isEmpty {
                // The final after endAudio often carries nothing; the last
                // partial is the transcript then.
            } else if !live.isEmpty && !continues(live, spoken) && (settled || isFinal) {
                // The recognizer began a new utterance without closing the old
                // one. Keep the old.
                committed = join(committed, live)
                live = spoken
                liveAt = Date()
            } else {
                live = spoken
                liveAt = Date()
            }
        }
        let segmentOver = isFinal || error != nil
        if let error = error { trace("error \(error.localizedDescription)") }
        if segmentOver {
            trace("segment over; committed=[\(join(committed, live))] stopping=\(stopping)")
            closed = true
            committed = join(committed, live)
            live = ""
            liveAt = Date()
        }
        let text = join(committed, live)
        let stop = stopping
        lock.unlock()
        writeTranscript(text)
        if segmentOver {
            if stop {
                finish(text: text)
            } else {
                // The take goes on: a new request picks up the next words.
                DispatchQueue.main.async { startRequest() }
            }
        }
    }

    lock.lock()
    request = req
    task = recognition
    lock.unlock()
    trace("request started")
}

func beginCapture() {
    guard let rec = SFSpeechRecognizer(locale: Locale(identifier: "en-US")) else {
        finish(error: "no speech recognizer available for en-US")
        return
    }
    guard rec.isAvailable else {
        finish(error: "speech recognizer is currently unavailable")
        return
    }

    let eng = AVAudioEngine()
    let input = eng.inputNode
    let format = input.outputFormat(forBus: 0)

    // The tap feeds whichever request is current, so a segment change never
    // drops audio.
    input.installTap(onBus: 0, bufferSize: 1024, format: format) { buffer, _ in
        lock.lock()
        let req = request
        lock.unlock()
        req?.append(buffer)
    }
    eng.prepare()
    do {
        try eng.start()
    } catch {
        finish(error: "microphone capture failed: \(error.localizedDescription)")
        return
    }

    lock.lock()
    engine = eng
    recognizer = rec
    lock.unlock()
    startRequest()
}

// Speech-recognition authorization. The callback starts capture; the main run
// loop stays free for the stop-file watch.
SFSpeechRecognizer.requestAuthorization { status in
    guard status == .authorized else {
        finish(
            error:
                "speech recognition not authorized; enable arbos voice under System Settings › Privacy & Security › Speech Recognition")
        return
    }
    beginCapture()
}

dispatchMain()
