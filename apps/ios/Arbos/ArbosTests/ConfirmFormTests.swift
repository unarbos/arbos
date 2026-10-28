import XCTest
@testable import Arbos

/// The confirm's form ([feedback confirm-alerts-ios-2026-10-27]): a centred alert — title, note,
/// Cancel · action — built by `Confirm.swift`. `.confirmationDialog` stays only where it sits on
/// its own control (a row's menu), marked `// confirm: anchored`; attached at a screen's root,
/// iOS 26 floated it in the middle of the transcript over someone's messages.
final class ConfirmFormTests: XCTestCase {
    // MARK: The shape

    func testASpecIsTitleNoteActionThenCancel() {
        let spec = ConfirmSpec(title: "Delete #general?", message: "Its messages go for everyone in the server.", action: "Delete channel")
        XCTAssertEqual(spec.buttons, ["Delete channel", "Cancel"], "the action, then Cancel")
        XCTAssertTrue(spec.destructive, "a confirm guards a removal unless it says otherwise")
        XCTAssertEqual(spec.cancel, "Cancel")
    }

    func testAPlainConfirmIsNotRed() {
        XCTAssertFalse(BlockedState.unblockConfirm(handle: "sam").destructive)
        XCTAssertFalse(ConfirmSpec(title: "Share screen", action: "Choose what to share…", destructive: false).destructive)
    }

    func testTheBlockConfirmsSayWhatHappens() {
        let block = BlockRow.blockConfirm(handle: "sam")
        XCTAssertEqual(block.title, "Block @sam?")
        XCTAssertEqual(block.message, "They cannot message you or add you to chats.")
        XCTAssertEqual(block.buttons, ["Block", "Cancel"])
        XCTAssertTrue(block.destructive)
        let unblock = BlockedState.unblockConfirm(handle: "sam")
        XCTAssertEqual(unblock.title, "Unblock @sam?")
        XCTAssertEqual(unblock.buttons, ["Unblock", "Cancel"])
    }

    // MARK: The source: every confirm goes through the helper

    private var sourceRoot: URL {
        URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().appendingPathComponent("Arbos")
    }

    private func swiftFiles() throws -> [URL] {
        let fm = FileManager.default
        guard fm.fileExists(atPath: sourceRoot.path) else { return [] }
        return try ["UI", "App", "Store", "Model"].flatMap { dir -> [URL] in
            let base = sourceRoot.appendingPathComponent(dir)
            guard fm.fileExists(atPath: base.path) else { return [] }
            return try fm.contentsOfDirectory(at: base, includingPropertiesForKeys: nil)
                .filter { $0.pathExtension == "swift" && $0.lastPathComponent != "Confirm.swift" }
        }
    }

    /// A `.confirmationDialog(` outside `Confirm.swift` must sit on its own control and say so
    /// with `// confirm: anchored` on the same line; everything else is a `.confirm(…)` alert.
    func testNoConfirmationDialogOffItsControl() throws {
        let files = try swiftFiles()
        try XCTSkipIf(files.isEmpty, "sources not beside the test bundle (CI without the tree)")
        var offenders: [String] = []
        var anchored = 0
        for file in files {
            let text = try String(contentsOf: file, encoding: .utf8)
            for (i, line) in text.components(separatedBy: "\n").enumerated() where line.contains(".confirmationDialog(") {
                if line.contains("// confirm: anchored") { anchored += 1 } else {
                    offenders.append("\(file.lastPathComponent):\(i + 1): \(line.trimmingCharacters(in: .whitespaces))")
                }
            }
        }
        XCTAssertTrue(offenders.isEmpty, "A confirm is a centred alert (`.confirm(ConfirmSpec(…), isPresented:)`, Confirm.swift). Keep `.confirmationDialog` only on the control it belongs to, marked `// confirm: anchored`:\n" + offenders.joined(separator: "\n"))
        XCTAssertEqual(anchored, 3, "the three row menus (mute durations, a member's actions, a connector's Disconnect) — update this count when one moves")
    }

    /// No hand-built two-button alert either: an `.alert(` whose buttons are an action and a
    /// Cancel is the helper's job. Alerts with a text field (Rename, Topic, Replace token) and
    /// single-OK notices are not confirms and stay.
    func testNoHandBuiltConfirmAlert() throws {
        let files = try swiftFiles()
        try XCTSkipIf(files.isEmpty, "sources not beside the test bundle (CI without the tree)")
        var offenders: [String] = []
        for file in files {
            let text = try String(contentsOf: file, encoding: .utf8)
            let lines = text.components(separatedBy: "\n")
            for (i, line) in lines.enumerated() where line.contains(".alert(") && !line.contains("// confirm: ") {
                // Look at the alert's body (up to its `message:` or the next modifier).
                let body = lines[i..<min(lines.count, i + 14)].joined(separator: "\n")
                let hasField = body.contains("TextField(") || body.contains("SecureField(")
                let hasCancel = body.contains("role: .cancel")
                let hasDestructive = body.contains("role: .destructive")
                if !hasField && hasCancel && hasDestructive {
                    offenders.append("\(file.lastPathComponent):\(i + 1): \(line.trimmingCharacters(in: .whitespaces))")
                }
            }
        }
        XCTAssertTrue(offenders.isEmpty, "A destructive action with a Cancel is a confirm: use `.confirm(ConfirmSpec(…), isPresented:)` so every confirm reads the same:\n" + offenders.joined(separator: "\n"))
    }
}
