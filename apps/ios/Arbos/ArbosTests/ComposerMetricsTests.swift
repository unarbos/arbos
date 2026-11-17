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
        // One pill, not a disc beside a field: the `+` is inside the field now, so the
        // field runs the whole width between the insets — 420 − 18 − 18 = 384 pt — and
        // there is no disc width and no gap to subtract.
        XCTAssertEqual(Theme.composerHeight, 48)
        XCTAssertEqual(Theme.composerInsetKeyboard, 18)
        XCTAssertEqual(Theme.composerInset, 31, "keyboard down keeps the 2026-10-15 reading")
        let fieldWidth = 420 - Theme.composerInsetKeyboard * 2
        XCTAssertEqual(fieldWidth, 384)
        // Both ends of the pill are now the same 96 × 96 disc, each inset 26 px from its
        // own edge — the white action went round in the same change, so this reads as a
        // pair rather than as a disc facing a capsule.
        XCTAssertEqual(Theme.composerDisc, Theme.actionPillHeight)
        XCTAssertEqual(Theme.composerDisc, Theme.actionPillWidth)
        XCTAssertEqual(Theme.composerPlusLeading, Theme.actionPillTrailing)
        assertOnPixel(Theme.composerPlusLeading, "composerPlusLeading")
        // Since [feedback iphone-header-composer-sizing-2026-10-26] every other field, pill
        // and disc is the composer's size (`Controls`, `ControlMetricsTests`).
        XCTAssertEqual(Theme.fieldHeight, Theme.composerHeight)
    }

    func testPlusGlyphIsGroksSize() {
        // Measured off the reference at 3 px/pt: its `+` draws 38 px arms where this
        // build drew 54, so the glyph comes down with the disc it sits in. SF `plus`,
        // regular: 16 pt draws 38.
        XCTAssertEqual(Theme.composerPlusGlyph, 16)
    }

    func testFieldTextStartsAtGroksCaret() {
        // Full-width caret centre 51 px from the pill's outer edge; one 22 pt line centred in 48.
        XCTAssertEqual(Theme.composerTextInset, 17)
        XCTAssertEqual(Theme.composerTextPadY, 13)
        XCTAssertEqual(MentionTextView.insets.left, Theme.composerTextInset)
        XCTAssertEqual(MentionTextView.insets.top + 22 + MentionTextView.insets.bottom, Theme.composerHeight)
    }

    func testAskPlaceholderSitsBesideThePlus() {
        // When the `+` shares the row, a second 17 pt pad made "Ask …" read centred
        // ([feedback AJuVf-Tv2c-WhHzsKiVmi9U], Jacob 2026-11-10: "Ask Bob should be left
        // aligned here in the chat bar"). The HStack gap is the only left space; full-width
        // rows (channel / stacked) still keep Grok's 17.
        XCTAssertEqual(Theme.composerTextInsetBesidePlus, 0)
        XCTAssertEqual(MentionTextView.textInsets(left: Theme.composerTextInsetBesidePlus).left, 0)
        XCTAssertEqual(MentionTextView.textInsets(left: Theme.composerTextInset).left, Theme.composerTextInset)
        XCTAssertEqual(MentionTextView.textInsets(left: Theme.composerTextInsetBesidePlus).top,
                       MentionTextView.insets.top)
        XCTAssertEqual(MentionTextView.textInsets(left: Theme.composerTextInsetBesidePlus).bottom,
                       MentionTextView.insets.bottom)
    }

    func testActionIsAWhiteCircleNotACapsule() {
        // 96 × 96 px, 24 px from the field's top and bottom, 26 px from its right edge.
        // Measured off the reference at 3 px/pt: 96 × 96 there against 120 × 96 here.
        XCTAssertEqual(Theme.actionPillWidth, 32)
        XCTAssertEqual(Theme.actionPillHeight, 32)
        XCTAssertEqual(Theme.actionPillWidth, Theme.actionPillHeight, "round, not a capsule")
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
        // The five bars span 15 pt and 14.5 pt — inside the 32 pt circle with room to spare.
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

    // MARK: The trailing swap ([feedback composer-send-crossfade-2026-10-30])

    /// Jacob's one hard rule: typing trades the grey mic + white call pill for
    /// the send arrow, and the text field must not move or resize while it
    /// happens. Before the swap was animated the mic was inside an `if`, so the
    /// field grew by a 44 pt frame and the 2 pt gap the moment a character
    /// landed. The mic keeps its frame now, so the trailing cluster is one
    /// width whatever the field holds.
    func testTheTrailingControlsTakeOneWidthWhateverTheFieldHolds() {
        // 132 + 6 + 96 + 26 px: the mic's hit frame, the gap, the circle, the inset.
        // 284 until the action went round and took 24 px off the cluster.
        XCTAssertEqual(Theme.composerTrailingWidth * 3, 260, accuracy: 0.001)
        XCTAssertEqual(
            Theme.composerTrailingWidth,
            Theme.micHitWidth + Theme.micPillGap + Theme.actionPillWidth + Theme.actionPillTrailing)
        // `scaleEffect` does not change a view's layout size, so the mic's slot
        // is this wide at every point of the swap, not only at its two ends.
        XCTAssertGreaterThan(Theme.composerSwapScale, 0)
        XCTAssertLessThan(Theme.composerSwapScale, 1)
    }

    /// Quick, and inside the 150–200 ms band: fast enough that the arrow being
    /// tappable from the first frame is not something the eye has to wait for.
    func testTheSwapIsQuick() {
        XCTAssertGreaterThanOrEqual(Theme.composerSwapDuration, 0.15)
        XCTAssertLessThanOrEqual(Theme.composerSwapDuration, 0.2)
    }

    /// Typing fast, deleting fast or clearing the field mid-swap must never
    /// leave the trailing slot without a control that answers a tap.
    ///
    /// This used to say something narrower: that the mic may leave, but only
    /// once the mark beside it had become the send arrow. Jacob's comparison
    /// against Grok on 2026-11-09 retired that rule — the mic now never leaves
    /// at all, so the slot's safety no longer depends on the handover being
    /// ordered correctly. What is left to check is that the arrow still shows
    /// exactly when it should, and that the mic is beside it in every state.
    func testTheMicIsBesideTheArrowInEveryState() {
        for draftEmpty in [true, false] {
            for recording in [true, false] {
                for isChannel in [true, false] {
                    let state = "empty \(draftEmpty) recording \(recording) channel \(isChannel)"
                    XCTAssertTrue(ComposerTrailing.micShown(), "the mic left: \(state)")
                    // An empty draft cannot send; a non-empty one always can.
                    let send = ComposerTrailing.sendShown(
                        canSend: !draftEmpty, recording: recording, isChannel: isChannel)
                    // The arrow is there for anything sendable, while a recording is being
                    // finished, and in a server channel, which has no call to offer (C1) and
                    // so shows it dimmed rather than showing nothing.
                    XCTAssertEqual(send, !draftEmpty || recording || isChannel,
                                   "the arrow is in the wrong state: \(state)")
                }
            }
        }
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

    /// [feedback status-row-gap-ios-2026-10-26]: the picture sits Grok's 42 px under the
    /// last bubble (the same 2026-10-25 shot: bubble bottom → picture 42 px, picture →
    /// composer 68 px, bubble → composer 187 px).
    ///
    /// [feedback typing-anchor-2026-11-08] moved the row out of the bar and into the
    /// transcript, so it settles *into* the tail rather than rising out of it. The two
    /// offsets are mirror images — `presenceTopGap` against `presenceBottomGap` — and
    /// every distance a reader can see is unchanged.
    func testStatusRowSettlesIntoTheTranscriptTail() {
        XCTAssertEqual(Theme.presenceTopGap * 3, 42, accuracy: 0.001)
        XCTAssertEqual(Theme.transcriptTail, 28)
        XCTAssertEqual(Theme.chatBarTopPad, 8)
        // Picture bottom → composer top = tail + pad − settle, Grok's 68 px.
        XCTAssertEqual(Theme.transcriptTail + Theme.chatBarTopPad - Theme.presenceSettle,
                       Theme.presenceBottomGap, accuracy: 0.001)
        // The row settles no further than the tail reserves, and never lifts off it.
        XCTAssertLessThanOrEqual(Theme.presenceSettle, Theme.transcriptTail)
        XCTAssertGreaterThanOrEqual(Theme.presenceSettle, 0)
        // Bubble → composer with the row showing: Grok's 187 px (icon crop ± 2 px).
        XCTAssertEqual((Theme.presenceTopGap + Theme.presenceAvatar + Theme.presenceBottomGap) * 3, 187, accuracy: 3)
    }

    /// The row left the bar for the transcript, and the reader must not be able to tell
    /// from anything but where it stops scrolling: the transcript's content is the same
    /// height either way, so a turn starting while someone is up in history cannot move
    /// the rows they are reading.
    func testMovingTheRowIntoTheTranscriptKeptEveryHeight() {
        // What the row used to add to the bar, and so to the tail that follows the bar.
        let addedToTheBarBefore = Theme.presenceAvatar + Theme.presenceBottomGap
            - (Theme.transcriptTail + Theme.chatBarTopPad - Theme.presenceTopGap)
        // What it adds to the transcript now: its own box, less the settle into the tail.
        let addedToTheTranscriptNow = Theme.presenceTopGap + Theme.presenceAvatar - Theme.presenceSettle
        XCTAssertEqual(addedToTheBarBefore, 26, accuracy: 0.001)
        XCTAssertEqual(addedToTheTranscriptNow, addedToTheBarBefore, accuracy: 0.001,
                       "the row appearing must change the scroll extent by what it always did")
    }

    func testEveryMetricLandsOnAPixel() {
        for (name, value) in [
            ("plusButton", Theme.plusButton), ("composerHeight", Theme.composerHeight),
            ("plusFieldGap", Theme.plusFieldGap), ("composerInsetKeyboard", Theme.composerInsetKeyboard),
            ("composerTextInset", Theme.composerTextInset),
            ("composerTextInsetBesidePlus", Theme.composerTextInsetBesidePlus),
            ("composerTextPadY", Theme.composerTextPadY),
            ("actionPillWidth", Theme.actionPillWidth), ("actionPillHeight", Theme.actionPillHeight),
            ("actionPillTrailing", Theme.actionPillTrailing),
            ("composerTrailingWidth", Theme.composerTrailingWidth),
            ("micGlyphWidth", Theme.micGlyphWidth),
            ("micGlyphHeight", Theme.micGlyphHeight), ("composerBottomKeyboard", Theme.composerBottomKeyboard),
            ("presenceAvatar", Theme.presenceAvatar), ("presenceAvatarOffset", Theme.presenceAvatarOffset),
            ("presenceGap", Theme.presenceGap), ("presenceBottomGap", Theme.presenceBottomGap),
            ("presenceTopGap", Theme.presenceTopGap), ("presenceSettle", Theme.presenceSettle),
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

    // Tester report, 2026-11-17: crop or trim a staged attachment before sending it.
    func testAnEditedFileTakesTheOriginalsPlaceAndCancelKeepsIt() {
        let trio = pendingTrio()
        let edited = AppModel.PendingAttachment(name: "portrait.jpg", mime: "image/jpeg", data: Data([9]))
        let after = ComposerAttachment.replacing(trio[1].id, with: edited, in: trio)
        XCTAssertEqual(after.map(\.name), ["profile-card.png", "portrait.jpg", "notes.txt"], "same place in the row")
        XCTAssertEqual(after[1].data, Data([9]))
        // Cancel never calls `replacing`; an attachment removed while its editor was open stays removed.
        XCTAssertEqual(ComposerAttachment.replacing(UUID(), with: edited, in: trio), trio)
        XCTAssertTrue(ComposerAttachment.canEdit(trio[0]))
        XCTAssertFalse(ComposerAttachment.canEdit(trio[2]), "a text file has nothing to crop")
        XCTAssertFalse(ComposerAttachment.canEdit(.init(name: "a.gif", mime: "image/gif", data: Data())))
        XCTAssertTrue(ComposerAttachment.canEdit(.init(name: "a.mov", mime: "video/quicktime", data: Data())))
    }

    func testCropRectangleStaysInsideThePictureAndSquareIsSquareInPixels() {
        let full = CGRect(x: 0, y: 0, width: 1, height: 1)
        // Bottom-right corner pulled in: the top-left corner does not move.
        let pulled = PhotoCrop.dragging(full, right: true, bottom: true, by: CGSize(width: -0.25, height: -0.5), square: false, aspect: 2)
        XCTAssertEqual(pulled, CGRect(x: 0, y: 0, width: 0.75, height: 0.5))
        // Pushed past the edge, and past the opposite corner: clamped, never inverted.
        let pushed = PhotoCrop.dragging(pulled, right: true, bottom: true, by: CGSize(width: 5, height: -5), square: false, aspect: 2)
        XCTAssertEqual(pushed.maxX, 1, accuracy: 1e-9)
        XCTAssertEqual(pushed.height, PhotoCrop.minSide, accuracy: 1e-9)
        // A 2:1 picture: a square covers half as much of the width as of the height.
        let square = PhotoCrop.squared(full, aspect: 2)
        XCTAssertEqual(square, CGRect(x: 0.25, y: 0, width: 0.5, height: 1))
        let dragged = PhotoCrop.dragging(square, right: false, bottom: false, by: CGSize(width: 0.1, height: 0.1), square: true, aspect: 2)
        XCTAssertEqual(dragged.width * 2, dragged.height, accuracy: 1e-9)
        XCTAssertEqual(dragged.maxX, square.maxX, accuracy: 1e-9)
        XCTAssertEqual(PhotoCrop.moving(square, by: CGSize(width: 3, height: 3)).origin, CGPoint(x: 0.5, y: 0))
        let image = UIGraphicsImageRenderer(size: CGSize(width: 200, height: 100)).image { _ in }
        XCTAssertEqual(PhotoCrop.cropped(image, to: square).size, CGSize(width: 100, height: 100))
    }

    // Tester report, 2026-11-17: a video shows its first frame, Play and its length.
    func testVideoPosterKeyFallbackAndLength() async throws {
        XCTAssertEqual(VideoThumb.cacheRef("att-1-clip.mp4"), "poster:att-1-clip.mp4", "never the video's own cache entry")
        XCTAssertTrue(VideoThumb.wantsPoster(kind: .video, blobRef: "att-1-clip.mp4"))
        XCTAssertFalse(VideoThumb.wantsPoster(kind: .video, blobRef: "local:clip.mp4"), "still uploading: the card")
        XCTAssertFalse(VideoThumb.wantsPoster(kind: .video, blobRef: nil))
        XCTAssertFalse(VideoThumb.wantsPoster(kind: .file, blobRef: "att-1-notes.pdf"))
        XCTAssertEqual(VideoThumb.durationLabel(7.2), "0:07")
        XCTAssertEqual(VideoThumb.durationLabel(3903), "1:05:03")
        XCTAssertEqual(VideoThumb.durationLabel(.nan), "")
        XCTAssertEqual(VideoThumb.durationLabel(.infinity), "")
        XCTAssertEqual(VideoThumb.durationLabel(1e300), "99:59:59")

        let clip = try XCTUnwrap(FixtureMedia.clip)
        let box = CGSize(width: 160, height: 160)
        let made = await VideoThumb.frame(from: clip, name: "clip.mp4", maxPixels: box)
        let frame = try XCTUnwrap(made)
        XCTAssertEqual(frame.image.size, CGSize(width: 160, height: 90), "scaled into the box at the video's ratio")
        XCTAssertEqual(frame.seconds, 2, accuracy: 0.1)
        // What decides is the content: a video under a wrong name still has a frame, and
        // bytes that are not a video have none whatever they are called.
        let misnamed = await VideoThumb.frame(from: clip, name: "notes.txt", maxPixels: box)
        XCTAssertNotNil(misnamed)
        let junk = await VideoThumb.frame(from: Data("not a video".utf8), name: "clip.mp4", maxPixels: box)
        XCTAssertNil(junk)
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

/// A closed chat frees its composer draft. The draft's callbacks capture the chat view,
/// whose `@State` owns the draft; while they stayed set, every open leaked the draft and,
/// through the view, that chat's rows and messages (about 4 MB a group open, Pass 3).
@MainActor
final class ComposerDraftLifetimeTests: XCTestCase {
    private final class DraftReference {
        weak var value: ComposerDraft?
        init(_ draft: ComposerDraft) { value = draft }
    }

    private func references(excluding existing: Set<ObjectIdentifier>) -> [DraftReference] {
        ComposerDraft.liveInstances.filter { !existing.contains(ObjectIdentifier($0)) }.map(DraftReference.init)
    }

    private func released(_ drafts: [DraftReference]) -> Bool {
        drafts.allSatisfy { $0.value == nil }
    }

    private func settle() {
        RunLoop.main.run(until: Date().addingTimeInterval(0.3))
    }

    func testAClosedChatFreesItsDraft() {
        let model = AppModel()
        settle()
        let existing = ComposerDraft.liveInstances
        let existingIds = Set(existing.map(ObjectIdentifier.init))
        var hosted: [DraftReference] = []
        // Another view may release an earlier draft during our run-loop turns.
        // Keep those identities alive so only this host's new drafts are observed.
        withExtendedLifetime(existing) {
            autoreleasepool {
                let window = UIWindow(frame: CGRect(x: 0, y: 0, width: 390, height: 844))
                let host = UIHostingController(rootView: AnyView(
                    ChatView(chatId: "draft-lifetime-chat", projectId: "draft-lifetime-project")
                        .environmentObject(model).environment(\.theme, Theme.dark)))
                window.rootViewController = host
                window.makeKeyAndVisible()
                settle()
                hosted = references(excluding: existingIds)
                XCTAssertFalse(hosted.isEmpty, "the open chat holds a draft")
                XCTAssertTrue(hosted.allSatisfy { $0.value != nil }, "the hosted draft is alive while open")
                host.rootView = AnyView(EmptyView())
                settle()
                window.isHidden = true
                window.rootViewController = nil
            }
            settle()
            XCTAssertTrue(released(hosted), "the closed chat's draft is still alive")
        }
    }

    func testUnrelatedDraftReleaseCannotFailTheInstanceObservation() {
        let existing = ComposerDraft.liveInstances
        withExtendedLifetime(existing) {
            var unrelated: ComposerDraft? = ComposerDraft()
            let unrelatedReference = DraftReference(unrelated!)
            let existingIds = Set(existing.map(ObjectIdentifier.init)).union([ObjectIdentifier(unrelated!)])
            var observed: [DraftReference] = []
            autoreleasepool {
                let draft = ComposerDraft()
                observed = references(excluding: existingIds)
                XCTAssertEqual(observed.count, 1)
                XCTAssertTrue(observed.first?.value === draft)
            }
            XCTAssertNotNil(unrelatedReference.value)
            unrelated = nil
            XCTAssertNil(unrelatedReference.value)
            XCTAssertTrue(released(observed), "an unrelated release cannot make a freed draft look retained")
        }
    }

    func testRetainedDraftIsDetectedWhenAnUnrelatedDraftFrees() {
        let existing = ComposerDraft.liveInstances
        withExtendedLifetime(existing) {
            var unrelated: ComposerDraft? = ComposerDraft()
            let unrelatedReference = DraftReference(unrelated!)
            let existingIds = Set(existing.map(ObjectIdentifier.init)).union([ObjectIdentifier(unrelated!)])
            var retained: ComposerDraft?
            var observed: [DraftReference] = []
            autoreleasepool {
                retained = ComposerDraft()
                observed = references(excluding: existingIds)
                XCTAssertEqual(observed.count, 1)
            }
            XCTAssertNotNil(unrelatedReference.value)
            unrelated = nil
            XCTAssertNil(unrelatedReference.value)
            XCTAssertFalse(released(observed), "an unrelated release cannot hide the retained owned draft")
            XCTAssertTrue(observed.first?.value === retained)
            retained = nil
            XCTAssertTrue(released(observed), "the observer itself must not retain the draft")
        }
    }
}
