import XCTest

/// A back swipe that starts on a sender's profile picture (Jacob, 2026-11-01: "the slide to
/// go back doesn't register when you are over top a user pfp which is common but this should
/// still register as the slide and only a direct click should go to the user profile screen").
///
/// Every avatar beside a bubble is a SwiftUI `Button` (`sender-avatar` → `toggleProfile`), and
/// a `Button` tracks its press with a recognizer that enters `.began` on touch-down. UIKit lets
/// an already-begun recognizer prevent every later one that does not share the touch with it,
/// so the swipe was prevented before `BackSwipe.lockDistance` and the screen did not move. The
/// fix is `BackSwipe.sharesTouch`: the back recognizers tell UIKit they may run alongside the
/// presses and taps, and still not alongside any pan.
///
/// Three cases, because the fix must buy the swipe without costing anything else:
/// the swipe goes back, the tap still opens the page, the vertical drag still scrolls. The
/// middle one is the guard on the mechanism as much as on the behaviour — `require(toFail:)`
/// would also make the swipe win, by holding the tap until the swipe had failed, and a profile
/// that opens after a pause is its own regression.
///
/// Separate from `BackSwipeUITests` on purpose: that file is being edited in another lane.
///
/// Runs the fixture app (`ARBOS_FIXTURE=1`, Debug builds): no relay, deterministic screens.
final class AvatarBackSwipeUITests: XCTestCase {
    /// The shared Mac runs the gate and releases at the same time; waits are generous.
    private let appear: TimeInterval = 90
    private let settle: TimeInterval = 20

    /// The Launch week group with a long transcript: many runs of messages, so avatars sit all
    /// down the screen and one of them is reachable wherever the chat comes to rest.
    private let chatId = "fx-group-launch"

    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    private func openGroup() -> XCUIApplication {
        let app = XCUIApplication()
        app.launchEnvironment = ["ARBOS_FIXTURE": "1", "ARBOS_OPEN_PEOPLE": chatId,
                                 "ARBOS_FIXTURE_LONG_ROWS": "120"]
        app.launch()
        XCTAssertTrue(back(app).waitForExistence(timeout: appear), "the Launch week group opened")
        settleTranscript(app)
        return app
    }

    private func back(_ app: XCUIApplication) -> XCUIElement {
        app.buttons["chat-back:\(chatId)"]
    }

    private var bubbleQuery: NSPredicate { NSPredicate(format: "identifier BEGINSWITH 'bubble:'") }

    /// Wait until the transcript has stopped moving. A chat opens by scrolling to its newest
    /// message, and the first bubble exists before that scroll ends: a frame read mid-scroll
    /// says where a row was, not where the finger will land.
    private func settleTranscript(_ app: XCUIApplication, timeout: TimeInterval = 8) {
        let bubbles = app.descendants(matching: .any).matching(bubbleQuery)
        var previous = CGRect.null
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            let current = bubbles.firstMatch.frame
            if !current.isEmpty, current == previous { return }
            previous = current
            Thread.sleep(forTimeInterval: 0.25)
        }
    }

    /// A sender's avatar the finger can reach: on screen, clear of the header and of the
    /// composer bar (a drag that starts in the bar is the composer's, by design), and clear of
    /// the 24 pt edge zone that belongs to the system pop. Returns its frame, so each case can
    /// aim where it needs to and say what it aimed at when it fails.
    private func reachableAvatar(_ app: XCUIApplication,
                                 file: StaticString = #filePath, line: UInt = #line) -> CGRect {
        let avatars = app.descendants(matching: .any).matching(identifier: "sender-avatar")
        XCTAssertTrue(avatars.firstMatch.waitForExistence(timeout: settle),
                      "the group transcript draws sender avatars", file: file, line: line)
        let window = app.windows.firstMatch.frame
        let usable = avatars.allElementsBoundByIndex.filter {
            $0.isHittable && $0.frame.minY > window.minY + 120 && $0.frame.maxY < window.maxY - 160
        }
        XCTAssertFalse(usable.isEmpty, "an avatar is on screen between the header and the composer",
                       file: file, line: line)
        let frame = usable.last!.frame
        XCTAssertGreaterThan(frame.minX, 24,
                             "the avatar is clear of the edge zone, so this is the full-screen "
                             + "swipe and not the system pop", file: file, line: line)
        return frame
    }

    /// Whether the chat is the frontmost screen. Taken from `ChatSearchResultJumpUITests`: a
    /// profile page pushed over the chat still reports the chat's elements with plausible
    /// frames, so "the header is reachable" is the only honest read of what is in front.
    private func chatIsFrontmost(_ app: XCUIApplication) -> Bool {
        app.descendants(matching: .any)["chat-search-button"].firstMatch.isHittable
    }

    private func assertGone(_ element: XCUIElement, file: StaticString = #filePath, line: UInt = #line) {
        let gone = NSPredicate(format: "exists == false")
        let wait = XCTNSPredicateExpectation(predicate: gone, object: element)
        XCTAssertEqual(XCTWaiter().wait(for: [wait], timeout: settle), .completed,
                       "\(element) left the screen", file: file, line: line)
    }

    private func assertHome(_ app: XCUIApplication, file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertTrue(app.buttons["Agents"].waitForExistence(timeout: settle),
                      "Home's list filter is on screen", file: file, line: line)
        XCTAssertTrue(app.buttons["People"].exists, file: file, line: line)
    }

    // MARK: The bug

    /// The case Jacob reported: the drag begins on a picture and is a back swipe like any
    /// other. It starts at the avatar's centre, which is about 55 pt in — well past the edge
    /// zone — so this is the transcript's swipe-right, not the system pop.
    func testASwipeRightStartingOnASenderAvatarGoesBack() {
        let app = openGroup()
        let avatar = reachableAvatar(app)
        let distance: CGFloat = 180
        XCTAssertLessThan(avatar.midX + distance, app.windows.firstMatch.frame.maxX - 8,
                          "the \(Int(distance)) pt drag would be clamped at the screen edge and "
                          + "never reach the app")
        let start = app.coordinate(withNormalizedOffset: .zero)
            .withOffset(CGVector(dx: avatar.midX, dy: avatar.midY))

        start.press(forDuration: 0.05, thenDragTo: start.withOffset(CGVector(dx: distance, dy: 0)),
                    withVelocity: .fast, thenHoldForDuration: 0.05)

        assertGone(back(app))
        assertHome(app)
    }

    // MARK: What the fix must not cost

    /// A tap on the avatar still opens the sender's page, and opens it on the touch that made
    /// it — nothing was made to wait for the swipe to fail. The page is "the chat is no longer
    /// frontmost": an agent's page carries none of the `profile-*` markers a person's does.
    func testATapOnASenderAvatarStillOpensTheProfile() {
        let app = openGroup()
        let avatar = reachableAvatar(app)
        XCTAssertTrue(chatIsFrontmost(app), "the chat is in front before the tap")

        app.coordinate(withNormalizedOffset: .zero)
            .withOffset(CGVector(dx: avatar.midX, dy: avatar.midY)).tap()

        let left = NSPredicate(format: "isHittable == false")
        let wait = XCTNSPredicateExpectation(predicate: left,
                                             object: app.descendants(matching: .any)["chat-search-button"].firstMatch)
        XCTAssertEqual(XCTWaiter().wait(for: [wait], timeout: settle), .completed,
                       "a tap on the avatar at \(avatar) opens the sender's page over the chat")
    }

    /// A vertical drag that begins on an avatar is still the scroll view's: the transcript
    /// moves and the chat stays. The scroll's pan is a `UIPanGestureRecognizer`, which is
    /// exactly what `BackSwipe.sharesTouch` still refuses to share with.
    func testAVerticalDragStartingOnASenderAvatarStillScrolls() {
        let app = openGroup()
        let avatar = reachableAvatar(app)
        let window = app.windows.firstMatch.frame
        XCTAssertGreaterThan(avatar.midY - 200, window.minY + 8, "the 200 pt drag up stays on screen")
        let bubbles = app.descendants(matching: .any).matching(bubbleQuery)
        let before = bubbles.firstMatch.frame
        let start = app.coordinate(withNormalizedOffset: .zero)
            .withOffset(CGVector(dx: avatar.midX, dy: avatar.midY))

        start.press(forDuration: 0.05, thenDragTo: start.withOffset(CGVector(dx: 6, dy: -200)),
                    withVelocity: .slow, thenHoldForDuration: 0.1)
        sleep(1)

        XCTAssertTrue(back(app).exists, "the chat did not go back")
        XCTAssertTrue(chatIsFrontmost(app), "no profile page opened either")
        XCTAssertNotEqual(bubbles.firstMatch.frame, before, "the transcript scrolled")
    }
}
