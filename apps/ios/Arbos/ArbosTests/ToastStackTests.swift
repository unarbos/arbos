import XCTest
@testable import Arbos

/// Jacob 2026-10-27 ([feedback toast-stack-ios-2026-10-27]; the Mac's `me_picker_tests` twin):
/// eight identical "Couldn't load your secrets. You're offline." toasts down the right side —
/// "this must never happen". One slot under the chat's header pill; the same failure again is a
/// ×N on its note, never a second note; at most three visible, the rest fold into a count.
@MainActor
final class ToastStackTests: XCTestCase {
    private func offline(_ verb: String) -> UserError {
        UserError.of(verb, .offline(deviceOffline: true), "dns error: failed to lookup address information")
    }

    func testTheSameFailureEightTimesIsOneToastWithACount() {
        let model = AppModel()
        for _ in 0..<8 { model.fail(offline("load your secrets")) }
        XCTAssertEqual(model.toasts.count, 1, "one toast, not eight")
        XCTAssertEqual(model.toasts.first?.count, 8)
        XCTAssertEqual(model.toasts.first?.text, "Couldn't load your secrets. You're offline.")
        // An info notice coalesces the same way.
        model.notify("Forwarded to Studio"); model.notify("Forwarded to Studio")
        XCTAssertEqual(model.toasts.count, 2)
        XCTAssertEqual(model.toasts.last?.count, 2)
    }

    func testAtMostThreeAreVisibleAndTheRestFoldIntoACount() {
        let model = AppModel()
        for verb in ["load your secrets", "load the voices", "download report.pdf", "save the secret", "load the devices"] {
            model.fail(offline(verb))
        }
        let stack = model.toastStack
        XCTAssertEqual(stack.visible.count, Toast.visibleMax)
        XCTAssertEqual(stack.folded, 2)
        XCTAssertEqual(stack.visible.last?.text, "Couldn't load the devices. You're offline.", "newest at the bottom")
        XCTAssertEqual(stack.visible.first?.text, "Couldn't download report.pdf. You're offline.")
        model.dismissAllToasts()
        XCTAssertTrue(model.toasts.isEmpty)
        XCTAssertEqual(model.toastStack.folded, 0)
    }

    func testAHoldHasACapUnlessDetailsAreOpen() {
        let model = AppModel()
        model.fail(offline("load your secrets"))
        let id = model.toasts[0].id
        model.holdToast(id, held: true)
        XCTAssertNotNil(model.toasts[0].heldSince)
        model.holdToast(id, held: false)
        XCTAssertNil(model.toasts[0].heldSince)
        model.setToastDetails(id, open: true)
        XCTAssertTrue(model.toasts[0].held)
        XCTAssertNotNil(model.toasts[0].heldSince)
        XCTAssertEqual(Toast.holdMax, 60)
    }

    // MARK: - Structure

    private var sourceRoot: URL {
        URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().appendingPathComponent("Arbos")
    }

    private func source(_ rel: String) throws -> String? {
        let url = sourceRoot.appendingPathComponent(rel)
        guard FileManager.default.fileExists(atPath: url.path) else { return nil }
        return try String(contentsOf: url, encoding: .utf8)
    }

    /// The slot hangs under the chat header's pill; the root's host steps aside while a chat is up.
    func testTheSlotIsUnderTheChatHeaderPill() throws {
        guard let chat = try source("UI/ChatView.swift"), let app = try source("App/ArbosApp.swift"),
              let note = try source("UI/ErrorNote.swift") else {
            throw XCTSkip("sources not beside the test bundle (CI without the tree)")
        }
        let pill = chat.range(of: "linkPill.padding(.top, -5)")
        let slot = chat.range(of: "ToastHost().environmentObject(model)")
        let chatNote = chat.range(of: "chatNote.padding(.horizontal, 16)")
        XCTAssertNotNil(pill); XCTAssertNotNil(slot); XCTAssertNotNil(chatNote)
        if let pill, let slot, let chatNote {
            XCTAssertTrue(pill.lowerBound < slot.lowerBound && slot.lowerBound < chatNote.lowerBound,
                          "the slot sits right under the pill, above the chat-level note")
        }
        XCTAssertTrue(app.contains("if !model.chatIsUp {"), "the root host only without a chat")
        XCTAssertTrue(note.contains("\"toast-folded-close\""), "the N more line's ×")
        XCTAssertTrue(note.contains("count: toast.count"), "the ×N reaches the note")
    }
}
