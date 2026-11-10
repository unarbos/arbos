import ImageIO
import PencilKit
import UIKit
import XCTest
@testable import Arbos

/// Crop and draw on a staged picture (Scout's request; mockups approved 2026-10-31).
///
/// The geometry is where this feature can go quietly wrong, so it is all in pure functions
/// and pinned here: the crop and the strokes stay independent, a stroke outside the crop
/// keeps its place rather than being rescaled into view, and the editor takes a still and
/// leaves anything with more than one frame to the preview that handles it today.
final class PhotoEditorTests: XCTestCase {

    // MARK: Geometry

    func testTheWholePictureFitsTheViewportInCropMode() {
        let frame = PhotoEditGeometry.pictureFrame(picture: CGSize(width: 1400, height: 900),
                                                   crop: PhotoEdit.whole, in: CGSize(width: 402, height: 700))
        XCTAssertEqual(frame.width, 402, accuracy: 0.01, "the long side fills the viewport")
        XCTAssertEqual(frame.height, 402 * 900 / 1400, accuracy: 0.01)
        XCTAssertEqual(frame.midX, 201, accuracy: 0.01, "centred")
        XCTAssertEqual(frame.midY, 350, accuracy: 0.01)
    }

    func testTheCropFillsTheViewportInDrawMode() {
        let picture = CGSize(width: 1400, height: 900)
        let crop = CGRect(x: 0.5, y: 0, width: 0.5, height: 1)
        let viewport = CGSize(width: 400, height: 800)
        let frame = PhotoEditGeometry.pictureFrame(picture: picture, crop: crop, in: viewport)
        let kept = PhotoEditGeometry.rect(of: crop, in: frame)
        // 700 × 900 fitted in 400 × 800 is 400 wide; the kept half is centred on the viewport.
        XCTAssertEqual(kept.width, 400, accuracy: 0.01)
        XCTAssertEqual(kept.midX, 200, accuracy: 0.01)
        XCTAssertEqual(kept.midY, 400, accuracy: 0.01)
    }

    func testStrokesOutsideTheCropKeepTheirPlaceOffScreen() {
        let picture = CGSize(width: 1000, height: 1000)
        let crop = CGRect(x: 0.6, y: 0.6, width: 0.4, height: 0.4)
        let viewport = CGSize(width: 400, height: 400)
        let frame = PhotoEditGeometry.pictureFrame(picture: picture, crop: crop, in: viewport)
        // The whole picture is 2.5× the viewport and hangs off its leading and top edges:
        // a stroke made at the picture's top-left is still at the picture's top-left, which
        // is what lets a widened crop bring it back.
        XCTAssertEqual(frame.width, 1000, accuracy: 0.01)
        XCTAssertLessThan(frame.minX, 0)
        XCTAssertLessThan(frame.minY, 0)
        XCTAssertEqual(PhotoEditGeometry.rect(of: crop, in: frame),
                       CGRect(x: 0, y: 0, width: 400, height: 400))
    }

    func testACornerMovesTwoSidesAndAnEdgeBarMovesOne() {
        let start = PhotoEdit.whole
        let corner = PhotoEditGeometry.resized(start, handle: .topLeading,
                                               by: CGSize(width: 0.2, height: 0.3))
        XCTAssertEqual(corner, CGRect(x: 0.2, y: 0.3, width: 0.8, height: 0.7))
        let edge = PhotoEditGeometry.resized(start, handle: .trailing,
                                             by: CGSize(width: -0.25, height: 0.4))
        XCTAssertEqual(edge, CGRect(x: 0, y: 0, width: 0.75, height: 1), "an edge bar ignores the other axis")
    }

    func testACropStaysInsideThePictureAndKeepsItsMinimum() {
        let out = PhotoEditGeometry.resized(PhotoEdit.whole, handle: .topLeading,
                                            by: CGSize(width: -2, height: -2))
        XCTAssertEqual(out, PhotoEdit.whole, "a drag past the edge stops at it")
        let tiny = PhotoEditGeometry.resized(PhotoEdit.whole, handle: .bottomTrailing,
                                             by: CGSize(width: -2, height: -2))
        XCTAssertEqual(tiny.width, PhotoEditGeometry.minimumSide, accuracy: 0.0001)
        XCTAssertEqual(tiny.height, PhotoEditGeometry.minimumSide, accuracy: 0.0001)
    }

    func testWideningACropAfterNarrowingItRestoresTheFrame() {
        let narrow = PhotoEditGeometry.resized(PhotoEdit.whole, handle: .leading,
                                               by: CGSize(width: 0.4, height: 0))
        XCTAssertEqual(narrow.minX, 0.4, accuracy: 0.0001)
        let wide = PhotoEditGeometry.resized(narrow, handle: .leading, by: CGSize(width: -0.4, height: 0))
        XCTAssertEqual(wide, PhotoEdit.whole, "the crop is a rectangle, not a destructive step")
    }

    func testEveryCornerAndEveryEdgeHasExactlyOneHandle() {
        let all = PhotoCropHandle.allCases
        XCTAssertEqual(all.count, 8)
        XCTAssertEqual(all.filter(\.isCorner).count, 4)
        let rect = CGRect(x: 10, y: 20, width: 100, height: 200)
        XCTAssertEqual(PhotoCropHandle.topLeading.point(of: rect), CGPoint(x: 10, y: 20))
        XCTAssertEqual(PhotoCropHandle.bottomTrailing.point(of: rect), CGPoint(x: 110, y: 220))
        XCTAssertEqual(PhotoCropHandle.top.point(of: rect), CGPoint(x: 60, y: 20))
        XCTAssertEqual(PhotoCropHandle.trailing.point(of: rect), CGPoint(x: 110, y: 120))
    }

    // MARK: What the editor takes

    func testAStillGifIsEditableAndAnAnimatedOneIsNot() {
        let still = gif(frames: 1)
        let moving = gif(frames: 3)
        XCTAssertEqual(PhotoEditorEligibility.frameCount(of: still), 1)
        XCTAssertEqual(PhotoEditorEligibility.frameCount(of: moving), 3)
        // Both are `image/gif`: only the frame count tells them apart.
        XCTAssertTrue(PhotoEditorEligibility.canEdit(mime: "image/gif", data: still))
        XCTAssertFalse(PhotoEditorEligibility.canEdit(mime: "image/gif", data: moving))
    }

    func testVideosAndFilesAreNotEditable() {
        XCTAssertFalse(PhotoEditorEligibility.canEdit(mime: "video/quicktime", frames: 1))
        XCTAssertFalse(PhotoEditorEligibility.canEdit(mime: "application/pdf", frames: 1))
        XCTAssertTrue(PhotoEditorEligibility.canEdit(mime: "IMAGE/JPEG", frames: 1), "the mime test is case-blind")
    }

    func testBytesNothingCanDecodeAreNotEditable() {
        XCTAssertEqual(PhotoEditorEligibility.frameCount(of: Data("not a picture".utf8)), 0)
        XCTAssertFalse(PhotoEditorEligibility.canEdit(mime: "image/png", data: Data("not a picture".utf8)))
        XCTAssertFalse(PhotoEditorEligibility.canEdit(mime: "image/png", data: Data()))
    }

    // MARK: Flattening

    func testPixelCropRoundsToWholePixelsAndIsNeverEmpty() {
        let box = PhotoEditRender.pixelCrop(CGRect(x: 0.333, y: 0.5, width: 0.334, height: 0.25),
                                            picture: CGSize(width: 999, height: 601))
        XCTAssertEqual(box, CGRect(x: 333, y: 301, width: 334, height: 150))
        let sliver = PhotoEditRender.pixelCrop(CGRect(x: 0, y: 0, width: 0.0001, height: 0.0001),
                                               picture: CGSize(width: 10, height: 10))
        XCTAssertEqual(sliver.width, 1, "a crop never flattens to nothing")
        XCTAssertEqual(sliver.height, 1)
    }

    func testFlatteningReturnsTheCropRegion() {
        let picture = halves()
        let right = PhotoEditRender.flattened(picture, crop: CGRect(x: 0.5, y: 0, width: 0.5, height: 1),
                                              drawing: PKDrawing())
        XCTAssertEqual(right.size, CGSize(width: 50, height: 100))
        let (r, _, b) = average(right)
        XCTAssertGreaterThan(b, r + 60, "the kept half is the blue one")
        let left = PhotoEditRender.flattened(picture, crop: CGRect(x: 0, y: 0, width: 0.5, height: 1),
                                             drawing: PKDrawing())
        let (lr, _, lb) = average(left)
        XCTAssertGreaterThan(lr, lb + 60, "and the other crop keeps the red one")
    }

    func testPngStaysPngAndEverythingElseLeavesAsJpeg() {
        let picture = halves()
        let png = PhotoEditRender.encoded(picture, name: "Relay health.png", mime: "image/png")
        XCTAssertEqual(png?.mime, "image/png")
        XCTAssertEqual(png?.name, "Relay health.png")
        let heic = PhotoEditRender.encoded(picture, name: "IMG_0042.heic", mime: "image/heic")
        XCTAssertEqual(heic?.mime, "image/jpeg")
        XCTAssertEqual(heic?.name, "IMG_0042.jpg", "the blob key is built from the name, so it follows the bytes")
        XCTAssertEqual(PhotoEditRender.renamed("", ext: "jpg"), "photo.jpg")
    }

    func testAnEmptyDrawingIsStoredAsNoBytes() {
        XCTAssertEqual(PhotoEdit.strokeData(PKDrawing()), Data(),
                       "so `isWhole` tells never-drew from drew-and-erased without decoding")
        var edit = PhotoEdit(original: Data("bytes".utf8))
        XCTAssertTrue(edit.isWhole)
        XCTAssertTrue(edit.drawing.strokes.isEmpty)
        edit.crop = CGRect(x: 0, y: 0, width: 0.5, height: 1)
        XCTAssertFalse(edit.isWhole)
    }

    // MARK: The composer's side

    func testDoneReplacesTheBytesAndKeepsTheOriginal() {
        let original = Data("original".utf8)
        let staged = [AppModel.PendingAttachment(name: "Relay health.png", mime: "image/png", data: original)]
        let edit = PhotoEdit(original: original, crop: CGRect(x: 0, y: 0, width: 0.5, height: 1), strokes: Data())
        let done = PhotoEditResult(id: staged[0].id, name: "Relay health.png", mime: "image/png",
                                   data: Data("cropped".utf8), edit: edit)
        let out = ChatSession.applyingPhotoEdit(done, to: staged)
        XCTAssertEqual(out[0].data, Data("cropped".utf8), "the send path only ever reads `data`")
        XCTAssertEqual(out[0].edit?.original, original, "and the original is still here while it is staged")
        XCTAssertEqual(ChatSession.applyingPhotoEdit(
            PhotoEditResult(id: UUID(), name: "x", mime: "image/png", data: Data(), edit: nil), to: staged),
                       staged, "an id that is no longer staged changes nothing")
    }

    func testAnEditBackToTheOriginalIsNotCarried() {
        let original = Data("original".utf8)
        var staged = [AppModel.PendingAttachment(name: "p.png", mime: "image/png", data: Data("cropped".utf8))]
        staged[0].edit = PhotoEdit(original: original, crop: CGRect(x: 0.2, y: 0, width: 0.8, height: 1))
        let reset = PhotoEditResult(id: staged[0].id, name: "p.png", mime: "image/png", data: original, edit: nil)
        let out = ChatSession.applyingPhotoEdit(reset, to: staged)
        XCTAssertEqual(out[0].data, original)
        XCTAssertNil(out[0].edit, "Original leaves no edit behind, not an identity one")
    }

    func testTheEditorTakesAStillAndLeavesAVideoToThePreview() {
        let still = AppModel.PendingAttachment(name: "p.gif", mime: "image/gif", data: gif(frames: 1))
        XCTAssertNotNil(ChatSession.editorTarget(for: still))
        let moving = AppModel.PendingAttachment(name: "loop.gif", mime: "image/gif", data: gif(frames: 4))
        XCTAssertNil(ChatSession.editorTarget(for: moving))
        let clip = AppModel.PendingAttachment(name: "clip.mov", mime: "video/quicktime", data: gif(frames: 1))
        XCTAssertNil(ChatSession.editorTarget(for: clip))
        let doc = AppModel.PendingAttachment(name: "deck.pdf", mime: "application/pdf", data: Data())
        XCTAssertNil(ChatSession.editorTarget(for: doc))
    }

    func testReopeningAnEditedPictureStartsFromItsOriginal() {
        let original = gif(frames: 1)
        var staged = AppModel.PendingAttachment(name: "p.gif", mime: "image/gif", data: Data("cropped".utf8))
        let crop = CGRect(x: 0.5, y: 0, width: 0.5, height: 1)
        staged.edit = PhotoEdit(original: original, crop: crop)
        let target = ChatSession.editorTarget(for: staged)
        XCTAssertEqual(target?.edit.original, original, "not the flattened bytes, or a widen could never undo a crop")
        XCTAssertEqual(target?.edit.crop, crop, "and it resumes where Done left it")
    }

    // MARK: Fixtures

    /// A real GIF with `frames` frames, written through ImageIO so the frame count under
    /// test is the one a picked file would have.
    private func gif(frames: Int) -> Data {
        let out = NSMutableData()
        guard let dest = CGImageDestinationCreateWithData(out as CFMutableData, "com.compuserve.gif" as CFString,
                                                          frames, nil) else {
            return Data()
        }
        let props: [CFString: Any] = [kCGImagePropertyGIFDictionary: [kCGImagePropertyGIFDelayTime: 0.1] as [CFString: Any]]
        for i in 0..<frames {
            let level = CGFloat(i) / CGFloat(max(frames, 1))
            let format = UIGraphicsImageRendererFormat.default()
            format.scale = 1
            let frame = UIGraphicsImageRenderer(size: CGSize(width: 8, height: 8), format: format).image { ctx in
                UIColor(white: level, alpha: 1).setFill()
                ctx.fill(CGRect(x: 0, y: 0, width: 8, height: 8))
            }
            guard let cg = frame.cgImage else { continue }
            CGImageDestinationAddImage(dest, cg, props as CFDictionary)
        }
        CGImageDestinationFinalize(dest)
        return out as Data
    }

    /// 100 × 100: red on the left, blue on the right.
    private func halves() -> UIImage {
        let format = UIGraphicsImageRendererFormat.default()
        format.scale = 1
        format.opaque = true
        return UIGraphicsImageRenderer(size: CGSize(width: 100, height: 100), format: format).image { ctx in
            UIColor.red.setFill()
            ctx.fill(CGRect(x: 0, y: 0, width: 50, height: 100))
            UIColor.blue.setFill()
            ctx.fill(CGRect(x: 50, y: 0, width: 50, height: 100))
        }
    }

    /// The whole picture averaged into one pixel, so a crop can be checked by what is in it
    /// without depending on Core Graphics' y direction.
    private func average(_ image: UIImage) -> (Int, Int, Int) {
        guard let cg = image.cgImage else { return (-1, -1, -1) }
        let buffer = UnsafeMutablePointer<UInt8>.allocate(capacity: 4)
        buffer.initialize(repeating: 0, count: 4)
        defer { buffer.deallocate() }
        guard let ctx = CGContext(data: buffer, width: 1, height: 1, bitsPerComponent: 8, bytesPerRow: 4,
                                  space: CGColorSpaceCreateDeviceRGB(),
                                  bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return (-1, -1, -1) }
        ctx.interpolationQuality = .high
        ctx.draw(cg, in: CGRect(x: 0, y: 0, width: 1, height: 1))
        return (Int(buffer[0]), Int(buffer[1]), Int(buffer[2]))
    }
}
