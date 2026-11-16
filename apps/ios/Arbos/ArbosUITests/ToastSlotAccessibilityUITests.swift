import XCTest

/// An empty toast slot publishes no box over the rows underneath it.
///
/// `ToastHost` is an overlay that was sized `.frame(maxWidth: .infinity, maxHeight: .infinity)`.
/// While it holds no toasts it holds no children either, and a container with no children has no
/// child frames to union, so SwiftUI publishes the element at its own layout box. Measured on
/// Home with nothing else happening, that box came out at `(0, 62, 402, 778)` on a 402×874
/// screen: one accessibility element spanning the content area, for as long as no toast is
/// showing, which is almost always. VoiceOver reads that tree, so an assistive technology user
/// met a full-screen element over the chat list.
///
/// The layer is content-sized now, so the box an empty one publishes is its own top padding —
/// `(0, 62, 402, 8)` as measured, against rows at `(0, 420, 402, 80)`. This case is the runtime
/// half of that change: `ToastStackTests` pins the frame and the three call sites that place it,
/// which is the code's shape, and this measures what the tree actually ends up containing.
///
/// **This is an accessibility-tree claim and nothing more.** Earlier versions of this file also
/// asserted that a row underneath reports a hit point and takes `row.tap()`, on the theory that
/// the slot was what made rows unreachable. That is refuted: with the full-screen slot published,
/// the row underneath was hittable and took `row.tap()` at every one of 70 samples across 10
/// launches, and about a dozen *other* full-screen elements contain every row's centre whatever
/// this slot does — a window, a `Toolbar` at `(0, 0, 402, 874)`, eight to ten unnamed containers
/// at the same frame. So no hittability, no hit points and no taps are asserted here, and any of
/// them re-added would be measuring something this view does not control. See
/// `internal/af834-voiceover-bubble-frames.md`, AF-851 and AF-917.
///
/// **What the assertion has to be is "covers nothing", not "is absent".** The slot is still in
/// the tree — a content-sized box is a small box, not no box — and an earlier
/// `XCTAssertFalse(slot.exists)` here failed on a fix that worked, because `exists` looks the
/// element up by identifier and so reports on the lookup rather than on the geometry.
///
/// The non-empty state is not asserted because no landable case can raise a toast: it needs a
/// debug hook, and the hook used for the measurement was scaffolding. With a toast up the box is
/// the union of the notices in it, which is what it has always been — measured at
/// `(16, 70, 370, 90)` in all five arms, with the note, its × and its Try again still in the tree.
final class ToastSlotAccessibilityUITests: XCTestCase {
    private let appear: TimeInterval = 150
    private let group = "fx-group-launch"

    /// Every assertion below is an independent reading, and an iPhone gate run costs about an
    /// hour of queue, so the case reports all of them rather than stopping at the first.
    override func setUpWithError() throws { continueAfterFailure = true }

    func testTheEmptyToastSlotCoversNoRows() {
        let app = XCUIApplication()
        app.launchEnvironment = ["ARBOS_FIXTURE": "1"]
        app.launch()
        XCTAssertTrue(app.buttons["Agents"].waitForExistence(timeout: appear), "Home is up")

        let row = app.buttons["home-row:\(group)"]
        XCTAssertTrue(row.waitForExistence(timeout: 60), "the group is in the list")

        // The positive control for the assertion that follows. An absent element reports
        // `frame == .zero`, which intersects nothing, so "covers no rows" would pass on a build
        // where the slot had simply stopped being published — including one where it stopped
        // for a reason that broke the toasts themselves.
        let slot = app.descendants(matching: .any)["toast-slot"].firstMatch
        XCTAssertTrue(slot.exists, "the slot is still in the tree, so the next assertion can fail")

        XCTAssertFalse(slot.frame.intersects(row.frame),
                       "the empty toast slot does not reach the rows: slot \(slot.frame) against "
                       + "row \(row.frame) in a \(app.windows.firstMatch.frame) window")
    }
}
