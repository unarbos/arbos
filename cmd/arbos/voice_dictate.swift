// arbos voice dictation helper.
//
// Captures the host machine's microphone and transcribes it on-device with
// Apple's Speech framework, then writes the transcript to a file. It is a tiny
// app bundle launched by arbos via `open` (so it is its own TCC-responsible
// process and macOS reads its Info.plist for the mic / speech prompts). Control
// is by files, since an `open`-launched app has no controlling terminal:
//
//   --out  <path>   transcript is written here; <path>.done marks completion,
//                   <path>.err carries a failure message
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
var latest = ""
var finished = false

// Held for the process lifetime so the OS does not tear capture down.
var engine: AVAudioEngine?
var request: SFSpeechAudioBufferRecognitionRequest?
var task: SFSpeechRecognitionTask?

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

    if let error = error {
        try? error.write(toFile: outPath + ".err", atomically: true, encoding: .utf8)
    } else {
        try? (text ?? "").write(toFile: outPath, atomically: true, encoding: .utf8)
    }
    FileManager.default.createFile(atPath: outPath + ".done", contents: Data())
    exit(error == nil ? 0 : 1)
}

func currentTranscript() -> String {
    lock.lock()
    defer { lock.unlock() }
    return latest
}

// stopAndFlush ends capture and gives the recognizer a beat to deliver its
// final result. Safe to call before capture even started (an early stop).
func stopAndFlush() {
    lock.lock()
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

func beginCapture() {
    guard let recognizer = SFSpeechRecognizer(locale: Locale(identifier: "en-US")) else {
        finish(error: "no speech recognizer available for en-US")
        return
    }
    guard recognizer.isAvailable else {
        finish(error: "speech recognizer is currently unavailable")
        return
    }

    let req = SFSpeechAudioBufferRecognitionRequest()
    req.shouldReportPartialResults = true
    // Keep it on the machine — no audio leaves the host.
    if recognizer.supportsOnDeviceRecognition {
        req.requiresOnDeviceRecognition = true
    }

    let eng = AVAudioEngine()
    let input = eng.inputNode
    let format = input.outputFormat(forBus: 0)

    let recognition = recognizer.recognitionTask(with: req) { result, error in
        if let result = result {
            lock.lock()
            latest = result.bestTranscription.formattedString
            let isFinal = result.isFinal
            lock.unlock()
            if isFinal { finish(text: result.bestTranscription.formattedString) }
        }
        if error != nil { finish(text: currentTranscript()) }
    }

    input.installTap(onBus: 0, bufferSize: 1024, format: format) { buffer, _ in
        req.append(buffer)
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
    request = req
    task = recognition
    lock.unlock()
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
