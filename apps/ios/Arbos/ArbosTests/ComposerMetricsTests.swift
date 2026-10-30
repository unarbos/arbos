import SwiftUI
import XCTest
@testable import Arbos

/// The chat composer and the status row over it match Grok Bot's within 1 px
/// ([feedback iphone-composer-grok-parity-2026-10-25], Jacob's 2026-10-25 shot:
/// iPhone Air, 420 pt, 3 px/pt, keyboard up). Every number here is that shot's pixel
/// count / 3; the same constants lay the composer out in every chat kind (agent 1:1,
/// DM, group, server channel, job tab), so a drift shows up here before a shot.
final class ComposerMetricsTests: XCTestCase {
    /// 3 px/pt: a metric is "on the pixel" when it is a whole number of thirds.
    private func assertOnPixel(_ value: CGFloat, _ name: String, file: StaticString = #filePath, line: UInt = #line) {
        let px = value * 3
        XCTAssertEqual(px, px.rounded(), accuracy: 0.001, "\(name) is not a whole pixel at 3x", file: file, line: line)
    }

    func testDiscFieldAndInsetsAreGroksKeyboardUp() {
        // `+` disc 144 px at 54 px from the edge; field 144 px tall; 30 px between them;
        // the field ends 54 px from the right edge. 420 − 18 − 48 − 10 − 18 = 326 pt field.
        XCTAssertEqual(Theme.plusButton, 48)
        XCTAssertEqual(Theme.composerHeight, 48)
        XCTAssertEqual(Theme.plusFieldGap, 10)
        XCTAssertEqual(Theme.composerInsetKeyboard, 18)
        XCTAssertEqual(Theme.composerInset, 31, "keyboard down keeps the 2026-10-15 reading")
        let fieldWidth = 420 - Theme.composerInsetKeyboard * 2 - Theme.plusButton - Theme.plusFieldGap
        XCTAssertEqual(fieldWidth, 326)
        // Since [feedback iphone-header-composer-sizing-2026-10-26] every other field, pill
        // and disc is the composer's size (`Controls`, `ControlMetricsTests`).
        XCTAssertEqual(Theme.fieldHeight, Theme.composerHeight)
    }

    func testPlusGlyphIsGroksSize() {
        // SF `plus`, regular: 54 px arms (22 pt draws 53, 22.5 draws 54), 5 px strokes
        // like Grok's — medium would draw 8.
        XCTAssertEqual(Theme.plusGlyph, 22.5)
    }

    func testFieldTextStartsAtGroksCaret() {
        // Caret centre 51 px from the field's outer edge; one 22 pt line centred in 48.
        XCTAssertEqual(Theme.composerTextInset, 17)
        XCTAssertEqual(Theme.composerTextPadY, 13)
        XCTAssertEqual(MentionTextView.insets.left, Theme.composerTextInset)
        XCTAssertEqual(MentionTextView.insets.top + 22 + MentionTextView.insets.bottom, Theme.composerHeight)
    }

    func testActionIsAWhitePillNotACircle() {
        // 120 × 96 px capsule, 24 px from the field's top and bottom, 26 px from its right edge.
        XCTAssertEqual(Theme.actionPillWidth, 40)
        XCTAssertEqual(Theme.actionPillHeight, 32)
        XCTAssertEqual((Theme.composerHeight - Theme.actionPillHeight) / 2, 8)
        XCTAssertEqual(Theme.actionPillTrailing * 3, 26, accuracy: 0.001)
        assertOnPixel(Theme.actionPillTrailing, "actionPillTrailing")
    }

    func testVoiceWaveGlyphIsGroksFiveBars() {
        // Bars 6 px wide, 9.75 px apart; 19.5 / 43.5 / 25.5 / 43.5 / 19.5 px tall; symmetric.
        XCTAssertEqual(VoiceWaveGlyph.barWidth, 2)
        XCTAssertEqual(VoiceWaveGlyph.pitch, 3.25)
        XCTAssertEqual(VoiceWaveGlyph.heights, [6.5, 14.5, 8.5, 14.5, 6.5])
        XCTAssertEqual(VoiceWaveGlyph.heights, Array(VoiceWaveGlyph.heights.reversed()))
        // The five bars span 15 pt and 14.5 pt — inside the 40 × 32 pill with room to spare.
        let span = VoiceWaveGlyph.pitch * 4 + VoiceWaveGlyph.barWidth
        XCTAssertEqual(span, 15)
        XCTAssertLessThan(span, Theme.actionPillWidth)
        XCTAssertLessThan(VoiceWaveGlyph.heights.max()!, Theme.actionPillHeight)
        // Drawn centred: the path's bounds sit on the rect's centre.
        let rect = CGRect(x: 0, y: 0, width: Theme.actionPillWidth, height: Theme.actionPillHeight)
        let bounds = VoiceWaveGlyph().path(in: rect).boundingRect
        XCTAssertEqual(bounds.midX, rect.midX, accuracy: 0.01)
        XCTAssertEqual(bounds.midY, rect.midY, accuracy: 0.01)
        XCTAssertEqual(bounds.width, span, accuracy: 0.01)
        XCTAssertEqual(bounds.height, 14.5, accuracy: 0.01)
    }

    func testMicGlyphIsGroksFilledMicWithoutABase() {
        // Box 36 × 49 px; head 17 × 31 px; cradle 36 px wide with a 4.5 px stroke; the
        // glyph's centre sits 72 px before the pill (22 pt hit half-width + 2 pt).
        XCTAssertEqual(Theme.micGlyphWidth, 12)
        XCTAssertEqual(Theme.micGlyphHeight * 3, 49, accuracy: 0.001)
        XCTAssertEqual(MicGlyph.headWidth * 3, 17, accuracy: 0.001)
        XCTAssertEqual(MicGlyph.headHeight * 3, 31, accuracy: 0.001)
        XCTAssertEqual(MicGlyph.cradleStroke, 1.5)
        XCTAssertEqual(MicGlyph.cradleArmTop * 3, 16, accuracy: 0.001)
        XCTAssertEqual(MicGlyph.cradleCenterY * 3, 23, accuracy: 0.001)
        XCTAssertEqual(MicGlyph.stemBottom, Theme.micGlyphHeight)
        XCTAssertEqual(Theme.micHitWidth / 2 + Theme.micPillGap, 24)
        XCTAssertGreaterThanOrEqual(Theme.micHitWidth, 44, "touch target")
        // The path fills exactly the box: no base bar below the stem.
        let rect = CGRect(origin: .zero, size: MicGlyph.size)
        let bounds = MicGlyph().path(in: rect).boundingRect
        XCTAssertEqual(bounds.minY, 0, accuracy: 0.01)
        XCTAssertEqual(bounds.maxY, Theme.micGlyphHeight, accuracy: 0.01)
        XCTAssertEqual(bounds.width, Theme.micGlyphWidth, accuracy: 0.01)
    }

    func testRimFadesInOverOnePoint() {
        // Three nested inner strokes — 1, 2/3, 1/3 pt — so the outer pixel is brightest
        // (58 / 47 / 40 luminance over #181818, Grok's edges top, bottom, left and right).
        let layers = Theme.composerRimLayers
        XCTAssertEqual(layers.map(\.width), [CGFloat(1), CGFloat(2) / 3, CGFloat(1) / 3])
        var lum: [Double] = []
        var value = 24.0  // #181818
        for layer in layers {
            value += layer.alpha * (255 - value)
            lum.append(value)
        }
        // Innermost pixel sees layer 0 only, the middle one layers 0+1, the outer all three.
        XCTAssertEqual(lum[0], 40, accuracy: 1)
        XCTAssertEqual(lum[1], 47, accuracy: 1)
        XCTAssertEqual(lum[2], 58, accuracy: 1)
    }

    func testBottomPaddingIsGroksWithTheKeyboardUp() {
        // 54 px between the field and the keyboard; 2 pt above the home indicator otherwise.
        XCTAssertEqual(Theme.composerBottomKeyboard, 18)
        XCTAssertEqual(Theme.composerBottom, 2)
    }

    func testStatusRowIsGroksPictureWordsAndGap() {
        // 76 px picture, 18 px inside the `+` disc's left edge (72 px from the screen with
        // the keyboard up), 42 px to the words, 17 pt body, 68 px to the composer's top.
        XCTAssertEqual(Theme.presenceAvatar * 3, 76, accuracy: 0.001)
        XCTAssertEqual(PresenceRow.avatar, Theme.presenceAvatar)
        XCTAssertEqual(PresenceRow.height, Theme.presenceAvatar, "the row is the picture tall; ChatView pads below it")
        XCTAssertEqual(Theme.presenceAvatarOffset, 6)
        XCTAssertEqual(16 + Theme.presenceInset(composerInset: Theme.composerInsetKeyboard), 24)
        XCTAssertEqual(16 + Theme.presenceInset(composerInset: Theme.composerInset), 37, "keyboard down: the same 6 pt inside the `+`")
        XCTAssertEqual(PresenceRow.gap, 14)
        XCTAssertEqual(16 + Theme.presenceInset(composerInset: Theme.composerInsetKeyboard) + Theme.presenceAvatar + PresenceRow.gap, 190 / 3, accuracy: 0.001, "words at 190 px")
        XCTAssertEqual(PresenceRow.textSize, Theme.body)
        XCTAssertEqual(Theme.presenceBottomGap * 3, 68, accuracy: 0.001)
        // The picture stays inside the `+` column with the keyboard up and down.
        for inset in [Theme.composerInset, Theme.composerInsetKeyboard] {
            let left = 16 + Theme.presenceInset(composerInset: inset)
            XCTAssertGreaterThanOrEqual(left, inset)
            XCTAssertLessThanOrEqual(left + Theme.presenceAvatar, inset + Theme.plusButton)
        }
        // The tasks chip (#1696) still fits on the row.
        XCTAssertLessThanOrEqual(PresenceRow.chipHeight, PresenceRow.height)
    }

    /// [feedback status-row-gap-ios-2026-10-26]: the row rises into the transcript's tail
    /// so the picture sits Grok's 42 px under the last bubble (the same 2026-10-25 shot:
    /// bubble bottom → picture 42 px, picture → composer 68 px, bubble → composer 187 px),
    /// instead of starting under the tail and the bar's pad (36 pt, 109 px).
    func testStatusRowRisesIntoTheTranscriptTail() {
        XCTAssertEqual(Theme.presenceTopGap * 3, 42, accuracy: 0.001)
        XCTAssertEqual(Theme.transcriptTail, 28)
        XCTAssertEqual(Theme.chatBarTopPad, 8)
        // Last bubble → picture top = tail + pad − lift.
        XCTAssertEqual(Theme.transcriptTail + Theme.chatBarTopPad - Theme.presenceLift, Theme.presenceTopGap, accuracy: 0.001)
        // The row never rises past the tail into the last bubble, and never sinks below the pad.
        XCTAssertLessThanOrEqual(Theme.presenceLift, Theme.transcriptTail)
        XCTAssertGreaterThanOrEqual(Theme.presenceLift, 0)
        // Bubble → composer with the row showing: Grok's 187 px (icon crop ± 2 px).
        XCTAssertEqual((Theme.presenceTopGap + Theme.presenceAvatar + Theme.presenceBottomGap) * 3, 187, accuracy: 3)
        // The bar grows by the row's height less the lift when the row appears.
        XCTAssertEqual(Theme.presenceAvatar + Theme.presenceBottomGap - Theme.presenceLift, 26, accuracy: 0.001)
    }

    func testEveryMetricLandsOnAPixel() {
        for (name, value) in [
            ("plusButton", Theme.plusButton), ("composerHeight", Theme.composerHeight),
            ("plusFieldGap", Theme.plusFieldGap), ("composerInsetKeyboard", Theme.composerInsetKeyboard),
            ("composerTextInset", Theme.composerTextInset), ("composerTextPadY", Theme.composerTextPadY),
            ("actionPillWidth", Theme.actionPillWidth), ("actionPillHeight", Theme.actionPillHeight),
            ("actionPillTrailing", Theme.actionPillTrailing), ("micGlyphWidth", Theme.micGlyphWidth),
            ("micGlyphHeight", Theme.micGlyphHeight), ("composerBottomKeyboard", Theme.composerBottomKeyboard),
            ("presenceAvatar", Theme.presenceAvatar), ("presenceAvatarOffset", Theme.presenceAvatarOffset),
            ("presenceGap", Theme.presenceGap), ("presenceBottomGap", Theme.presenceBottomGap),
            ("presenceTopGap", Theme.presenceTopGap), ("presenceLift", Theme.presenceLift),
        ] {
            assertOnPixel(value, name)
        }
    }

    // MARK: Pending attachments ([feedback composer-attachment-removal-ios-2026-10-27])

    private func pendingTrio() -> [AppModel.PendingAttachment] {
        [.init(name: "profile-card.png", mime: "image/png", data: Data([1])),
         .init(name: "portrait.png", mime: "image/png", data: Data([2])),
         .init(name: "notes.txt", mime: "text/plain", data: Data([3]))]
    }

    func testRemovingOnePendingAttachmentKeepsTheOthers() {
        let trio = pendingTrio()
        let left = ChatSession.removingPending(trio[1].id, from: trio)
        XCTAssertEqual(left.map(\.name), ["profile-card.png", "notes.txt"])
        XCTAssertEqual(ChatSession.removingPending(UUID(), from: trio).count, 3, "an unknown id removes nothing")
    }

    func testBackspaceOnAnEmptyDraftRemovesTheLastPendingAttachment() {
        let trio = pendingTrio()
        let two = ChatSession.removingLastPending(from: trio, draftEmpty: true)
        XCTAssertEqual(two.map(\.name), ["profile-card.png", "portrait.png"])
        XCTAssertEqual(ChatSession.removingLastPending(from: trio, draftEmpty: false).count, 3,
                       "with text in the draft the key edits text, never the strip")
        XCTAssertTrue(ChatSession.removingLastPending(from: [], draftEmpty: true).isEmpty)
    }

    func testPendingRemoveHitTargetIsAtLeastTwentyPoints() {
        XCTAssertGreaterThanOrEqual(Theme.pendingRemoveHit, 20)
        // The glyph keeps 10 pt from the capsule's edge whatever the hit frame.
        XCTAssertEqual(Theme.pendingChipTrailing + (Theme.pendingRemoveHit - Theme.pendingRemoveGlyph) / 2, 10)
        XCTAssertGreaterThanOrEqual(Theme.pendingChipTrailing, 0)
    }
}
