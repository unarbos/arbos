import Foundation
import XCTest
@testable import Arbos

/// No coloured status dot anywhere in the app (Jacob 2026-10-27, [feedback
/// neon-dots-ios-2026-10-27]: "on a matter of principle never use colored dots").
/// A status dot is a small `Circle()` (frame ≤ 14 pt or `Theme.unreadDot`) filled
/// with a status colour — `theme.unread / online / warn / danger / accent`, or a
/// machine-state colour. Activity → `WorkingSpinner`, unread → `UnreadBadge`,
/// a fault → `FaultGlyph`, at rest → nothing (`StatusMarks.swift`). A disc whose
/// colour is data or a surface (a pager's page dots, a chart swatch) carries
/// `// status-dot: exception <why>` on its `.fill(` line. Twin of the Mac's
/// `marks::lint_tests`.
final class StatusDotLintTests: XCTestCase {
    static let statusFill = try! NSRegularExpression(
        pattern: #"\.fill\((theme\.(unread|online|warn|danger|accent)\b|machineDotColor\(|dot\))"#)
    static let smallFrame = try! NSRegularExpression(
        pattern: #"\.frame\(width:\s*(Theme\.unreadDot|(1[0-4]|[1-9])(\.\d+)?)\b"#)

    private var sourceRoot: URL {
        URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().appendingPathComponent("Arbos")
    }

    private func swiftFiles() throws -> [URL] {
        let fm = FileManager.default
        guard fm.fileExists(atPath: sourceRoot.path) else { return [] }
        return try ["UI", "App", "Store"].flatMap { dir -> [URL] in
            let base = sourceRoot.appendingPathComponent(dir)
            return try fm.contentsOfDirectory(at: base, includingPropertiesForKeys: nil)
                .filter { $0.pathExtension == "swift" }
        }
    }

    static func offenders(in text: String, file: String) -> [String] {
        let lines = text.components(separatedBy: "\n")
        var out: [String] = []
        for (i, line) in lines.enumerated() {
            let range = NSRange(line.startIndex..., in: line)
            guard line.contains("Circle()"), statusFill.firstMatch(in: line, range: range) != nil else { continue }
            if line.contains("status-dot: exception") { continue }
            let window = lines[i..<min(i + 3, lines.count)].joined(separator: "\n")
            let wr = NSRange(window.startIndex..., in: window)
            guard smallFrame.firstMatch(in: window, range: wr) != nil else { continue }
            out.append("\(file):\(i + 1): \(line.trimmingCharacters(in: .whitespaces))")
        }
        return out
    }

    func testNoColouredStatusDotIsDrawnInTheApp() throws {
        let files = try swiftFiles()
        try XCTSkipIf(files.isEmpty, "sources not beside the test bundle (CI without the tree)")
        var offenders: [String] = []
        for file in files {
            let text = try String(contentsOf: file, encoding: .utf8)
            offenders += Self.offenders(in: text, file: file.lastPathComponent)
        }
        XCTAssertTrue(offenders.isEmpty,
                      "Coloured status dots (never — use WorkingSpinner / UnreadBadge / FaultGlyph, or mark `// status-dot: exception <why>`):\n" + offenders.joined(separator: "\n"))
    }

    func testTheLintCatchesADotAndHonoursTheMarker() {
        let dot = """
            HStack {
                Circle().fill(theme.unread).frame(width: Theme.unreadDot, height: Theme.unreadDot)
            }
            """
        XCTAssertEqual(Self.offenders(in: dot, file: "X.swift").count, 1)
        let tiny = """
            Circle()
                .fill(theme.online)
                .frame(width: 8, height: 8)
            """
        XCTAssertEqual(Self.offenders(in: "Circle().fill(theme.online)\n    .frame(width: 8, height: 8)", file: "X.swift").count, 1)
        _ = tiny
        let big = "Circle().fill(theme.danger).frame(width: 64, height: 64)"
        XCTAssertTrue(Self.offenders(in: big, file: "X.swift").isEmpty, "a 64 pt button is not a dot")
        let swatch = "Circle().fill(theme.accent) // status-dot: exception legend swatch\n    .frame(width: 8, height: 8)"
        XCTAssertTrue(Self.offenders(in: swatch, file: "X.swift").isEmpty)
        let surface = "Circle().fill(theme.card).frame(width: 8, height: 8)"
        XCTAssertTrue(Self.offenders(in: surface, file: "X.swift").isEmpty, "a surface colour is not a status")
    }

    func testTheMachineMarkIsSpinnerFaultOrNothing() {
        XCTAssertEqual(machineMark(.waking), .working)
        XCTAssertEqual(machineMark(.error), .fault)
        for s in [BoxState.awake, .hibernatedLight, .hibernatedDeep, .stoppedByUser] {
            XCTAssertNil(machineMark(s), "\(s) shows no mark")
        }
    }
}
