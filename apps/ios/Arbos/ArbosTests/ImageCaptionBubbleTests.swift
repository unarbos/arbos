import SwiftUI
import XCTest
@testable import Arbos

/// A picture with a typed note is one photo bubble: picture on top, caption under it,
/// one rounded container — and the bubble is a text bubble in width. Jacob's report
/// ([feedback image-caption-bubble-ios-2026-10-25]): a member's portrait screenshot
/// fitted to 320 pt tall is ~150 pt wide, and the caption wrapped in that 150 pt
/// column, a word or two per line down the screen. The caption now wraps at the
/// bubble cap; a picture-only bubble is unchanged.
final class ImageCaptionBubbleTests: XCTestCase {
    /// iPhone 17 width minus `Theme.bubbleInset` — what `RowView.maxBubble` hands the bubble.
    private let cap: CGFloat = 393 - Theme.bubbleInset
    private let longCaption = "It's hard to explain, but I feel like your own sent texts could be improved a little. "
        + "It now seems like it's still waiting for connection before it gets sent out because it's faded. "
        + "Should be more colored imho @const"

    /// Jacob's case: a portrait phone screenshot (390 × 844, the fixture's `IMG_4301.PNG`)
    /// fits to 320 tall and well under half the cap wide.
    private var portrait: CGSize { ImageBubble.fit(CGSize(width: 390, height: 844), maxWidth: min(280, cap)) }
    private var landscape: CGSize { ImageBubble.fit(CGSize(width: 640, height: 400), maxWidth: min(280, cap)) }

    func testPortraitPictureIsASliverAndTheCaptionDoesNotFollowIt() {
        XCTAssertEqual(portrait.height, 320)
        XCTAssertLessThan(portrait.width, cap / 2, "the fitted portrait is the narrow column the caption used to wrap in")

        // A long caption wraps at the cap: the bubble is as wide as a text bubble, not the picture.
        let caption = wrappedCaption(longCaption, at: cap)
        let g = PhotoBubbleLayout.geometry(picture: portrait, caption: caption, cap: cap, pictureInset: ImageBubble.pictureInset)
        XCTAssertGreaterThan(g.size.width, cap * 0.85, "the caption column is the bubble cap, not the picture width")
        XCTAssertLessThanOrEqual(g.size.width, cap)
        XCTAssertGreaterThan(g.size.width, portrait.width * 1.8)
        // Picture on top (floating: centred, inset, own radius), caption under it.
        XCTAssertFalse(g.pictureFlush)
        XCTAssertEqual(g.tileHeight, portrait.height + ImageBubble.pictureInset)
        XCTAssertEqual(g.captionTop, g.tileHeight)
        XCTAssertEqual(g.size.height, g.tileHeight + caption.height)
        // At the cap the note is a handful of lines, not a column down the screen.
        let lines = (caption.height - 2 * Theme.bubbleVPad) / lineHeight
        XCTAssertLessThanOrEqual(lines.rounded(), 7, "≤ 7 lines at the bubble cap (was ~17 in the 150 pt column)")
    }

    /// The real SwiftUI layout agrees with the arithmetic: a hosted photo bubble with the
    /// portrait sliver and the long note measures as wide as a text bubble.
    @MainActor
    func testHostedPhotoBubbleTakesTheBubbleCap() {
        let picture = portrait
        let bubble = PhotoBubbleLayout(maxWidth: cap, pictureInset: ImageBubble.pictureInset) {
            Color.gray.frame(width: picture.width, height: picture.height)
            Text(longCaption)
                .font(.system(size: Theme.body))
                .lineSpacing(Theme.bodyLineSpacing)
                .padding(.horizontal, Theme.bubbleHPad)
                .padding(.vertical, Theme.bubbleVPad)
        }
        let host = UIHostingController(rootView: bubble)
        let size = host.sizeThatFits(in: CGSize(width: 393, height: 4000))
        XCTAssertGreaterThan(size.width, cap * 0.85, "hosted: the bubble is a text bubble's width")
        XCTAssertLessThanOrEqual(size.width, cap + 0.5)
        XCTAssertLessThan(size.height, picture.height + ImageBubble.pictureInset + 7.5 * lineHeight + 2 * Theme.bubbleVPad,
                          "hosted: the caption is a few lines (6 on an iPhone 17), not a column")

        // Picture only: the bubble is exactly the picture.
        let alone = UIHostingController(rootView: PhotoBubbleLayout(maxWidth: cap, pictureInset: ImageBubble.pictureInset) {
            Color.gray.frame(width: picture.width, height: picture.height)
        })
        XCTAssertEqual(alone.sizeThatFits(in: CGSize(width: 393, height: 4000)), picture)
    }

    /// A landscape picture already spans the bubble: flush, the caption wraps under it at
    /// the picture's width (which is the bubble's), exactly as before.
    func testLandscapePictureStaysFlushWithItsCaptionUnderIt() {
        XCTAssertEqual(landscape.width, min(280, cap))
        let caption = wrappedCaption(longCaption, at: landscape.width)
        let g = PhotoBubbleLayout.geometry(picture: landscape, caption: caption, cap: cap, pictureInset: ImageBubble.pictureInset)
        XCTAssertEqual(g.size.width, landscape.width)
        XCTAssertTrue(g.pictureFlush)
        XCTAssertEqual(g.tileHeight, landscape.height)
        XCTAssertEqual(g.size.height, landscape.height + caption.height)
    }

    /// A short note never shrinks the bubble below the picture; a picture-only bubble is
    /// the picture (no regression for bubbles without a note).
    func testShortCaptionAndNoCaptionKeepThePictureFlush() {
        let short = wrappedCaption("Lol", at: cap)
        for picture in [portrait, landscape] {
            let g = PhotoBubbleLayout.geometry(picture: picture, caption: short, cap: cap, pictureInset: ImageBubble.pictureInset)
            XCTAssertEqual(g.size.width, picture.width)
            XCTAssertTrue(g.pictureFlush)
            XCTAssertEqual(g.tileHeight, picture.height)

            let alone = PhotoBubbleLayout.geometry(picture: picture, caption: nil, cap: cap, pictureInset: ImageBubble.pictureInset)
            XCTAssertEqual(alone.size, picture)
            XCTAssertTrue(alone.pictureFlush)
        }
    }

    /// A tiny row (split view, huge text) caps the bubble at what the row offers.
    func testTheRowCapWins() {
        let caption = wrappedCaption(longCaption, at: 200)
        let g = PhotoBubbleLayout.geometry(picture: portrait, caption: caption, cap: 200, pictureInset: ImageBubble.pictureInset)
        XCTAssertEqual(g.size.width, 200)
    }

    func testCaptionTextAndRadii() {
        XCTAssertNil(ImageBubble.captionText(nil))
        XCTAssertNil(ImageBubble.captionText("  \n"))
        XCTAssertEqual(ImageBubble.captionText("  Deck for Friday  "), "Deck for Friday")
        XCTAssertEqual(ImageBubble.pictureRadius, Theme.bubbleRadius - ImageBubble.pictureInset)
        XCTAssertTrue(PhotoBubbleLayout.pictureIsFlush(picture: CGSize(width: 148, height: 320), bubbleWidth: 148))
        XCTAssertFalse(PhotoBubbleLayout.pictureIsFlush(picture: CGSize(width: 148, height: 320), bubbleWidth: 149))
    }

    /// Video poster bubble size and duration label
    /// (feedback app-01M61H3VVBQZ6NCE708XB41NCC).
    func testVideoPosterBubbleSizeAndDurationLabel() throws {
        let landscape = try XCTUnwrap(VideoPoster.bubbleSize(width: 1280, height: 720, maxWidth: min(280, cap)))
        XCTAssertEqual(landscape, ImageBubble.fit(CGSize(width: 1280, height: 720), maxWidth: min(280, cap)))
        let portrait = try XCTUnwrap(VideoPoster.bubbleSize(width: 720, height: 1280, maxWidth: min(280, cap)))
        XCTAssertEqual(portrait, ImageBubble.fit(CGSize(width: 720, height: 1280), maxWidth: min(280, cap)))
        XCTAssertNil(VideoPoster.bubbleSize(width: 0, height: 720, maxWidth: 280))
        XCTAssertNil(VideoPoster.bubbleSize(width: 1280, height: 0, maxWidth: 280))
        XCTAssertEqual(VideoPoster.durationLabel(ms: 7_000), "0:07")
        XCTAssertEqual(VideoPoster.durationLabel(ms: 65_000), "1:05")
        XCTAssertEqual(VideoPoster.durationLabel(ms: 3_903_000), "1:05:03")
    }

    /// One body line as laid out: the system font's line height plus the bubble's line spacing.
    private var lineHeight: CGFloat { UIFont.systemFont(ofSize: Theme.body).lineHeight + Theme.bodyLineSpacing }

    /// The caption's wrapped size at a bubble width, padding included — UIKit's measure of
    /// the same system font the bubble uses (`Theme.body`, text scale 1).
    private func wrappedCaption(_ text: String, at bubbleWidth: CGFloat) -> CGSize {
        let style = NSMutableParagraphStyle()
        style.lineSpacing = Theme.bodyLineSpacing
        let rect = (text as NSString).boundingRect(
            with: CGSize(width: bubbleWidth - 2 * Theme.bubbleHPad, height: .greatestFiniteMagnitude),
            options: [.usesLineFragmentOrigin, .usesFontLeading],
            attributes: [.font: UIFont.systemFont(ofSize: Theme.body), .paragraphStyle: style], context: nil)
        return CGSize(width: ceil(rect.width) + 2 * Theme.bubbleHPad, height: ceil(rect.height) + 2 * Theme.bubbleVPad)
    }
}
