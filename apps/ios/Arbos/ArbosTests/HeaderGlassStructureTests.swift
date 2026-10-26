import XCTest
@testable import Arbos

/// The chat header ([feedback ios-header-glass-2026-10-25], Jacob 2026-10-25): back,
/// the name pill and search are one spec for every chat kind, and on Glass all of
/// them — and the Search-in-chat bar — carry the glass material (Liquid Glass on
/// iOS 26, frost before it). Reference: Grok Bot's Affine header; the Bubble shot
/// showed our near-solid discs. The Mac twin is `ui/header.rs` `glass_tests`.
final class HeaderGlassStructureTests: XCTestCase {
    func testEveryKindHasTheSameThreeControls() {
        for kind in ChatHeaderKind.allCases {
            XCTAssertEqual(ChatHeaderGlass.controls(for: kind), [.back, .titlePill, .search], "\(kind)")
        }
    }

    func testGlassSurfacePutsTheGlassMaterialOnEveryControlOfEveryKind() {
        for kind in ChatHeaderKind.allCases {
            let liquid = ChatHeaderGlass.materials(kind: kind, surface: .glass, liquidGlassAvailable: true)
            XCTAssertEqual(liquid.map(\.1), [.liquidGlass, .liquidGlass, .liquidGlass], "\(kind): iOS 26")
            let frost = ChatHeaderGlass.materials(kind: kind, surface: .glass, liquidGlassAvailable: false)
            XCTAssertEqual(frost.map(\.1), [.frosted, .frosted, .frosted], "\(kind): older iOS")
            XCTAssertFalse(liquid.map(\.1).contains(.solid), "\(kind): no solid disc on Glass")
        }
        // The same material whichever kind opened the chat: no Affine-vs-Bubble split.
        let perKind = ChatHeaderKind.allCases.map { kind in ChatHeaderGlass.materials(kind: kind, surface: .glass, liquidGlassAvailable: true).map(\.1) }
        XCTAssertEqual(Set(perKind.map { $0.description }).count, 1)
    }

    func testMatteKeepsSolidControls() {
        for kind in ChatHeaderKind.allCases {
            XCTAssertEqual(ChatHeaderGlass.materials(kind: kind, surface: .matte, liquidGlassAvailable: true).map(\.1), [.solid, .solid, .solid], "\(kind)")
        }
    }

    func testSearchBarWearsTheHeaderMaterial() {
        for available in [true, false] {
            for surface in Surface.allCases {
                XCTAssertEqual(
                    ChatHeaderGlass.searchBarMaterial(surface: surface, liquidGlassAvailable: available),
                    ChatHeaderGlass.material(surface: surface, liquidGlassAvailable: available),
                    "\(surface) liquid=\(available)")
            }
        }
    }

    func testFrostTintIsThinEnoughToSeeThrough() {
        // The old 85 % tint is what made the discs read as solid.
        XCTAssertLessThanOrEqual(ChatHeaderGlass.frostedTint, 0.4)
        XCTAssertEqual(ChatHeaderGlass.legacyTint, 0.85)
    }
}
