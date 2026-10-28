import XCTest
import UIKit
@testable import Arbos

/// The sheet header ([feedback iphone-add-members-2026-10-26], Jacob 2026-10-26 17:52 CR):
/// on "Add members to Arbonites" the Add pill was squeezed to "A…". The rule now: the
/// confirm control is never truncated or compressed at any title, on any phone width;
/// the title shrinks (to 70 %) and then cuts, centred between the controls.
final class SheetHeaderLayoutTests: XCTestCase {
    /// iPhone SE / mini / 17 / 17 Pro Max logical widths.
    private let widths: [CGFloat] = [320, 375, 393, 430]

    /// The longest titles the sheets can show. A group name is free text; 60 characters is
    /// well past anything in the member lists ("Arbonites scratch before-0.1.150" is 32).
    private let titles: [String] = [
        "Add members to Arbonites",
        "Add members to Arbonites scratch before-0.1.150",
        "Add members to " + String(repeating: "Verylonggroupname", count: 4),
        "Give a person access",
        "Give access",
        "New group", "New channel", "Edit group", "Add agent", "New secret", "Replace value",
        "Connect 1Password",
        "OPENROUTER_API_KEY_FOR_THE_SECOND_RESEARCH_AGENT",
        "Share " + String(repeating: "Comms", count: 12),
    ]

    private func width(_ text: String, size: CGFloat, weight: UIFont.Weight) -> CGFloat {
        ceil((text as NSString).size(withAttributes: [.font: UIFont.systemFont(ofSize: size, weight: weight)]).width)
    }

    /// The pill as `SheetHeaderBar` draws it: `Theme.small` semibold + `Controls.textInset` each side.
    private func pillWidth(_ label: String) -> CGFloat { width(label, size: Theme.small, weight: .semibold) + 2 * Controls.textInset }
    private var cancelWidth: CGFloat { width("Cancel", size: Theme.body, weight: .regular) }

    func testTheConfirmControlIsNeverSqueezedAtAnyTitleOnAnyPhone() {
        for w in widths {
            for title in titles {
                for label in SheetAction.all {
                    let trailing = pillWidth(label)
                    let layout = SheetHeaderLayout.solve(width: w, leadingWidth: cancelWidth,
                                                         titleWidth: width(title, size: Theme.body, weight: .semibold),
                                                         trailingWidth: trailing)
                    let ctx = "\(Int(w)) pt · “\(title)” · \(label)"
                    XCTAssertEqual(layout.trailing.width, trailing, "the control keeps its natural width — \(ctx)")
                    XCTAssertEqual(layout.trailing.maxX, w - SheetHeaderLayout.inset, "flush with the bar's inset — \(ctx)")
                    XCTAssertEqual(layout.leading.minX, SheetHeaderLayout.inset, ctx)
                    XCTAssertLessThanOrEqual(layout.title.maxX, layout.trailing.minX - SheetHeaderLayout.gap + 0.01, "title never runs under the control — \(ctx)")
                    XCTAssertGreaterThanOrEqual(layout.title.minX + 0.01, layout.leading.maxX + SheetHeaderLayout.gap, "nor under Cancel — \(ctx)")
                    if layout.titleScale == 1 && !layout.titleTruncated,
                       width(title, size: Theme.body, weight: .semibold) <= SheetHeaderLayout.titleRoom(width: w, leadingWidth: cancelWidth, trailingWidth: trailing) {
                        XCTAssertEqual(layout.title.midX, w / 2, accuracy: 0.01, "a fitting title is centred on the bar — \(ctx)")
                    } else {
                        XCTAssertEqual(layout.title.midX, (layout.leading.maxX + layout.trailing.minX) / 2, accuracy: 0.01, "a long title is centred between the controls — \(ctx)")
                    }
                    XCTAssertGreaterThanOrEqual(layout.titleScale, SheetHeaderLayout.minimumTitleScale, ctx)
                    XCTAssertLessThanOrEqual(layout.titleScale, 1, ctx)
                    XCTAssertEqual(layout.trailing.height, SheetHeaderLayout.height, ctx)
                }
            }
        }
    }

    /// Jacob's exact case: the title that produced "A…" is whole with "Add" and the pill intact;
    /// the wider busy label shrinks it a little and never cuts it.
    func testAddMembersToArbonitesKeepsAddWholeOnAnIPhone17() {
        let title = "Add members to Arbonites"
        for label in ["Add", "Add (1)", "Adding…"] {
            let layout = SheetHeaderLayout.solve(width: 393, leadingWidth: cancelWidth,
                                                 titleWidth: width(title, size: Theme.body, weight: .semibold),
                                                 trailingWidth: pillWidth(label))
            XCTAssertFalse(layout.titleTruncated, label)
            XCTAssertGreaterThanOrEqual(layout.titleScale, 0.8, "\(label): the title shrinks a little at most")
            XCTAssertEqual(layout.trailing.width, pillWidth(label))
        }
        let plain = SheetHeaderLayout.solve(width: 393, leadingWidth: cancelWidth,
                                            titleWidth: width(title, size: Theme.body, weight: .semibold), trailingWidth: pillWidth("Add"))
        XCTAssertEqual(plain.titleScale, 1, "with “Add” the title is drawn whole")
    }

    func testAVeryLongTitleShrinksThenCutsWithoutMovingTheControls() {
        let long = "Add members to " + String(repeating: "Verylonggroupname", count: 4)
        let layout = SheetHeaderLayout.solve(width: 320, leadingWidth: cancelWidth,
                                             titleWidth: width(long, size: Theme.body, weight: .semibold),
                                             trailingWidth: pillWidth("Add (12)"))
        XCTAssertEqual(layout.titleScale, SheetHeaderLayout.minimumTitleScale)
        XCTAssertTrue(layout.titleTruncated)
        XCTAssertEqual(layout.title.width, SheetHeaderLayout.betweenControls(width: 320, leadingWidth: cancelWidth, trailingWidth: pillWidth("Add (12)")))
        XCTAssertEqual(layout.trailing.width, pillWidth("Add (12)"))
        XCTAssertEqual(layout.trailing.maxX, 320 - SheetHeaderLayout.inset)
    }

    func testAFittingTitleIsCentredOnTheBarNotBetweenUnequalControls() {
        let room = SheetHeaderLayout.titleRoom(width: 393, leadingWidth: 20, trailingWidth: 90)
        XCTAssertEqual(room, 393 - 2 * (SheetHeaderLayout.inset + 90 + SheetHeaderLayout.gap))
        let layout = SheetHeaderLayout.solve(width: 393, leadingWidth: 20, titleWidth: 100, trailingWidth: 90)
        XCTAssertEqual(layout.title.midX, 393 / 2)
        XCTAssertEqual(layout.titleScale, 1)
        let long = SheetHeaderLayout.solve(width: 393, leadingWidth: 20, titleWidth: 300, trailingWidth: 90)
        XCTAssertEqual(long.title.minX, long.leading.maxX + SheetHeaderLayout.gap, accuracy: 0.01, "a long title fills the space between the controls")
        XCTAssertEqual(long.title.maxX, long.trailing.minX - SheetHeaderLayout.gap, accuracy: 0.01)
    }

    func testTheBarIsAMemberOfTheControlFamily() {
        XCTAssertEqual(SheetHeaderLayout.height, Controls.height)
        XCTAssertEqual(SheetHeaderLayout.inset, Controls.inset)
        XCTAssertEqual(SheetHeaderLayout.gap, Controls.gap)
        XCTAssertEqual(Controls.Screen.sheetHeader.members, [.bar])
        XCTAssertLessThan(Controls.sheetAction, Controls.height, "the pill sits inside the bar")
    }

    func testTheActionLabelCountsThePicks() {
        XCTAssertEqual(SheetAction.label(adding: true, busy: false, picks: 1), "Add")
        XCTAssertEqual(SheetAction.label(adding: true, busy: false, picks: 3), "Add (3)")
        XCTAssertEqual(SheetAction.label(adding: true, busy: true, picks: 3), "Adding…")
        XCTAssertEqual(SheetAction.label(adding: false, busy: false, picks: 2), "Create")
        XCTAssertEqual(SheetAction.label(adding: false, busy: true, picks: 2), "Creating…")
        for label in SheetAction.all { XCTAssertFalse(label.isEmpty) }
        XCTAssertTrue(SheetAction.all.contains("Add (12)"), "the widest Add form is measured")
    }

    // MARK: Source structure

    private var sourceRoot: URL {
        URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().appendingPathComponent("Arbos/UI")
    }

    /// The sheets with a possibly long title draw `SheetHeaderBar`, and none of them keeps a
    /// system `topBarTrailing` item — the thing the inline bar squeezed.
    func testEveryLongTitleSheetUsesTheSharedHeader() throws {
        let files = ["PeopleSheets.swift", "ProfilePages.swift", "ServerChannelsView.swift", "AgentPicker.swift", "SecretsViews.swift"]
        let first = sourceRoot.appendingPathComponent(files[0])
        try XCTSkipIf(!FileManager.default.fileExists(atPath: first.path), "sources not beside the test bundle")
        for name in files {
            let text = try String(contentsOf: sourceRoot.appendingPathComponent(name), encoding: .utf8)
            XCTAssertTrue(text.contains(".sheetHeader("), "\(name) draws the shared header")
            XCTAssertFalse(text.contains("placement: .topBarTrailing"), "\(name) has no system trailing toolbar item left to squeeze")
        }
        let header = try String(contentsOf: sourceRoot.appendingPathComponent("SheetHeader.swift"), encoding: .utf8)
        XCTAssertTrue(header.contains(".fixedSize()"), "the controls never compress")
        XCTAssertTrue(header.contains(".minimumScaleFactor(SheetHeaderLayout.minimumTitleScale)"), "the title is what gives")
    }
}
