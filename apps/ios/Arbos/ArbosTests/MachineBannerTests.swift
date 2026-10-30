import UIKit
import XCTest
@testable import Arbos

/// The chat-level "Can't reach your machine" note ([feedback machine-banner-glass-2026-10-30],
/// Arbonites 2026-10-30: unreadable and undismissable in glass mode). Three mechanisms, each
/// asserted as a rule rather than as an appearance.
final class MachineBannerTests: XCTestCase {

    // MARK: 1. Readable — the material the note never had

    /// On dark Glass a note pinned over the transcript gets a material under its tint. Its own
    /// `dangerSoft` is a 14 % wash, so without one the sentence reads against live bubbles.
    func testPinnedNoteTakesAMaterialOnDarkGlass() {
        XCTAssertEqual(ChatHeaderGlass.overlayNoteMaterial(surface: .glass, isDark: true, liquidGlassAvailable: true),
                       .liquidGlass)
        XCTAssertEqual(ChatHeaderGlass.overlayNoteMaterial(surface: .glass, isDark: true, liquidGlassAvailable: false),
                       .frosted, "plain frost before iOS 26")
    }

    /// The tint is a wash, not a fill — which is why the material matters. If someone ever
    /// "fixes" the readability by making `dangerSoft` opaque instead, this fails.
    func testTheNotesOwnFillStaysAWashOnBothAppearances() {
        for theme in [Theme.dark, Theme.light] {
            var alpha: CGFloat = -1
            XCTAssertTrue(UIColor(theme.dangerSoft).getRed(nil, green: nil, blue: nil, alpha: &alpha))
            XCTAssertLessThan(alpha, 0.2, "dangerSoft is a wash; readability comes from the material under it")
        }
    }

    /// Solid mode is untouched, and the light appearance keeps the bare tint until Jacob rules
    /// on light glass. Both are hard constraints.
    func testMatteAndLightTakeNoMaterial() {
        for dark in [true, false] {
            XCTAssertNil(ChatHeaderGlass.overlayNoteMaterial(surface: .matte, isDark: dark),
                         "Matte already has an opaque page behind the note")
        }
        XCTAssertNil(ChatHeaderGlass.overlayNoteMaterial(surface: .glass, isDark: false),
                     "light Glass is unchanged pending sign-off")
    }

    // MARK: 2. Dismissable — closing one note must not hide the next

    func testDismissalIsKeyedToTheNoteNotTheSurface() {
        let first = ErrorNote.dismissKey(stateSince: 1_000, raw: "machine state: error")
        XCTAssertEqual(first, ErrorNote.dismissKey(stateSince: 1_000, raw: "machine state: error"),
                       "the same note stays closed")
        XCTAssertNotEqual(first, ErrorNote.dismissKey(stateSince: 2_000, raw: "machine state: error"),
                          "the machine moved and failed again — a new note, shown again")
        XCTAssertNotEqual(first, ErrorNote.dismissKey(stateSince: 1_000, raw: "relay refused: no_capacity"),
                          "a different failure is a different note")
        XCTAssertNotEqual(ErrorNote.dismissKey(stateSince: nil, raw: "a"),
                          ErrorNote.dismissKey(stateSince: nil, raw: "b"),
                          "a machine with no timestamp still distinguishes failures")
    }

    // MARK: 3. Auto-clearing — the state is sticky, so the link coming back re-asks

    /// `startMachine` / `restartMachine` write `.error` locally when their own call fails and
    /// nothing polls `/machine`, so the note outlived its cause. Only this combination re-asks.
    func testOnlyAReturningLinkWithTheMachineInErrorReAsks() {
        XCTAssertTrue(AppModel.shouldReAskMachine(wasDown: true, now: .live, machineState: .error))
        XCTAssertFalse(AppModel.shouldReAskMachine(wasDown: false, now: .live, machineState: .error),
                       "the link never went away; something else already owns this state")
        XCTAssertFalse(AppModel.shouldReAskMachine(wasDown: true, now: .reconnecting, machineState: .error),
                       "still coming back, not back")
        for state in [BoxState.awake, .waking, .hibernatedLight, .hibernatedDeep, .stoppedByUser] {
            XCTAssertFalse(AppModel.shouldReAskMachine(wasDown: true, now: .live, machineState: state),
                           "\(state): no note to clear, so no extra call")
        }
    }

    // MARK: The call sites still route through the three mechanisms

    func testTheChatNoteUsesAllThree() throws {
        let chat = try source("UI/ChatView.swift")
        XCTAssertTrue(chat.contains("overTranscript: true"),
                      "the chat-level note no longer asks for the material")
        XCTAssertTrue(chat.contains("onDismiss:"), "the chat-level note no longer has a ×")
        XCTAssertTrue(chat.contains("ErrorNote.dismissKey("), "dismissal is no longer keyed to the note")

        let note = try source("UI/ErrorNote.swift")
        XCTAssertTrue(note.contains("OverlayNoteSurface(shape: shape, tint: theme.dangerSoft)"),
                      "the pinned note no longer routes its background through OverlayNoteSurface")

        let theme = try source("UI/Theme.swift")
        let surface = try XCTUnwrap(Self.region(named: "struct OverlayNoteSurface", in: theme))
        XCTAssertTrue(surface.contains("HeaderGlassBackground("),
                      "OverlayNoteSurface paints no material — a tint alone cannot blur the transcript")
        XCTAssertFalse(surface.contains("compositingGroup()") || surface.contains("drawingGroup()"),
                       "flattening the subtree stops the material sampling what is behind it")
    }

    private func source(_ path: String) throws -> String {
        // …/apps/ios/Arbos/ArbosTests/<this>.swift → …/apps/ios/Arbos/Arbos/<path>
        let url = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("Arbos").appendingPathComponent(path)
        try XCTSkipUnless(FileManager.default.fileExists(atPath: url.path),
                          "sources not beside the test bundle (CI without the tree)")
        return try String(contentsOf: url, encoding: .utf8)
    }

    static func region(named decl: String, in text: String) -> String? {
        let lines = text.components(separatedBy: "\n")
        guard let start = lines.firstIndex(where: { $0.hasPrefix(decl) }) else { return nil }
        let starters = ["struct ", "enum ", "extension ", "final class ", "class ", "protocol "]
        let end = lines[(start + 1)...].firstIndex { line in starters.contains { line.hasPrefix($0) } }
        return lines[start..<(end ?? lines.endIndex)].joined(separator: "\n")
    }
}
