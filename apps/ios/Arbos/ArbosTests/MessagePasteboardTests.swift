import UIKit
import UniformTypeIdentifiers
import XCTest
@testable import Arbos

/// Copy of a message puts the message on the pasteboard, not its words alone
/// ([feedback copy-image-messages-ios-2026-10-27]): per message shape — text, picture
/// only, picture + caption, several pictures, a GIF — what `UIPasteboard` ends up
/// holding, and what the composer's paste reads back from it.
final class MessagePasteboardTests: XCTestCase {
    private func png(_ w: Int = 2, _ h: Int = 2, color: UIColor = .red) -> Data {
        UIGraphicsImageRenderer(size: CGSize(width: w, height: h)).image { ctx in
            color.setFill()
            ctx.fill(CGRect(x: 0, y: 0, width: w, height: h))
        }.pngData()!
    }

    private func jpeg(_ w: Int = 4, _ h: Int = 4) -> Data {
        UIGraphicsImageRenderer(size: CGSize(width: w, height: h)).image { ctx in
            UIColor.blue.setFill()
            ctx.fill(CGRect(x: 0, y: 0, width: w, height: h))
        }.jpegData(compressionQuality: 0.9)!
    }

    /// A real 1 × 1 GIF89a (what Messages animates when more frames follow).
    private var gif: Data {
        Data([0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x01, 0x00, 0x01, 0x00, 0x80, 0x00, 0x00,
              0x00, 0xFF, 0x00, 0xFF, 0xFF, 0xFF, 0x21, 0xF9, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00,
              0x2C, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x02, 0x02, 0x44, 0x01, 0x00, 0x3B])
    }

    private func fetched(_ name: String, _ mime: String, _ data: Data) -> MessagePasteboard.Fetched {
        .init(name: name, mime: mime, data: data)
    }

    private func message(_ parts: [Part]) -> Message {
        Message(id: "m1", chatId: "c1", role: .user, sender: "user:u1", status: .done, turn: nil, decision: nil,
                subagentId: nil, subagentName: nil, inReplyTo: nil, usage: [:], model: nil,
                createdAt: 1_700_000_000_000, completedAt: nil, pending: false, parts: parts, hiddenCount: 0)
    }

    override func setUp() {
        super.setUp()
        UIPasteboard.general.items = []
    }

    // MARK: layout per message shape

    func testTextOnlyIsOneTextItem() {
        let items = MessagePasteboard.items(text: "  hello there  ", fetched: [])
        XCTAssertEqual(items.count, 1)
        XCTAssertEqual(items[0][MessagePasteboard.text] as? String, "hello there")
        XCTAssertEqual(items[0].count, 1, "no picture types on a words-only item")
        XCTAssertTrue(MessagePasteboard.items(text: "  ", fetched: []).isEmpty)
    }

    func testImageOnlyKeepsThePngUnderItsOwnTypeAndNoText() {
        let bytes = png()
        let items = MessagePasteboard.items(text: "", fetched: [fetched("shot.png", "image/png", bytes)])
        XCTAssertEqual(items.count, 1)
        XCTAssertEqual(items[0][MessagePasteboard.png] as? Data, bytes, "the original bytes, not a re-encode")
        XCTAssertNil(items[0][MessagePasteboard.text])
        XCTAssertEqual(items[0].count, 1)
    }

    func testJpegStaysJpegWithAPngTwin() throws {
        let bytes = jpeg()
        let items = MessagePasteboard.items(text: "", fetched: [fetched("photo.jpg", "image/jpeg", bytes)])
        XCTAssertEqual(items[0][MessagePasteboard.jpeg] as? Data, bytes)
        let twin = try XCTUnwrap(items[0][MessagePasteboard.png] as? Data, "a public.png twin for PNG-only readers")
        XCTAssertEqual(UIImage(data: twin)?.size, CGSize(width: 4, height: 4))
    }

    func testGifTypeIsPreserved() {
        // A wrong MIME from the relay does not change the declared type: the bytes decide.
        let items = MessagePasteboard.items(text: "", fetched: [fetched("loop.gif", "application/octet-stream", gif)])
        XCTAssertEqual(items[0][MessagePasteboard.gif] as? Data, gif, "the GIF bytes themselves, frames and all")
        XCTAssertNotNil(items[0][MessagePasteboard.png], "plus a PNG of the first frame")
    }

    func testMixedMessageIsOneRichItemWithTextAndPicture() {
        let bytes = png()
        let items = MessagePasteboard.items(text: "Look at this", fetched: [fetched("shot.png", "image/png", bytes)])
        XCTAssertEqual(items.count, 1, "one item, several types — not two items")
        XCTAssertEqual(items[0][MessagePasteboard.text] as? String, "Look at this")
        XCTAssertEqual(items[0][MessagePasteboard.png] as? Data, bytes)
    }

    func testMultipleImagesAreMultipleItemsInMessageOrderTextOnTheFirst() {
        let items = MessagePasteboard.items(text: "three shots", fetched: [
            fetched("1.png", "image/png", png()),
            fetched("2.jpg", "image/jpeg", jpeg()),
            fetched("3.gif", "image/gif", gif),
        ])
        XCTAssertEqual(items.count, 3)
        XCTAssertNotNil(items[0][MessagePasteboard.png])
        XCTAssertNotNil(items[1][MessagePasteboard.jpeg])
        XCTAssertNotNil(items[2][MessagePasteboard.gif])
        XCTAssertEqual(items[0][MessagePasteboard.text] as? String, "three shots")
        XCTAssertNil(items[1][MessagePasteboard.text])
        XCTAssertNil(items[2][MessagePasteboard.text])
    }

    func testAFileTravelsUnderItsOwnType() {
        let items = MessagePasteboard.items(text: "the deck", fetched: [fetched("deck.pdf", "application/pdf", Data("%PDF-1.4".utf8))])
        XCTAssertEqual(items.count, 1)
        XCTAssertEqual(items[0][UTType.pdf.identifier] as? Data, Data("%PDF-1.4".utf8))
        XCTAssertEqual(items[0][MessagePasteboard.text] as? String, "the deck")
    }

    func testTheFormatComesFromTheBytesThenTheMimeThenTheName() {
        XCTAssertEqual(MessagePasteboard.imageType(of: fetched("x.bin", "application/octet-stream", jpeg())), MessagePasteboard.jpeg)
        XCTAssertEqual(MessagePasteboard.imageType(of: fetched("x.bin", "image/webp", Data([1, 2, 3]))), MessagePasteboard.webp)
        XCTAssertEqual(MessagePasteboard.imageType(of: fetched("x.gif", "application/octet-stream", Data([1, 2, 3]))), MessagePasteboard.gif)
        XCTAssertNil(MessagePasteboard.imageType(of: fetched("notes.pdf", "application/pdf", Data([1, 2, 3]))))
    }

    // MARK: the message body feeds it

    func testThePictureOnlyMessageHasABodyToCopy() throws {
        let m = message([.attachment(id: "a1", attachment: Attachment(name: "IMG_1.jpg", mime: "image/jpeg", blobRef: "k1", kind: "image"))])
        let body = try XCTUnwrap(m.body, "a picture-only message is a message like any other")
        XCTAssertEqual(body.text, "")
        XCTAssertEqual(body.attachments.map(\.blobRef), ["k1"])
        XCTAssertEqual(m.plainText, "", "what the old Copy put on the pasteboard: nothing")
    }

    func testAnUploadingPictureIsNotCopiedYet() {
        let m = message([.attachment(id: "a1", attachment: Attachment(name: "IMG_1.jpg", mime: "image/jpeg", blobRef: "local:1", kind: "image"))])
        XCTAssertNil(m.body)
        XCTAssertTrue(m.hasUploadingAttachment)
    }

    // MARK: UIPasteboard round trip

    func testWrittenItemsReadBackByTypeAndTheComposerStagesThem() {
        let shot = png(3, 3)
        let items = MessagePasteboard.items(text: "caption", fetched: [
            fetched("shot.png", "image/png", shot),
            fetched("loop.gif", "image/gif", gif),
        ])
        MessagePasteboard.write(items)
        let pb = UIPasteboard.general
        XCTAssertEqual(pb.numberOfItems, 2)
        XCTAssertEqual(pb.string, "caption", "a text field receives the words")
        XCTAssertEqual(pb.data(forPasteboardType: MessagePasteboard.png, inItemSet: IndexSet(integer: 0))?.first, shot)
        XCTAssertEqual(pb.data(forPasteboardType: MessagePasteboard.gif, inItemSet: IndexSet(integer: 1))?.first, gif)
        XCTAssertTrue(pb.hasImages)
        XCTAssertEqual(pb.image?.size, CGSize(width: 3, height: 3), "UIImage readers get the picture")

        // The composer's paste: one attachment per picture item, the GIF as a GIF.
        XCTAssertTrue(MessagePasteboard.hasMedia(pb))
        let staged = MessagePasteboard.pendingAttachments(from: pb)
        XCTAssertEqual(staged.count, 2)
        XCTAssertEqual(staged[0].mime, "image/png")
        XCTAssertEqual(staged[0].data, shot)
        XCTAssertTrue(staged[0].name.hasSuffix(".png"))
        XCTAssertEqual(staged[1].mime, "image/gif")
        XCTAssertEqual(staged[1].data, gif, "the native GIF, not its PNG twin")
        XCTAssertTrue(staged[1].name.hasSuffix(".gif"))
    }

    func testAWordsOnlyPasteboardStagesNothing() {
        UIPasteboard.general.string = "just words"
        XCTAssertFalse(MessagePasteboard.hasMedia())
        XCTAssertTrue(MessagePasteboard.pendingAttachments().isEmpty)
    }
}
