import SwiftUI
import XCTest
@testable import Arbos

/// Every swipe action colours its own pill ([feedback hidden-chats-swipe-delete-2026-11-05]).
///
/// `ArbosApp` puts `.tint(theme.text)` on the whole scene, and `theme.text` is `.white` in dark
/// mode. A swipe action's pill takes its colour from that environment tint, and `role:
/// .destructive` only supplies a *default* red that an environment tint replaces. The glyph and
/// the title on the pill are drawn white whatever the tint is, so an untinted Delete is a white
/// trash glyph on a white pill — the blank pill Affinity reported, beside an Unarchive that
/// looked right only because it sets `.tint(theme.unread)`.
///
/// The role is therefore not enough on its own, and neither is reading the role back: what the
/// row presents is the tint, so that is what this lints. Every `Button` inside a `.swipeActions`
/// block must carry an explicit `.tint(…)`.
final class SwipeActionTintLintTests: XCTestCase {
    /// One `Button` declared inside a `.swipeActions` block.
    struct Action: CustomStringConvertible {
        let file: String
        let line: Int
        let text: String
        let tinted: Bool
        var description: String { "\(file):\(line): \(text)" }
    }

    private var sourceRoot: URL {
        // …/apps/ios/Arbos/ArbosTests/SwipeActionTintLintTests.swift → …/apps/ios/Arbos/Arbos
        URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().appendingPathComponent("Arbos")
    }

    private func swiftFiles() throws -> [URL] {
        let fm = FileManager.default
        guard fm.fileExists(atPath: sourceRoot.path) else { return [] }
        return try ["UI", "App"].flatMap { dir -> [URL] in
            let base = sourceRoot.appendingPathComponent(dir)
            return try fm.contentsOfDirectory(at: base, includingPropertiesForKeys: nil)
                .filter { $0.pathExtension == "swift" }
        }.sorted { $0.lastPathComponent < $1.lastPathComponent }
    }

    /// Every `Button` inside every `.swipeActions` block, with whether a `.tint(` is attached to
    /// it. A button owns the lines from its own `Button` down to the next one in the same block,
    /// which is where its modifiers sit; anything before the first `Button` belongs to no action.
    private func swipeActions(in file: URL) throws -> [Action] {
        let name = file.lastPathComponent
        let lines = try String(contentsOf: file, encoding: .utf8).components(separatedBy: "\n")
        var out: [Action] = []
        var i = 0
        while i < lines.count {
            guard lines[i].contains(".swipeActions") else { i += 1; continue }
            var depth = 0
            var body: [(line: Int, text: String)] = []
            var end = i
            for j in i..<lines.count {
                if j > i { body.append((line: j + 1, text: lines[j])) }
                depth += lines[j].filter { $0 == "{" }.count - lines[j].filter { $0 == "}" }.count
                end = j
                if j > i || lines[j].contains("{") { if depth <= 0 { break } }
            }
            var current: (line: Int, text: String)? = nil
            var tinted = false
            func close() {
                guard let open = current else { return }
                out.append(Action(file: name, line: open.line,
                                  text: open.text.trimmingCharacters(in: .whitespaces), tinted: tinted))
            }
            for entry in body {
                if entry.text.trimmingCharacters(in: .whitespaces).hasPrefix("Button") {
                    close()
                    current = entry
                    tinted = false
                }
                if current != nil, entry.text.contains(".tint(") { tinted = true }
            }
            close()
            i = end + 1
        }
        return out
    }

    private func allSwipeActions() throws -> [Action] {
        try swiftFiles().flatMap { try swipeActions(in: $0) }
    }

    func testEverySwipeActionSetsItsOwnTint() throws {
        let files = try swiftFiles()
        try XCTSkipIf(files.isEmpty, "sources not beside the test bundle (CI without the tree)")
        let untinted = try allSwipeActions().filter { !$0.tinted }
        XCTAssertTrue(untinted.isEmpty, """
            A swipe action takes its pill colour from the scene tint ArbosApp sets, so
            `role: .destructive` alone draws a white pill in dark mode. Give each of these its
            own `.tint(…)` — `theme.danger` for a destructive one:
            \(untinted.map(\.description).joined(separator: "\n"))
            """)
    }

    /// The guard above passes trivially if the scan finds nothing, so prove it reaches the
    /// actions this bug was about and the premise that makes the lint necessary.
    func testTheScanReachesTheActionsAndThePremiseHolds() throws {
        let files = try swiftFiles()
        try XCTSkipIf(files.isEmpty)
        let actions = try allSwipeActions()
        XCTAssertEqual(actions.filter { $0.file == "HomeView.swift" }.count, 10,
                       "Home and Hidden chats declare ten swipe actions between them: \(actions)")
        XCTAssertEqual(actions.filter { $0.text.contains("Delete") || $0.text.contains("deleteProject") || $0.text.contains("deleteOrLeave") }.count, 2,
                       "the two Hidden-chats Deletes: \(actions)")
        XCTAssertTrue(actions.contains { $0.file == "PeopleSheets.swift" && $0.text.contains("Remove") },
                      "the project-members Remove: \(actions)")
        let app = try String(contentsOf: sourceRoot.appendingPathComponent("App/ArbosApp.swift"), encoding: .utf8)
        XCTAssertTrue(app.contains(".tint(theme.text)"),
                      "the scene-wide tint is why role alone cannot colour a swipe action; if it is gone, re-check this lint")
        XCTAssertEqual(Theme.dark.text, .white, "the scene tint resolves to white in dark mode")
    }
}
