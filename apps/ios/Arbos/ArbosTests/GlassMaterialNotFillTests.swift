import XCTest
@testable import Arbos

/// Guard for the one mechanism that takes the see-through composer away
/// ([feedback composer-glass-regression-2026-10-30], Jacob: "We lost the glass see through
/// chat bars on iPhone"): a **material replaced by, or covered with, an opaque fill**.
///
/// `PillGlassTests` pins what the spec should say. This file pins that the spec is still
/// honoured in the views — a green run there with a `Color` fill painted over the glass is
/// exactly the hole that let the first see-through cut ship as a near-opaque dark pill.
///
/// The source checks read `UI/Theme.swift` and `UI/ChatView.swift` the way
/// `ControlTokenLintTests` does, and skip when the tree is not beside the test bundle.
final class GlassMaterialNotFillTests: XCTestCase {

    // MARK: - The spec

    /// A chip floating over the transcript is clear glass on dark Glass, like the pills.
    func testTranscriptChipsAreClearGlassOnDarkGlass() {
        let liquid = ChatHeaderGlass.transcriptChipGlass(surface: .glass, isDark: true, liquidGlassAvailable: true)
        XCTAssertEqual(liquid.material, .liquidGlass)
        XCTAssertTrue(liquid.clear, "Liquid Glass .clear — the bubble under the chip must read")
        XCTAssertFalse(liquid.tinted, "a tint over the material is a fill by another name")

        let frost = ChatHeaderGlass.transcriptChipGlass(surface: .glass, isDark: true, liquidGlassAvailable: false)
        XCTAssertEqual(frost.material, .frosted, "plain frost before iOS 26")
        XCTAssertTrue(frost.clear)
        XCTAssertFalse(frost.tinted)
    }

    /// Solid mode is untouched, and the light appearance keeps its solid chip until Jacob
    /// signs the light glass off. Both are hard constraints, not preferences.
    func testMatteAndLightStaySolid() {
        for dark in [true, false] {
            XCTAssertEqual(ChatHeaderGlass.transcriptChipGlass(surface: .matte, isDark: dark).material, .solid,
                           "Matte is solid on both appearances")
        }
        let light = ChatHeaderGlass.transcriptChipGlass(surface: .glass, isDark: false, liquidGlassAvailable: true)
        XCTAssertEqual(light.material, .solid, "light Glass is unchanged pending sign-off")
        XCTAssertFalse(light.clear)
    }

    // MARK: - The views still paint a material

    /// Every view that is supposed to paint glass still names a real system material. Swap
    /// `.ultraThinMaterial` / `.glassEffect` for a `Color` and this fails, whatever the spec says.
    func testGlassViewsStillNameASystemMaterial() throws {
        let theme = try source("Theme.swift")
        for name in ["struct HeaderGlassBackground", "struct PillSurface", "struct TranscriptChipSurface"] {
            let body = try XCTUnwrap(Self.region(named: name, in: theme), "\(name) is gone from Theme.swift")
            XCTAssertTrue(body.contains("ultraThinMaterial") || body.contains("glassEffect")
                              || body.contains("HeaderGlassBackground("),
                          "\(name) paints no system material any more — a fill cannot blur what is behind it")
        }
    }

    /// Nothing on the glass path flattens the view tree. `compositingGroup()` and
    /// `drawingGroup()` render the subtree off-screen first, so the material samples that
    /// private buffer instead of the transcript, and the pill goes flat with no fill in sight.
    func testGlassViewsDoNotFlattenTheTree() throws {
        let theme = try source("Theme.swift")
        for name in ["struct HeaderGlassBackground", "struct PillSurface", "struct TranscriptChipSurface",
                     "struct GlassCircle", "struct GlassPill"] {
            guard let body = Self.region(named: name, in: theme) else { continue }
            XCTAssertFalse(body.contains("compositingGroup()"), "\(name): compositingGroup breaks blur sampling")
            XCTAssertFalse(body.contains("drawingGroup()"), "\(name): drawingGroup breaks blur sampling")
        }
    }

    // MARK: - The call sites still route through the glass views

    /// The composer field and the transcript chips must take their background from the glass
    /// views. A crossfade or animation that swaps one for a shape with a `Color` fill — the
    /// usual way this breaks — fails here.
    func testComposerAndChipsRouteThroughTheGlassSurfaces() throws {
        let chat = try source("ChatView.swift")
        XCTAssertTrue(chat.contains("PillSurface(shape: fieldShape)"),
                      "the composer field no longer takes PillSurface — check for a Color fill in its place")
        XCTAssertTrue(chat.contains("TranscriptChipSurface(shape: Circle())"),
                      "the scroll-to-bottom chip no longer takes TranscriptChipSurface")
        XCTAssertTrue(chat.contains("TranscriptChipSurface(shape: Capsule())"),
                      "the New messages pill no longer takes TranscriptChipSurface")
    }

    // MARK: - Reading the tree

    private func source(_ file: String) throws -> String {
        // …/apps/ios/Arbos/ArbosTests/<this>.swift → …/apps/ios/Arbos/Arbos/UI/<file>
        let url = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("Arbos/UI").appendingPathComponent(file)
        try XCTSkipUnless(FileManager.default.fileExists(atPath: url.path),
                          "sources not beside the test bundle (CI without the tree)")
        return try String(contentsOf: url, encoding: .utf8)
    }

    /// The lines of one top-level declaration: from its `struct …` line to the next
    /// declaration that starts at column 0.
    static func region(named decl: String, in text: String) -> String? {
        let lines = text.components(separatedBy: "\n")
        guard let start = lines.firstIndex(where: { $0.hasPrefix(decl) }) else { return nil }
        let starters = ["struct ", "enum ", "extension ", "final class ", "class ", "protocol "]
        let end = lines[(start + 1)...].firstIndex { line in starters.contains { line.hasPrefix($0) } }
        return lines[start..<(end ?? lines.endIndex)].joined(separator: "\n")
    }
}
