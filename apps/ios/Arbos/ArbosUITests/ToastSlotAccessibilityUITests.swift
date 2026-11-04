import XCTest

/// An empty toast slot publishes no box over the rows underneath it.
///
/// `ToastHost` is an overlay that was sized `.frame(maxWidth: .infinity, maxHeight: .infinity)`
/// unconditionally. While it holds no toasts it holds no children either, and a container with
/// no children has no child frames to union, so SwiftUI publishes the element at its own layout
/// box. Measured on Home with nothing else happening, that box came out at `(0, 62, 402, 778)`
/// on a 402×874 screen: one accessibility element spanning the content area, for as long as no
/// toast is showing, which is almost always. VoiceOver reads that tree, so an assistive
/// technology user met a full-screen element over the chat list.
///
/// **This is an accessibility-tree claim and nothing more.** Earlier versions of this file also
/// asserted that a row underneath reports a hit point and takes `row.tap()`, on the theory that
/// the slot was what made rows unreachable. Five arms measured in one build refuted that: with
/// the full-screen slot published, the row underneath is hittable and takes `row.tap()` anyway,
/// and about a dozen *other* full-screen elements contain every row's centre whatever this slot
/// does — a window, a `Toolbar` at `(0, 0, 402, 874)`, eight to ten unnamed containers at the
/// same frame. So no hittability, no hit points and no taps are asserted here, and any of them
/// re-added would be measuring something this view does not control. See
/// `internal/af834-voiceover-bubble-frames.md` and AF-851.
///
/// **What the assertion has to be is "covers nothing", not "is absent".** A collapsed slot is
/// still in the tree — that is deliberate, so the toast transition stays a change within one
/// container — and an earlier `XCTAssertFalse(slot.exists)` here failed on a fix that worked,
/// because `exists` looks the element up by identifier and so reports on the lookup rather than
/// on the geometry.
///
/// The non-empty state is not asserted because no landable case can raise a toast: it needs a
/// debug hook, and the hook used for the measurement was scaffolding. It is unchanged by
/// construction — with a toast up this view's modifier chain is the one it has always had — and
/// it was measured so: all five arms published `(16, 70, 370, 90)` with a toast showing and kept
/// the note, its × and its Try again in the tree.
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
