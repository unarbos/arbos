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
    /// Jacob's numbers: the disc spans 12–48.
    private static let avatarLeft: CGFloat = 12
    private static let avatarSize: CGFloat = 36
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

    /// The picture's own column, in absolute points. These are Jacob's numbers and ours.
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
    }

    /// Where the bubble bodies start, as the names report it, keyed by the sender named.
    ///
    /// The disc and the bubble are placed by two different expressions — the disc by
    /// `avatarColumn`'s padding, the bubble by the HStack spacing after it — so a disc in the
    /// right place does not prove the bubble followed it. The name is the only handle on the
    /// bubble body: the body carries no accessibility identifier (only my own bubbles do) and
    /// the row's `bubble:` identifier spans the whole row.
    ///
    /// **The names do not share one leading edge, and no absolute is asserted.** A name in a
    /// text bubble sits inside that bubble's `Theme.bubbleHPad`, so it starts at 66: the body
    /// begins at 52 and the padding is 14. A name above a picture or file card has no bubble
    /// around it — `RowView.mediaRow` puts the name and the card in a bare VStack — and
    /// carries its own 6 pt leading padding, so it starts at 58. Both are measured from the
    /// same lane and both are right. The 66 is confirmed against ink rather than inferred: in
    /// the 2026-11-01 capture at 3.0 px/pt the "Mira Kaplan" glyphs run from 67.3 pt, which is
    /// 66.0 plus about 1.3 pt of side bearing on the "M".
    ///
    /// So two names are never compared with each other, and neither number is pinned — 66 and
    /// 58 are both reachable only through this proxy, and an absolute here would redden the
    /// gate for a row type the suite was not written to police. Each name is compared with
    /// *itself* across the two keyboard states instead. That is the invariant this suite
    /// exists for: a lane re-derived from `composerInset` at the call site moves every name by
    /// the 13 pt the inset changes by, whatever a given row's own padding happens to be.
    private func nameEdgesByLabel(_ app: XCUIApplication, _ state: String) -> [String: CGFloat] {
        let named = names(app)
        XCTAssertFalse(named.isEmpty, "\(state): at least one bubble names its sender")
        var edges: [String: CGFloat] = [:]
        var seen: Set<String> = []
        for element in named {
            let label = element.label
            guard !label.isEmpty else { continue }
            // A sender who both writes and sends a picture is named twice in one state, once
            // at each inset. Neither entry can be matched to its own self in the other state,
            // so drop the label rather than guess which row survived the scroll.
            if seen.contains(label) {
                edges[label] = nil
                continue
            }
            seen.insert(label)
            edges[label] = element.frame.minX
        }
        return edges
    }

    /// Keyboard down, then up, in one launch: the lane must not move between them. Asserting
    /// both in one test is deliberate — the regression this guards against is a *difference*
    /// between the two states, and two separate tests could each pass on their own numbers
    /// while the lane still slid.
    func testTheAvatarLaneHoldsItsColumnWhateverTheKeyboardDoes() {
        let app = launch()
        assertLaneIsWhereJacobPutIt(app, "keyboard down")
        let down = discs(app).map(\.frame.minX)
        let namesDown = nameEdgesByLabel(app, "keyboard down")

        raiseKeyboard(app)
        assertLaneIsWhereJacobPutIt(app, "keyboard up")
        let up = discs(app).map(\.frame.minX)
        let namesUp = nameEdgesByLabel(app, "keyboard up")

        XCTAssertFalse(up.isEmpty, "pictures are still on screen with the keyboard up")
        for x in down + up {
            XCTAssertEqual(x, Self.avatarLeft, accuracy: Self.slack,
                           "the column is the same with the keyboard down \(down) and up \(up); "
                           + "a lane that reads the composer inset gives 37 and 24")
        }
        // Each name against its own earlier self, matched by the sender it names, and only
        // for senders named in both states: raising the keyboard scrolls rows off, so a name
        // present in one list and absent from the other says nothing about the lane.
        let shared = Set(namesDown.keys).intersection(namesUp.keys).sorted()
        XCTAssertFalse(shared.isEmpty,
                       "some sender is named in both keyboard states, so there is a name to "
                       + "compare — down \(namesDown), up \(namesUp)")
        for label in shared {
            guard let wasAt = namesDown[label], let nowAt = namesUp[label] else { continue }
            XCTAssertEqual(nowAt, wasAt, accuracy: Self.slack,
                           "\(label)'s name starts where it did before the keyboard came up; "
                           + "a lane that reads the composer inset moves it by the 13 pt the "
                           + "inset changes by — down \(namesDown), up \(namesUp)")
        }
    }
}
