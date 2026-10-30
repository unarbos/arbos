import XCTest
@testable import Arbos

/// The iPhone control family ([feedback iphone-header-composer-sizing-2026-10-26], Jacob
/// 2026-10-26): one diameter, one height, one radius, one rim, one glyph size, one bar
/// inset for every round button, pill, field and bar — the composer's numbers (#1841).
/// `docs/mac-design-rules.md` § "iPhone control family" is the written rule; this test
/// pins it and walks the main screens so a new screen cannot drift.
final class ControlMetricsTests: XCTestCase {
    func testTheFamilyIsTheComposers() {
        XCTAssertEqual(Controls.diameter, 48)
        XCTAssertEqual(Controls.height, 48)
        XCTAssertEqual(Controls.diameter, Theme.plusButton, "round buttons = the composer +")
        XCTAssertEqual(Controls.height, Theme.composerHeight, "pills and fields = the composer field")
        XCTAssertEqual(Controls.radius, Controls.height / 2, "capsule corners")
        XCTAssertEqual(Controls.gap, Theme.plusFieldGap)
        XCTAssertEqual(Controls.textInset, Theme.composerTextInset)
        XCTAssertEqual(Theme.control, Controls.diameter)
        XCTAssertEqual(Theme.fieldHeight, Controls.height)
    }

    func testPillPictureSitsConcentricWithTheCapsuleEnd() {
        XCTAssertEqual(Controls.avatar, 32)
        XCTAssertEqual(Controls.avatarInset, 8)
        XCTAssertEqual(Controls.avatar + 2 * Controls.avatarInset, Controls.height)
    }

    func testGlyphScalesWithTheDisc() {
        // 17 pt in the old 42 pt disc → 19 in the 48 one (17 × 48 / 42 = 19.4).
        XCTAssertEqual(Controls.glyph, 19)
        XCTAssertEqual(Controls.glyph, (17 * Controls.diameter / 42).rounded(.down))
    }

    func testEveryMainScreenUsesTheFamilySizes() {
        for screen in Controls.Screen.allCases {
            XCTAssertFalse(screen.members.isEmpty, "\(screen)")
            for member in screen.members {
                switch member {
                case .disc: XCTAssertEqual(member.size, Controls.diameter, "\(screen) disc")
                case .pill, .field, .bar: XCTAssertEqual(member.size, Controls.height, "\(screen) \(member)")
                }
            }
            if let inset = screen.sideInset {
                XCTAssertEqual(inset, Controls.inset, "\(screen) side inset")
            } else {
                XCTAssertEqual(screen, .composer, "only the composer's side inset follows the keyboard")
            }
        }
        // The three top bars (home, chat, server) carry the same disc · pill · disc family.
        XCTAssertEqual(Controls.Screen.chatHeader.members, Controls.Screen.serverHeader.members)
        XCTAssertEqual(Controls.Screen.chatSearchBar.members, Controls.Screen.bottomSearchBar.members)
    }

    func testEveryExceptionIsWrittenDown() {
        // Each case documents a surface that keeps its own size; the rule doc lists the same set.
        XCTAssertEqual(Set(Controls.exceptions), Set(Controls.Exception.allCases))
        XCTAssertEqual(Controls.Exception.allCases.count, 11)
        XCTAssertEqual(Theme.voiceButton, 36, "in-call strip chip — Exception.inCallStrip")
        XCTAssertEqual(Theme.plusGlyph, 22.5, "composer + glyph — Exception.plusGlyph")
    }
}
