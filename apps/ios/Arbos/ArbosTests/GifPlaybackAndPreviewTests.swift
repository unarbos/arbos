import ImageIO
import UIKit
import XCTest
@testable import Arbos

/// A sent GIF, reported twice over on 2026-11-05
/// ([feedback gif-send-and-playback-2026-11-05]): it never animated on the iPhone, and when
/// it was the latest message the chat list showed the text message before it.
///
/// Two faults, one path. The frames were always decoded — `ChatImageCache` keeps every one —
/// but SwiftUI's `Image(uiImage:)` draws one frame of a multi-frame `UIImage` and never
/// advances it, so the picture stood still; `AnimatedImage` hands those frames to a
/// `UIImageView`, the only view that plays them. The chat list, separately, asked each
/// message for prose and skipped a row that had none.
final class GifPlaybackAndPreviewTests: XCTestCase {
    // MARK: bytes

    /// A real animated GIF: `frames` 8 pt squares alternating red and blue, 0.1 s each.
    private func animatedGIF(frames: Int = 3) throws -> Data {
        let data = NSMutableData()
        guard let dest = CGImageDestinationCreateWithData(data, "com.compuserve.gif" as CFString, frames, nil) else {
            throw XCTSkip("ImageIO cannot write GIFs here")
        }
        CGImageDestinationSetProperties(dest, [kCGImagePropertyGIFDictionary: [kCGImagePropertyGIFLoopCount: 0]] as CFDictionary)
        let frameProps = [kCGImagePropertyGIFDictionary: [kCGImagePropertyGIFDelayTime: 0.1]] as CFDictionary
        for i in 0..<frames {
            let frame = UIGraphicsImageRenderer(size: CGSize(width: 8, height: 8)).image { ctx in
                (i.isMultiple(of: 2) ? UIColor.red : UIColor.blue).setFill()
                ctx.fill(CGRect(x: 0, y: 0, width: 8, height: 8))
            }
            guard let cg = frame.cgImage else { throw XCTSkip("no bitmap for a rendered frame") }
            CGImageDestinationAddImage(dest, cg, frameProps)
        }
        XCTAssertTrue(CGImageDestinationFinalize(dest), "the fixture GIF is written")
        return data as Data
    }

    private func still() -> Data {
        UIGraphicsImageRenderer(size: CGSize(width: 8, height: 8)).image { ctx in
            UIColor.green.setFill()
            ctx.fill(CGRect(x: 0, y: 0, width: 8, height: 8))
        }.pngData()!
    }

    // MARK: the decode keeps every frame

    func testAnimatedBytesDecodeToEveryFrameAndAStillDoesNot() throws {
        let gif = try animatedGIF(frames: 3)
        let frames = AnimatedImageFrames.frames(from: gif)
        XCTAssertEqual(frames?.images.count, 3, "every frame of the GIF")
        XCTAssertEqual(frames?.duration ?? 0, 0.3, accuracy: 0.05, "the frames' own delays")
        XCTAssertEqual(AnimatedImageFrames.animatedImage(from: gif)?.images?.count, 3)
        XCTAssertNil(AnimatedImageFrames.frames(from: still()), "a PNG is one frame, not an animation")
    }

    /// The bubble's decode (`ChatImageCache.decode`) and the viewer's frames-only decode
    /// (`animatedOnly`) agree about what animates.
    func testBothDecodePathsReportTheAnimation() async throws {
        let gif = try animatedGIF(frames: 4)
        let cap = CGSize(width: 840, height: 960)
        let bubble = await ChatImageCache.decode(gif, maxPixels: cap)
        XCTAssertEqual(bubble?.animated, true)
        XCTAssertEqual(bubble?.image.images?.count, 4)
        XCTAssertNotNil(bubble?.stillFrame, "the first frame holds the row while it is off screen")

        let viewer = await ChatImageCache.animatedOnly(gif)
        XCTAssertEqual(viewer?.image.images?.count, 4)
        let viewerStill = await ChatImageCache.animatedOnly(still())
        XCTAssertNil(viewerStill, "a still keeps its full resolution for the zoom")

        let stillDecode = await ChatImageCache.decode(still(), maxPixels: cap)
        XCTAssertEqual(stillDecode?.animated, false)
        XCTAssertNil(stillDecode?.image.images)
    }

    // MARK: the frames actually run

    @MainActor
    func testTheAnimatedViewRunsTheFramesAndStopsWhenAsked() throws {
        let gif = try animatedGIF(frames: 3)
        guard let animated = AnimatedImageFrames.animatedImage(from: gif) else {
            throw XCTSkip("no animation decoded")
        }
        let view = AnimatedImageView()
        view.show(animated, playing: true)
        XCTAssertEqual(view.animationImages?.count, 3, "the frames reach the UIImageView")
        XCTAssertEqual(view.animationDuration, animated.duration, accuracy: 0.001)
        XCTAssertEqual(view.animationRepeatCount, 0, "a GIF loops")
        XCTAssertNotNil(view.image, "the first frame shows under the animation")
        XCTAssertTrue(view.wantsAnimation, "and it is running")

        // An off-screen row parks the frames, and comes back to them.
        view.show(animated, playing: false)
        XCTAssertFalse(view.wantsAnimation)
        view.show(animated, playing: true)
        XCTAssertTrue(view.wantsAnimation)

        // A still asks for nothing, however insistently it is played.
        let plain = UIImage(data: still())!
        view.show(plain, playing: true)
        XCTAssertNil(view.animationImages, "one frame is not an animation")
        XCTAssertFalse(view.wantsAnimation)
    }

    /// Every place the iPhone shows a decoded picture that may carry frames hands it to
    /// `AnimatedImage`: the transcript bubble, the sticker bubble and the full-screen viewer.
    /// A SwiftUI `Image` there is the bug itself, so the sources say so.
    func testThePictureSurfacesUseTheAnimatedView() throws {
        guard let transcript = try source("UI/Transcript.swift"),
              let picker = try source("UI/FunPicker.swift"),
              let viewer = try source("UI/MediaViewer.swift") else {
            throw XCTSkip("sources not beside the test bundle (CI without the tree)")
        }
        XCTAssertTrue(transcript.contains("AnimatedImage(image: shown, playing: true)"), "the photo bubble plays frames")
        XCTAssertTrue(picker.contains("AnimatedImage(image: image, playing: true)"), "an animated sticker plays")
        XCTAssertTrue(viewer.contains("AnimatedImage(image: image, playing: true)"), "a GIF full screen plays")
        XCTAssertTrue(viewer.contains("ChatImageCache.animatedOnly(data)"), "the viewer decodes frames before it draws")
    }

    // MARK: the chat list's line

    private func message(_ parts: [Part], at: Int64 = 1_762_000_000_000) -> Message {
        Message(id: "m1", chatId: "c1", role: .user, sender: "user:me", status: .done, turn: nil, decision: nil,
                subagentId: nil, subagentName: nil, inReplyTo: nil, usage: [:], model: nil,
                createdAt: at, completedAt: nil, pending: false, parts: parts, hiddenCount: 0)
    }

    private func carrying(_ attachment: Attachment, caption: String? = nil) -> Message {
        var parts: [Part] = [.attachment(id: "m1-a", attachment: attachment)]
        if let caption { parts.append(.text(id: "m1-t", text: caption)) }
        return message(parts)
    }

    /// A picked GIF as both clients write it: no `ref` (nothing is stored), the provider's
    /// URL, and the provider's numeric id for a file name.
    private var pickedGif: Attachment {
        Attachment(name: "6178590884800169.gif", mime: "image/gif", blobRef: nil, kind: "image",
                   url: "https://cdn.klipy.com/6178590884800169.gif",
                   source: AttachmentSource(provider: "KLIPY", id: "6178590884800169",
                                            pageUrl: "https://klipy.com/gif/6178590884800169"))
    }

    func testAWordlessRowIsNamedByWhatItCarries() {
        let line = { (m: Message) in m.mediaPreviewLine }
        XCTAssertEqual(line(carrying(pickedGif)), "GIF")
        // A GIF from somewhere other than the picker: the file name decides.
        XCTAssertEqual(line(carrying(Attachment(name: "dance.GIF", mime: "application/octet-stream", blobRef: "b1"))), "GIF")
        XCTAssertEqual(line(carrying(Attachment(name: "IMG_0042.jpeg", mime: "image/jpeg", blobRef: "b1", kind: "image"))), "Photo")
        XCTAssertEqual(line(carrying(Attachment(name: "hug.png", mime: "image/png", blobRef: "b1", kind: "sticker"))), "Sticker")
        XCTAssertEqual(line(carrying(Attachment(name: "clip.mp4", mime: "video/mp4", blobRef: "b1", kind: "video"))), "Video")
        XCTAssertEqual(line(carrying(Attachment(name: "memo.m4a", mime: "audio/mp4", blobRef: "b1", kind: "audio"))), "Voice message")
        // A document keeps its name, which is the useful thing about it.
        XCTAssertEqual(line(carrying(Attachment(name: "Q3 plan.pdf", mime: "application/pdf", blobRef: "b1", kind: "pdf"))), "Q3 plan.pdf")
        XCTAssertNil(line(message([.text(id: "m1-t", text: "hi")])), "words, not media")
    }

    func testTheChatListShowsTheGifRowInsteadOfSkippingIt() {
        let gif = carrying(pickedGif)
        XCTAssertTrue(gif.hasPreviewLine, "the newest row is previewable, so the list stops at it")
        XCTAssertEqual(gif.previewLine, "GIF")

        // A caption still wins: the words are what the reader recognises.
        XCTAssertEqual(carrying(pickedGif, caption: "this is you").previewLine, "this is you")

        let words = message([.text(id: "m1-t", text: "**Sending** you something\nsecond line")])
        XCTAssertTrue(words.hasPreviewLine)
        XCTAssertEqual(words.previewLine, "Sending you something", "first line, markdown markers stripped")

        let blank = message([.text(id: "m1-t", text: "   ")])
        XCTAssertFalse(blank.hasPreviewLine, "nothing to say, so the list looks further back")
        XCTAssertEqual(blank.previewLine, "")
    }

    /// Five lists read the same predicate — the agent chat row, the DM row, the project's
    /// newest message, the home row's static preview and the people preview. Each one
    /// skipped a wordless GIF before.
    func testEveryHomeListAsksForAPreviewLineNotForProse() throws {
        guard let home = try source("UI/HomeView.swift") else {
            throw XCTSkip("sources not beside the test bundle (CI without the tree)")
        }
        XCTAssertEqual(home.components(separatedBy: "$0.hasPreviewLine").count - 1, 5, "all five lists")
        XCTAssertFalse(home.contains("!$0.plainText.isEmpty"), "no list asks for prose alone any more")
    }

    private var sourceRoot: URL {
        // …/apps/ios/Arbos/ArbosTests/<this file> → …/apps/ios/Arbos/Arbos
        URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().appendingPathComponent("Arbos")
    }

    private func source(_ rel: String) throws -> String? {
        let url = sourceRoot.appendingPathComponent(rel)
        guard FileManager.default.fileExists(atPath: url.path) else { return nil }
        return try String(contentsOf: url, encoding: .utf8)
    }
}
