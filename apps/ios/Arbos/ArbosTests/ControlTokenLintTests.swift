import XCTest
@testable import Arbos

/// Drift guard for the iPhone control family ([feedback
/// iphone-header-composer-sizing-2026-10-26]): no view outside `UI/Theme.swift` may size
/// or outline a round button, pill or field by hand. A line that does one of these and
/// is not the family must say why with `// control-size: exception <Controls.Exception>`:
///
/// - `GlassCircle(size: <number>)` — a disc sized by a literal;
/// - `.background(Capsule()/Circle()/RoundedRectangle(…).fill(theme.field))` — a hand-built
///   field or pill (use `.controlField()`, `.controlPill()` or `ControlSurface`);
/// - `strokeBorder(theme.fieldBorder` — the old 1 pt rim (the family's rim is `ComposerRim`).
///
/// The same check runs from the shell as `apps/ios/scripts/control-size-lint.sh`.
final class ControlTokenLintTests: XCTestCase {
    static let patterns: [(name: String, regex: NSRegularExpression)] = [
        ("literal disc", try! NSRegularExpression(pattern: #"GlassCircle\(size:\s*[0-9]"#)),
        ("hand-built field", try! NSRegularExpression(pattern: #"\.background\((Capsule\(\)|Circle\(\)|RoundedRectangle\([^)]*\))\.fill\(theme\.field\)"#)),
        ("old rim", try! NSRegularExpression(pattern: #"strokeBorder\(theme\.fieldBorder"#)),
    ]
    static let marker = try! NSRegularExpression(pattern: #"// control-size: (exception|source)(?: (\w+))?"#)

    private var sourceRoot: URL {
        // …/apps/ios/Arbos/ArbosTests/ControlTokenLintTests.swift → …/apps/ios/Arbos/Arbos
        URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().appendingPathComponent("Arbos")
    }

    private func swiftFiles() throws -> [URL] {
        let fm = FileManager.default
        guard fm.fileExists(atPath: sourceRoot.path) else { return [] }
        return try ["UI", "App"].flatMap { dir -> [URL] in
            let base = sourceRoot.appendingPathComponent(dir)
            return try fm.contentsOfDirectory(at: base, includingPropertiesForKeys: nil)
                .filter { $0.pathExtension == "swift" && $0.lastPathComponent != "Theme.swift" }
        }
    }

    func testNoHandSizedControlOutsideTheTokenFile() throws {
        let files = try swiftFiles()
        try XCTSkipIf(files.isEmpty, "sources not beside the test bundle (CI without the tree)")
        var offenders: [String] = []
        var badMarkers: [String] = []
        let known = Set(Controls.Exception.allCases.map(\.rawValue))
        for file in files {
            let text = try String(contentsOf: file, encoding: .utf8)
            for (i, line) in text.components(separatedBy: "\n").enumerated() {
                let range = NSRange(line.startIndex..., in: line)
                let hit = Self.patterns.first { $0.regex.firstMatch(in: line, range: range) != nil }
                guard let hit else { continue }
                if let m = Self.marker.firstMatch(in: line, range: range) {
                    let kind = (line as NSString).substring(with: m.range(at: 1))
                    let name = m.range(at: 2).location == NSNotFound ? "" : (line as NSString).substring(with: m.range(at: 2))
                    if kind == "exception", !known.contains(name) {
                        badMarkers.append("\(file.lastPathComponent):\(i + 1): unknown exception '\(name)'")
                    }
                } else {
                    offenders.append("\(file.lastPathComponent):\(i + 1): \(hit.name) — \(line.trimmingCharacters(in: .whitespaces))")
                }
            }
        }
        XCTAssertTrue(offenders.isEmpty, "Size these through Controls / .controlField() / GlassCircle, or mark the line `// control-size: exception <case>`:\n" + offenders.joined(separator: "\n"))
        XCTAssertTrue(badMarkers.isEmpty, "Exception markers must name a Controls.Exception case:\n" + badMarkers.joined(separator: "\n"))
    }

    func testTheSourceFixtureIsReadable() throws {
        let files = try swiftFiles()
        try XCTSkipIf(files.isEmpty)
        XCTAssertTrue(files.contains { $0.lastPathComponent == "ChatView.swift" })
    }
}
