import XCTest

/// Opening a chat must draw its rows without anyone scrolling the list.
///
/// [feedback iphone-blank-chat-on-open-2026-11-03]: Jacob opened an agent chat and got a
/// header and a composer with nothing between them — no rows, no spinner, no hint. A scroll
/// brought the rows in and they stayed. The rows were in the model the whole time; the lazy
/// stack had been left holding the space it reserved for children that left, and built none of
/// the new ones (iOS 27; 26.5 re-measures from the same binary). `transcriptStackIdentity`
/// covered only a search keyword, so that was the one transition with a cure.
///
/// **What this suite can and cannot hold.** XCUITest reads the accessibility tree, not frames
/// per frame, so "the first frame" here means the first look this suite takes — and it never
/// touches the list before taking it. That is enough to fail a transcript that needs a scroll,
/// which is the bug. It is not enough to fail one that is blank for 200 ms and then paints:
/// the mechanism itself is held by `BlankTranscriptGuardTests`, on the pure key, where every
/// transition can be named. The two halves exist on purpose.
///
/// The blank is device-only — the fixture is caught up before the first layout, so no rig open
/// ever reaches the transition that causes it. `ARBOS_FIXTURE_ROWS_DELAY` is the one arm that
/// does: it hands the chats their rows after the screen is up, the shape of an open over a
/// slow link.
final class TranscriptFirstFrameUITests: XCTestCase {
    private let appear: TimeInterval = 150
    private let agentChat = "fx-comms-main"
    private let agentProject = "fx-comms"
    private let group = "fx-group-launch"
    private let dm = "fx-dm-unconst"
    /// `peopleSlot`: the project id every people chat (DM, group) lives under.
    private let peopleProject = "people"

    override func setUpWithError() throws { continueAfterFailure = true }

    private func bubbles(_ app: XCUIApplication) -> [XCUIElement] {
        app.descendants(matching: .any)
            .matching(NSPredicate(format: "identifier BEGINSWITH %@", "bubble:"))
            .allElementsBoundByIndex
    }

    /// Rows exist, and at least one of them is inside the window with height. Taken without a
    /// scroll, a tap or a swipe of any kind on the transcript.
    private func assertRowsAreDrawn(_ app: XCUIApplication, _ state: String) {
        let frames = bubbles(app).map(\.frame).filter { $0.height > 0 }
        XCTAssertFalse(frames.isEmpty, "\(state): the transcript has rows without being scrolled")
        let window = app.windows.firstMatch.frame
        let onScreen = frames.filter { $0.maxY > window.minY && $0.minY < window.maxY }
        XCTAssertFalse(onScreen.isEmpty,
                       "\(state): a row is on screen without being scrolled — \(frames.count) "
                       + "rows are in the tree but all of them are outside \(window)")
        // The newest row is the one a chat opens on: it must not be left below the fold.
        let newest = frames.max { $0.maxY < $1.maxY }
        XCTAssertNotNil(newest)
        if let newest {
            XCTAssertLessThanOrEqual(newest.minY, window.maxY,
                                     "\(state): the newest row starts inside the window — \(newest) in \(window)")
        }
    }

    /// Launch straight into a chat, wait only for the chat screen and a row to be in the tree,
    /// then look. Nothing in here moves the transcript.
    private func landAndAssert(_ state: String, chat: String, project: String,
                               env: [String: String] = [:]) -> XCUIApplication {
        let app = XCUIApplication()
        var e = env
        e["ARBOS_FIXTURE"] = "1"
        e["ARBOS_OPEN_PROJECT"] = project
        e["ARBOS_OPEN_CHAT"] = chat
        app.launchEnvironment = e
        app.launch()
        XCTAssertTrue(app.buttons["chat-back:\(chat)"].waitForExistence(timeout: appear),
                      "\(state): the chat screen is up")
        let row = app.descendants(matching: .any)
            .matching(NSPredicate(format: "identifier BEGINSWITH %@", "bubble:")).firstMatch
        XCTAssertTrue(row.waitForExistence(timeout: appear), "\(state): a row reached the tree")
        assertRowsAreDrawn(app, state)
        return app
    }

    /// Cold: nothing restored from disk, first time this chat is on screen in this launch.
    func testAShortAgentChatDrawsItsRowsOnAColdOpen() {
        _ = landAndAssert("cold short agent chat", chat: agentChat, project: agentProject,
                          env: ["ARBOS_NO_STORE": "1"])
    }

    func testALongHistoryDrawsItsRowsOnAColdOpen() {
        _ = landAndAssert("cold long history", chat: group, project: peopleProject,
                          env: ["ARBOS_NO_STORE": "1", "ARBOS_FIXTURE_LONG_ROWS": "600",
                                "ARBOS_FIXTURE_LONG_RICH": "1"])
    }

    /// The reported shape: the chat is on screen with no rows yet — an agent chat draws its
    /// empty hint there — and the rows land under it. That hint leaving and the window's rows
    /// arriving is one change to the stack's children, and it used to leave the stack blank.
    func testAnAgentChatDrawsRowsThatLandWhileItIsOnScreen() {
        _ = landAndAssert("rows land under the empty hint", chat: agentChat, project: agentProject,
                          env: ["ARBOS_NO_STORE": "1", "ARBOS_FIXTURE_ROWS_DELAY": "4000"])
    }

    /// Warm: the chat has been on screen once already this launch, so its rows, mention table
    /// and draft are all built. Back out and open it again from the list.
    private func reopenAndAssert(_ state: String, chat: String, project: String,
                                 env: [String: String] = [:]) {
        let app = landAndAssert("\(state) (first open)", chat: chat, project: project, env: env)
        app.buttons["chat-back:\(chat)"].tap()
        let row = app.buttons["home-row:\(chat)"]
        XCTAssertTrue(row.waitForExistence(timeout: 60), "\(state): the chat is back in the list")
        // By coordinate, not `row.tap()`: on iOS 26.5 a home row never reports itself hittable
        // after the pop, though it takes the touch, and on 27 it does. The open below is the
        // check that the tap landed either way.
        row.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)).tap()
        XCTAssertTrue(app.buttons["chat-back:\(chat)"].waitForExistence(timeout: 60),
                      "\(state): the chat screen is up again")
        assertRowsAreDrawn(app, state)
    }

    func testAShortChatDrawsItsRowsOnAWarmOpen() {
        reopenAndAssert("warm short chat", chat: dm, project: peopleProject)
    }

    func testALongHistoryDrawsItsRowsOnAWarmOpen() {
        reopenAndAssert("warm long history", chat: group, project: peopleProject,
                        env: ["ARBOS_FIXTURE_LONG_ROWS": "600", "ARBOS_FIXTURE_LONG_RICH": "1"])
    }
}
