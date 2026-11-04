import XCTest

/// The empty toast slot is not in the accessibility tree, so the rows under it can be reached.
///
/// `ToastHost` is an overlay with `.frame(maxWidth: .infinity, maxHeight: .infinity)`. While it
/// holds no toasts it holds no children either, and a container with no children has no child
/// frames to union, so SwiftUI publishes the element at its own layout box: one element covering
/// the content area, over every row. Measured on Home with nothing else happening, the slot came
/// out at `(0, 62, 402, 778)` on a 402×874 screen and `home-row:` reported `isHittable == false`
/// with a correct frame of `(0, 420, 402, 80)`.
///
/// Two things that are **not** the defect, both measured rather than assumed. It is not a
/// touch-delivery bug: the same row takes a coordinate tap and opens its chat, because the
/// expanded container has no background and no `contentShape`, so there is nothing in it to hit.
/// And it is not caused by a toast being up — with a toast showing, the slot publishes its
/// content box (`(16, 70, 370, 90)`) and the row is hittable. The result is the inverse of the
/// intuition: the rows were reachable while a toast was up and unreachable when the layer was
/// empty, which is to say almost always.
///
/// The cost was real and not confined to tests: VoiceOver reads the same tree, so an
/// assistive-technology user met a full-screen element over the chat list.
final class ToastSlotAccessibilityUITests: XCTestCase {
    private let appear: TimeInterval = 150
    private let group = "fx-group-launch"

    override func setUpWithError() throws { continueAfterFailure = false }

    /// A Home row reports itself hittable and takes `row.tap()` — the call that computes a hit
    /// point from the accessibility tree, which is the thing that was broken. A coordinate tap
    /// would pass with the defect in place and so proves nothing here.
    func testAHomeRowCanBeTappedWithNoToastUp() {
        let app = XCUIApplication()
        app.launchEnvironment = ["ARBOS_FIXTURE": "1"]
        app.launch()
        XCTAssertTrue(app.buttons["Agents"].waitForExistence(timeout: appear), "Home is up")

        let slot = app.descendants(matching: .any)["toast-slot"].firstMatch
        XCTAssertFalse(slot.exists,
                       "the empty toast slot is not published at all — it is at \(slot.frame) "
                       + "in a \(app.windows.firstMatch.frame) window")

        let row = app.buttons["home-row:\(group)"]
        XCTAssertTrue(row.waitForExistence(timeout: 60), "the group is in the list")
        XCTAssertTrue(row.isHittable, "the row reports a hit point: it is at \(row.frame)")
        row.tap()
        XCTAssertTrue(app.buttons["chat-back:\(group)"].waitForExistence(timeout: 60),
                      "the chat opened")
    }
}
