import XCTest

/// The before/after screenshot rig, driven entirely by the environment so that one committed
/// test serves every shoot: `.github/workflows/ios-capture.yml` passes a launch environment and
/// a step script through `TEST_RUNNER_*`, and never has to add a Swift file or edit
/// `Arbos.xcodeproj`. Hand shoots used to copy a throwaway class over `BackSwipeUITests.swift`
/// to get into the test target at all.
///
/// **Inert without `ARBOS_SHOT_DIR`.** `setUpWithError` skips before anything is launched, so on
/// the `swift-core-check.yml` pull-request gate — which runs the whole scheme — this class costs
/// one skipped test case and no simulator work at all. Nothing below this line runs there.
///
/// Variables (all read from the test runner, i.e. `xcodebuild … TEST_RUNNER_<name>=<value>`):
///
/// - `ARBOS_SHOT_DIR` — host directory for the PNGs. Absent = skip. The simulator shares the
///   host file system, so this is an ordinary path on the runner.
/// - `ARBOS_SHOT_TAG` — prefix for every file, e.g. `base` or `head`. The pair job matches a
///   before shot to an after shot by the name that follows it.
/// - `ARBOS_SHOT_LAUNCH_ENV` — `KEY=VALUE` items, separated by `;` or newlines, handed to the app
///   as `launchEnvironment`. This is how a shoot reaches `DebugHooks` (`ARBOS_FIXTURE=1`,
///   `ARBOS_OPEN_PEOPLE=fx-group-launch`, `ARBOS_SHOW=actions`, and the rest).
/// - `ARBOS_SHOT_SCRIPT` — steps, separated by `;` or newlines:
///
///       wait <identifier> [seconds]        wait for that accessibility identifier
///       waitlabel <text…> [seconds]        wait for an element whose label contains the text
///       sleep <seconds>                    let the screen settle
///       shot <name>                        write <tag>-<name>.png
///       tap <identifier>
///       taplabel <text…>
///       press <identifier> [seconds]       long-press (default 1.2 s)
///       presslast <id prefix> [seconds]    long-press the last fully visible match; that is,
///                                          `presslast bubble:` is the newest bubble on screen
///       type <text…>                       type into whatever has focus
///       drag <x1> <y1> <x2> <y2>           normalised window coordinates
///
///   A step whose element never appears is recorded and the script carries on, because a lookup
///   can miss while the screen is right (`message-menu-delete` did, with the menu plainly up).
///   Shots are written before the verdict, so a miss still leaves evidence; the test fails at the
///   end, with the whole list, after everything is on disk.
final class CaptureRigUITests: XCTestCase {
    /// A hosted runner is a shared three-core machine and the gate Mac runs under load, so waits
    /// are generous by default. Both are overridable per step.
    private let appear: TimeInterval = 180
    private let longPress: TimeInterval = 1.2

    private var shotDir = ""
    private var tag = "shot"
    private var script: [String] = []
    private var launchEnv: [String: String] = [:]
    private var written: [String] = []
    private var log: [String] = []
    private var problems: [String] = []

    override func setUpWithError() throws {
        continueAfterFailure = true
        let env = ProcessInfo.processInfo.environment
        guard let dir = env["ARBOS_SHOT_DIR"], !dir.isEmpty else {
            throw XCTSkip("no ARBOS_SHOT_DIR: this run asked for no screenshots")
        }
        shotDir = dir
        tag = env["ARBOS_SHOT_TAG"].flatMap { $0.isEmpty ? nil : $0 } ?? "shot"
        script = Self.items(env["ARBOS_SHOT_SCRIPT"])
        launchEnv = Self.pairs(env["ARBOS_SHOT_LAUNCH_ENV"])
    }

    /// Items separated by `;` or newlines, blanks dropped. One dispatch input holds a whole
    /// script that way, because the dispatch form's text boxes are a single line.
    private static func items(_ raw: String?) -> [String] {
        (raw ?? "")
            .split(whereSeparator: { $0 == ";" || $0.isNewline })
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .filter { !$0.isEmpty }
    }

    private static func pairs(_ raw: String?) -> [String: String] {
        var out: [String: String] = [:]
        for item in items(raw) {
            guard let eq = item.firstIndex(of: "=") else { continue }
            let key = String(item[..<eq]).trimmingCharacters(in: .whitespaces)
            if !key.isEmpty { out[key] = String(item[item.index(after: eq)...]) }
        }
        return out
    }

    /// `<argument…> [number]`: the trailing token counts as the number when it parses as one, so
    /// an argument may hold spaces (`waitlabel teaser cut 30`).
    private static func argument(_ rest: String, or fallback: Double) -> (text: String, number: Double) {
        var parts = rest.split(separator: " ").map(String.init)
        if parts.count > 1, let last = parts.last, let value = Double(last) {
            parts.removeLast()
            return (parts.joined(separator: " "), value)
        }
        return (rest, fallback)
    }

    func testCapture() {
        let app = XCUIApplication()
        app.launchEnvironment = launchEnv
        note("launch \(launchEnv.keys.sorted().joined(separator: ","))")
        app.launch()
        for step in script { perform(step, app) }
        // Never hand back an empty artifact: a script with no `shot` still proves what was on
        // screen, and so does one whose every lookup missed.
        if written.isEmpty { write("final") }
        finish()
    }

    private func perform(_ step: String, _ app: XCUIApplication) {
        let verb = String(step.split(separator: " ", maxSplits: 1).first ?? "")
        let rest = step.dropFirst(verb.count).trimmingCharacters(in: .whitespaces)
        switch verb {
        case "shot":
            write(rest.isEmpty ? "shot\(written.count + 1)" : rest)
        case "sleep":
            let seconds = Double(rest) ?? 1
            Thread.sleep(forTimeInterval: seconds)
            note("sleep \(seconds)")
        case "wait":
            let (id, timeout) = Self.argument(rest, or: appear)
            expect(element(app, id), "wait \(id)", timeout)
        case "waitlabel":
            let (text, timeout) = Self.argument(rest, or: appear)
            expect(labelled(app, text), "waitlabel \(text)", timeout)
        case "tap":
            let target = element(app, rest)
            if expect(target, "tap \(rest)", appear) { target.tap() }
        case "taplabel":
            let target = labelled(app, rest)
            if expect(target, "taplabel \(rest)", appear) { target.tap() }
        case "press":
            let (id, duration) = Self.argument(rest, or: longPress)
            let target = element(app, id)
            if expect(target, "press \(id)", appear) { target.press(forDuration: duration) }
        case "presslast":
            let (prefix, duration) = Self.argument(rest, or: longPress)
            if let target = lastVisible(app, prefix: prefix) {
                target.press(forDuration: duration)
                note("presslast \(prefix) → \(target.identifier)")
            } else {
                fault("presslast \(prefix): nothing fully visible with that identifier")
            }
        case "type":
            app.typeText(rest)
            note("type \(rest.count) characters")
        case "drag":
            drag(app, rest)
        default:
            fault("unknown step \"\(step)\"")
        }
    }

    private func element(_ app: XCUIApplication, _ id: String) -> XCUIElement {
        app.descendants(matching: .any)[id]
    }

    private func labelled(_ app: XCUIApplication, _ text: String) -> XCUIElement {
        app.descendants(matching: .any).matching(NSPredicate(format: "label CONTAINS %@", text)).firstMatch
    }

    /// The last match that is wholly between the header and the composer — `presslast bubble:`
    /// is the newest bubble on screen, which is what a long-press shoot almost always wants.
    private func lastVisible(_ app: XCUIApplication, prefix: String) -> XCUIElement? {
        let window = app.windows.firstMatch.frame
        let matches = app.descendants(matching: .any)
            .matching(NSPredicate(format: "identifier BEGINSWITH %@", prefix))
        return matches.allElementsBoundByIndex.last {
            $0.exists && $0.isHittable && $0.frame.minY > 120 && $0.frame.maxY < window.maxY - 120
        }
    }

    private func drag(_ app: XCUIApplication, _ rest: String) {
        let n = rest.split(separator: " ").compactMap { Double($0) }
        guard n.count == 4 else {
            fault("drag needs four numbers, got \"\(rest)\"")
            return
        }
        let window = app.windows.firstMatch
        window.coordinate(withNormalizedOffset: CGVector(dx: n[0], dy: n[1]))
            .press(forDuration: 0.05,
                   thenDragTo: window.coordinate(withNormalizedOffset: CGVector(dx: n[2], dy: n[3])))
        note("drag \(rest)")
    }

    @discardableResult
    private func expect(_ target: XCUIElement, _ what: String, _ timeout: TimeInterval) -> Bool {
        if target.waitForExistence(timeout: timeout) {
            note(what)
            return true
        }
        fault("\(what): never appeared within \(Int(timeout)) s")
        return false
    }

    private func write(_ name: String) {
        let screenshot = XCUIScreen.main.screenshot()
        let file = "\(tag)-\(name).png"
        // The result bundle too: a shot survives even when the workspace does not.
        let attachment = XCTAttachment(screenshot: screenshot)
        attachment.name = file
        attachment.lifetime = .keepAlways
        add(attachment)
        try? FileManager.default.createDirectory(atPath: shotDir, withIntermediateDirectories: true)
        let url = URL(fileURLWithPath: shotDir).appendingPathComponent(file)
        do {
            try screenshot.pngRepresentation.write(to: url)
            written.append(file)
            note("shot \(file)")
        } catch {
            fault("could not write \(file): \(error.localizedDescription)")
        }
    }

    private func note(_ line: String) {
        log.append("ok   \(line)")
        NSLog("ARBOS_CAPTURE ok %@", line)
    }

    private func fault(_ line: String) {
        log.append("MISS \(line)")
        problems.append(line)
        NSLog("ARBOS_CAPTURE MISS %@", line)
    }

    /// Everything is on disk by now, including the shots of steps that missed. Only then does
    /// the verdict land, so a red capture still hands back the pictures that explain why.
    private func finish() {
        let report = (["# capture \(tag)"] + log + ["", "wrote: \(written.joined(separator: " "))"])
            .joined(separator: "\n") + "\n"
        let url = URL(fileURLWithPath: shotDir).appendingPathComponent("steps-\(tag).log")
        try? report.write(to: url, atomically: true, encoding: .utf8)
        if !problems.isEmpty {
            XCTFail("\(problems.count) step(s) missed:\n" + problems.joined(separator: "\n"))
        }
    }
}
