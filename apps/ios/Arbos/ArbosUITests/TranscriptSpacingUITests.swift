import XCTest

/// Where an incoming row's avatar and bubble actually land, measured on screen.
///
/// `ChatDensityMetricsTests` and `PresenceRowTests` pin the tokens and the pure function —
/// `Theme.avatarColumnInset`, and `Theme.memberLane` answering the same at both composer
/// insets. That catches anyone who re-derives the column inside `Theme`, and it catches a
/// change of type at compile time. It does **not** catch the edit that reaches for
/// `composerInset` at the call site in `RowView.avatarColumn`, where that value is already in
/// scope: every unit test would still pass while the lane moved. This suite is the other half.
/// Both exist on purpose; neither is redundant.
///
/// The column was centred on the composer's `+` until Jacob's 2026-11-01 reference
/// ([feedback group-chat-spacing-ios-2026-11-01]) put his discs at 12 pt. The keyboard-up pass
/// is the one that matters: with the keyboard down a composer derivation and a fixed column
/// can agree by luck, and with it up they cannot — the old rule moved the whole lane to 24 pt.
final class TranscriptSpacingUITests: XCTestCase {
    private let appear: TimeInterval = 150
    /// The transcript's own horizontal padding, outside anything `Theme` exposes.
    private static let listPad: CGFloat = 16
    /// Jacob's numbers: the disc spans 12–48 and the bubble body starts at 52.
    private static let avatarLeft: CGFloat = 12
    private static let avatarSize: CGFloat = 36
    /// The sender's name sits inside the bubble's `Theme.bubbleHPad`, so its leading edge is
    /// the bubble's left edge plus 14. The name is the only handle on the bubble body: the
    /// body carries no accessibility identifier (only my own bubbles do) and the row's
    /// `bubble:` identifier spans the whole row, so neither can say where the bubble starts.
    ///
    /// 52 and 12 are measured — off Jacob's reference and off simulator frames. 66 is
    /// *derived* from them, because no log we have reports this button's frame; the name's
    /// ink lands there in the captures. If the gate says otherwise, the button's frame is
    /// offset from its text and this constant moves once — the failure prints the frame it
    /// read. The guard keeps its value either way: the regression it exists to catch moves
    /// this edge from 66 to 97.
    private static let bubbleHPad: CGFloat = 14
    private static let bubbleLeft: CGFloat = 52
    /// Frames come back in device points as `Double`; a third of a point of rounding is noise.
    private static let slack: CGFloat = 0.6

    override func setUpWithError() throws {
        continueAfterFailure = true
    }

    private func launch() -> XCUIApplication {
        let app = XCUIApplication()
        app.launchEnvironment = [
            "ARBOS_FIXTURE": "1",
            "ARBOS_NO_STORE": "1",
            "ARBOS_OPEN_PEOPLE": "fx-group-launch",
        ]
        app.launch()
        let bubbles = app.descendants(matching: .any)
            .matching(NSPredicate(format: "identifier BEGINSWITH %@", "bubble:"))
        XCTAssertTrue(bubbles.firstMatch.waitForExistence(timeout: appear), "the transcript is up")
        return app
    }

    private func discs(_ app: XCUIApplication) -> [XCUIElement] {
        app.descendants(matching: .any)
            .matching(NSPredicate(format: "identifier == %@", "sender-avatar"))
            .allElementsBoundByIndex
            .filter { $0.exists && $0.frame.height > 0 }
    }

    private func names(_ app: XCUIApplication) -> [XCUIElement] {
        app.descendants(matching: .any)
            .matching(NSPredicate(format: "identifier == %@", "bubble-name"))
            .allElementsBoundByIndex
            .filter { $0.exists && $0.frame.width > 0 }
    }

    /// Open the composer so the keyboard comes up, which is what used to move the lane.
    private func raiseKeyboard(_ app: XCUIApplication) {
        let field = app.descendants(matching: .any)["composer-field"].firstMatch
        guard field.exists else { return XCTFail("the composer is on screen") }
        field.tap()
        _ = app.keyboards.firstMatch.waitForExistence(timeout: 10)
        XCTAssertEqual(app.keyboards.count, 1, "the keyboard came up, so the composer has moved in")
        sleep(2)
    }

    /// Both halves of the claim, read in one pass so they describe the same moment.
    private func assertLaneIsWhereJacobPutIt(_ app: XCUIApplication, _ state: String) {
        let found = discs(app)
        XCTAssertFalse(found.isEmpty, "\(state): the transcript shows at least one sender's picture")
        for disc in found {
            let f = disc.frame
            XCTAssertEqual(f.minX, Self.avatarLeft, accuracy: Self.slack,
                           "\(state): the picture starts \(Self.avatarLeft) pt from the screen, "
                           + "not on the composer's + — frame \(f)")
            XCTAssertEqual(f.width, Self.avatarSize, accuracy: Self.slack,
                           "\(state): the picture is \(Self.avatarSize) pt wide — frame \(f)")
        }
        // The disc and the bubble are placed by two different expressions — the disc by
        // `avatarColumn`'s padding, the bubble by the HStack spacing after it — so a disc in
        // the right place does not prove the bubble followed it.
        let named = names(app)
        XCTAssertFalse(named.isEmpty, "\(state): at least one bubble names its sender")
        for name in named {
            XCTAssertEqual(name.frame.minX, Self.bubbleLeft + Self.bubbleHPad, accuracy: Self.slack,
                           "\(state): the bubble body starts at \(Self.bubbleLeft), so the name "
                           + "inside its \(Self.bubbleHPad) pt padding starts at "
                           + "\(Self.bubbleLeft + Self.bubbleHPad) — frame \(name.frame)")
        }
    }

    /// Keyboard down, then up, in one launch: the lane must not move between them. Asserting
    /// both in one test is deliberate — the regression this guards against is a *difference*
    /// between the two states, and two separate tests could each pass on their own numbers
    /// while the lane still slid.
    func testTheAvatarLaneHoldsItsColumnWhateverTheKeyboardDoes() {
        let app = launch()
        assertLaneIsWhereJacobPutIt(app, "keyboard down")
        let down = discs(app).map(\.frame.minX)

        raiseKeyboard(app)
        assertLaneIsWhereJacobPutIt(app, "keyboard up")
        let up = discs(app).map(\.frame.minX)

        XCTAssertFalse(up.isEmpty, "pictures are still on screen with the keyboard up")
        for x in down + up {
            XCTAssertEqual(x, Self.avatarLeft, accuracy: Self.slack,
                           "the column is the same with the keyboard down \(down) and up \(up); "
                           + "a lane that reads the composer inset gives 37 and 24")
        }
    }
}
