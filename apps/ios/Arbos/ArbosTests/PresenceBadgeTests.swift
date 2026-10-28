import SwiftUI
import XCTest
@testable import Arbos

/// The presence badge ([feedback presence-dot-layering-ios-2026-10-27], Jacob 2026-10-27;
/// `docs/mac-design-rules.md` § Avatars): one component draws every status dot on a
/// picture — in front of it, past the bottom-right corner, inside a 2 pt ring cut out of
/// the picture so the surface behind shows through. The numbers are the Mac's
/// (`ui::badge`): dot = size / 4 clamped 6–12, ring 2, the ring box 2 past the edge.
final class PresenceBadgeTests: XCTestCase {
    func testTheDotIsAQuarterOfThePictureWithinBounds() {
        XCTAssertEqual(PresenceBadge.metrics(48).dot, 12, "the home account circle (Grok's 12 on 48)")
        XCTAssertEqual(PresenceBadge.metrics(48).ring, 16)
        XCTAssertEqual(PresenceBadge.metrics(Controls.avatar).dot, 8, "the header pill's 32 pt picture")
        XCTAssertEqual(PresenceBadge.metrics(36).dot, 9, "the Mac's footer disc, same rule")
        XCTAssertEqual(PresenceBadge.metrics(22).dot, 6, "the Mac's header disc")
        XCTAssertEqual(PresenceBadge.metrics(116).dot, PresenceBadge.dotMax, "a hero never grows past 12")
        XCTAssertEqual(PresenceBadge.metrics(16).dot, PresenceBadge.dotMin)
        XCTAssertEqual(PresenceBadge.ringWidth, 2)
        XCTAssertEqual(PresenceBadge.overhang, 2)
        for size: CGFloat in [22, 32, 36, 48, 116] {
            let m = PresenceBadge.metrics(size)
            XCTAssertEqual(m.ring, m.dot + 2 * PresenceBadge.ringWidth, "\(size): a 2 pt ring")
            XCTAssertEqual(m.overhang, PresenceBadge.overhang)
            // The dot's centre lies past the picture's circle: read against the surface,
            // never swallowed by the picture. Ring box right = size + overhang, so the
            // centre sits at size + overhang − ring / 2 on both axes.
            let c = size + m.overhang - m.ring / 2 - size / 2
            XCTAssertGreaterThan((c * c * 2).squareRoot(), size / 2, "\(size): dot centre outside the circle")
        }
    }

    /// The badge is the last layer and the picture under it is masked (the ring is a hole,
    /// not a painted colour): the modifier takes no ring colour at all, so no surface can
    /// get a wrong one.
    func testTheRingIsACutOutNotAColour() {
        let m = PresenceBadge.Modifier(color: .green, size: 48)
        XCTAssertEqual(m.size, 48)
        XCTAssertNotNil(m.color)
        let mirror = Mirror(reflecting: m)
        XCTAssertEqual(Set(mirror.children.compactMap(\.label)), ["color", "size"], "dot colour and size only — the ring is cut from the picture")
    }

    // MARK: Lint — every machine dot on a picture goes through the badge

    private var sourceRoot: URL {
        URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().appendingPathComponent("Arbos")
    }

    /// A `machineDotColor(…)` outside `UI/Avatar.swift` must be a `presenceBadge(…)`
    /// argument, a row's `dot:` parameter (an inline status dot beside text), or carry
    /// `// presence-badge: inline (<why>)`. A corner dot built by hand (a `Circle()` with
    /// its own `strokeBorder` ring and `offset`) is what this fails on.
    func testNoHandBuiltCornerDotOutsideTheBadge() throws {
        let fm = FileManager.default
        let ui = sourceRoot.appendingPathComponent("UI")
        try XCTSkipIf(!fm.fileExists(atPath: ui.path), "sources not beside the test bundle (CI without the tree)")
        var offenders: [String] = []
        for file in try fm.contentsOfDirectory(at: ui, includingPropertiesForKeys: nil)
            where file.pathExtension == "swift" && file.lastPathComponent != "Avatar.swift" {
            let lines = try String(contentsOf: file, encoding: .utf8).components(separatedBy: "\n")
            for (i, line) in lines.enumerated() where line.contains("machineDotColor(") {
                let allowed = line.contains("presenceBadge(") || line.contains("dot: machineDotColor(") || line.contains("// presence-badge: inline")
                if !allowed {
                    offenders.append("\(file.lastPathComponent):\(i + 1): \(line.trimmingCharacters(in: .whitespaces))")
                }
            }
        }
        XCTAssertTrue(offenders.isEmpty, "machine dots drawn outside the presence badge:\n" + offenders.joined(separator: "\n"))
    }
}
