import XCTest
@testable import Arbos

/// Drift guard for the one error element ([feedback error-messages-ios-2026-10-26]; the Mac
/// twin is `error_note::error_ui_lint`): no raw error string may reach a notice, and no ad-hoc
/// red failure text may land outside `UI/ErrorNote.swift`. A line that must stay carries
/// `// error-ui: exception <case>` on it or on the line above.
///
/// - raw strings: `Notice(text: "…\(error.localizedDescription)")`, `notify("…\(error…)")`,
///   `Notice(text: … localizedDescription …)` and `Text(error.localizedDescription)`;
/// - red text: `.foregroundStyle(theme.danger)` / `.foregroundStyle(.red)` on a `Text` outside
///   the element (a destructive action label or a status mark says so with the marker);
/// - structure: every surface's note is still drawn through `ErrorNote(id: "<surface>"` in its
///   file, and no `ErrorNote(` call uses an id outside the list.
final class ErrorUILintTests: XCTestCase {
    static let marker = try! NSRegularExpression(pattern: #"// error-ui: exception (\S+)"#)

    private var sourceRoot: URL {
        URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().appendingPathComponent("Arbos")
    }

    private func swiftFiles(_ dirs: [String]) throws -> [URL] {
        let fm = FileManager.default
        guard fm.fileExists(atPath: sourceRoot.path) else { return [] }
        return try dirs.flatMap { dir -> [URL] in
            let base = sourceRoot.appendingPathComponent(dir)
            return try fm.contentsOfDirectory(at: base, includingPropertiesForKeys: nil)
                .filter { $0.pathExtension == "swift" }
        }.sorted { $0.path < $1.path }
    }

    private func excepted(_ lines: [String], _ i: Int) -> Bool {
        let here = NSRange(lines[i].startIndex..., in: lines[i])
        if Self.marker.firstMatch(in: lines[i], range: here) != nil { return true }
        guard i > 0 else { return false }
        let above = lines[i - 1].trimmingCharacters(in: .whitespaces)
        return above.hasPrefix("// error-ui: exception")
    }

    func testNoRawErrorStringsReachANoticeOrTheScreen() throws {
        let files = try swiftFiles(["UI", "App", "Store", "Sync", "Model"])
        try XCTSkipIf(files.isEmpty, "sources not beside the test bundle (CI without the tree)")
        let shapes: [(String, NSRegularExpression)] = [
            ("raw error in a notice", try NSRegularExpression(pattern: #"(Notice\(text:|notify\()[^\n]*localizedDescription"#)),
            ("raw error in a notice", try NSRegularExpression(pattern: #"(Notice\(text:|notify\()[^\n]*\\\((error|err|e)\)"#)),
            ("raw error on screen", try NSRegularExpression(pattern: #"Text\((error|err|e)\.localizedDescription\)"#)),
            ("raw error on screen", try NSRegularExpression(pattern: #"Text\("[^"]*\\\((error|err)\.localizedDescription\)"#)),
        ]
        var offenders: [String] = []
        for file in files where file.lastPathComponent != "UserError.swift" {
            let lines = try String(contentsOf: file, encoding: .utf8).components(separatedBy: "\n")
            for (i, line) in lines.enumerated() {
                let range = NSRange(line.startIndex..., in: line)
                guard let hit = shapes.first(where: { $0.1.firstMatch(in: line, range: range) != nil }) else { continue }
                if excepted(lines, i) { continue }
                offenders.append("\(file.lastPathComponent):\(i + 1): \(hit.0) — \(line.trimmingCharacters(in: .whitespaces))")
            }
        }
        XCTAssertTrue(offenders.isEmpty, "raw error strings (route them through UserError / AppModel.fail, or mark the line `// error-ui: exception <case>`):\n" + offenders.joined(separator: "\n"))
    }

    func testNoAdHocRedFailureTextOutsideErrorNote() throws {
        let files = try swiftFiles(["UI", "App"])
        try XCTSkipIf(files.isEmpty, "sources not beside the test bundle (CI without the tree)")
        let red = try NSRegularExpression(pattern: #"\.foregroundStyle\((theme\.danger|\.red)\)"#)
        var offenders: [String] = []
        for file in files where file.lastPathComponent != "ErrorNote.swift" {
            let lines = try String(contentsOf: file, encoding: .utf8).components(separatedBy: "\n")
            for (i, line) in lines.enumerated() {
                let range = NSRange(line.startIndex..., in: line)
                guard red.firstMatch(in: line, range: range) != nil else { continue }
                // Only text is in scope: a red glyph on a destructive button or a status dot
                // is not a message. A `Text(` on this line or the two above makes it text.
                let window = lines[max(0, i - 2)...i].joined(separator: " ")
                guard window.contains("Text(") || window.contains("Label(") else { continue }
                if excepted(lines, i) { continue }
                offenders.append("\(file.lastPathComponent):\(i + 1): \(line.trimmingCharacters(in: .whitespaces))")
            }
        }
        XCTAssertTrue(offenders.isEmpty, "ad-hoc red text (use ErrorNote, or mark a destructive action / status mark `// error-ui: exception <case>`):\n" + offenders.joined(separator: "\n"))
    }

    /// One note per surface, in the file the rule names (`docs/mac-design-rules.md` § Error
    /// messages). A surface that loses its note, or a new one that invents its own id, fails.
    static let surfaces: [(id: String, file: String)] = [
        ("avatar-generate-error", "AvatarPickerSheet.swift"),
        ("create-entity-error", "PeopleSheets.swift"),
        ("people-sheet-error", "PeopleSheets.swift"),
        ("new-channel-error", "ServerChannelsView.swift"),
        ("join-server-error", "ServerChannelsView.swift"),
        ("composer-error", "ChatView.swift"),
        ("delivery-error", "ChatView.swift"),
        ("machine-error", "ChatView.swift"),
        ("agent-turn-error", "Transcript.swift"),
        ("sign-in-error", "SignInView.swift"),
        ("pairing-error", "SignInView.swift"),
        ("feedback-error", "FeedbackSheet.swift"),
        ("question-card-error", "QuestionCard.swift"),
        ("connector-error", "SettingsView.swift"),
        ("devices-error", "SettingsView.swift"),
        ("privacy-error", "SettingsView.swift"),
        ("profile-error", "SettingsView.swift"),
        ("secrets-sheet-error", "SecretsViews.swift"),
        ("call-error", "CallView.swift"),
        ("voices-error", "VoicePickerView.swift"),
        ("home-registry-error", "HomeView.swift"),
        ("toast-", "ErrorNote.swift"),
    ]

    func testEverySurfaceHasItsOneNote() throws {
        let files = try swiftFiles(["UI", "App"])
        try XCTSkipIf(files.isEmpty, "sources not beside the test bundle (CI without the tree)")
        var texts: [String: String] = [:]
        for file in files { texts[file.lastPathComponent] = try String(contentsOf: file, encoding: .utf8) }
        var missing: [String] = []
        for (id, file) in Self.surfaces {
            let text = texts[file] ?? ""
            if !(text.contains("ErrorNote(") && text.contains("\"\(id)")) { missing.append("\(id) in \(file)") }
        }
        XCTAssertTrue(missing.isEmpty, "surfaces without their note: \(missing)")
        // Every ErrorNote(id: "…") names a listed surface.
        let idRegex = try NSRegularExpression(pattern: #"ErrorNote\(\s*id:\s*"([a-z\-]+)"#)
        var unknown: [String] = []
        for (name, text) in texts where name != "ErrorNote.swift" {
            let range = NSRange(text.startIndex..., in: text)
            for m in idRegex.matches(in: text, range: range) {
                let id = (text as NSString).substring(with: m.range(at: 1))
                if !Self.surfaces.contains(where: { $0.id == id }) { unknown.append("\(name): \(id)") }
            }
        }
        XCTAssertTrue(unknown.isEmpty, "ErrorNote with an id not in the surface list (add the surface to the rule and the list): \(unknown)")
        // The shared sheet note helper goes through the element too.
        XCTAssertTrue(texts["ErrorNote.swift"]?.contains("struct SheetErrorNote") == true)
    }
}
