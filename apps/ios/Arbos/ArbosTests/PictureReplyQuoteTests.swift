import SwiftUI
import XCTest
@testable import Arbos

/// Tester bug ([feedback picture-reply-quote-ios-2026-10-28]): a reply that carried a picture
/// lost its quote. The caption rides the photo bubble, so a picture reply — with or without
/// words — left no text bubble to hold the header. The quote now sits above the first media
/// card, once; the caption bar under it stays plain; a text reply to a picture quotes the
/// file's name; and the row the send writes keeps `replyTo` next to its attachments.
@MainActor
final class PictureReplyQuoteTests: XCTestCase {
    private let chat = "fx-dm-ben"
    private let t0: Int64 = 1_700_000_000_000
    private let me = "user:fx-me"
    private let picture = Attachment(name: "IMG_0042.jpeg", mime: "image/jpeg", blobRef: "blob-42", kind: "image")

    private func text(_ i: Int, from sender: String, _ text: String, replyTo: String? = nil) -> Message {
        var m = Message(id: "m\(i)", chatId: chat, role: .user, sender: sender, status: .done, turn: nil, decision: nil,
                        subagentId: nil, subagentName: nil, inReplyTo: nil, usage: [:], model: nil,
                        createdAt: t0 + Int64(i) * 1_000, completedAt: nil, pending: false,
                        parts: [.text(id: "m\(i)-t", text: text)], hiddenCount: 0, senderHandle: "ben", senderName: "Ben")
        m.replyTo = replyTo
        return m
    }

    /// The send shape: the picture part first, then the (possibly empty) text part.
    private func pictureReply(_ i: Int, from sender: String, caption: String, replyTo: String) -> Message {
        var m = Message(id: "m\(i)", chatId: chat, role: .user, sender: sender, status: .done, turn: nil, decision: nil,
                        subagentId: nil, subagentName: nil, inReplyTo: nil, usage: [:], model: nil,
                        createdAt: t0 + Int64(i) * 1_000, completedAt: nil, pending: false,
                        parts: [.attachment(id: "m\(i)-a", attachment: picture), .text(id: "m\(i)-t", text: caption)],
                        hiddenCount: 0, senderHandle: "ben", senderName: "Ben")
        m.replyTo = replyTo
        return m
    }

    private func rows(_ messages: [Message]) -> [Row] {
        Transcript.rows(messages, projectId: "\(peopleSlot):\(chat)", me: me, people: true, multiParty: false)
    }

    /// Per row: `(kind, quote text, caption)`.
    private func shape(_ rows: [Row]) -> [String] {
        rows.compactMap { row in
            switch row.kind {
            case .image(_, _, let caption, _, _, _, _, _, let quote):
                return "image quote=\(quote?.text ?? "-") caption=\(caption ?? "-")"
            case .file(_, _, _, _, _, _, let quote):
                return "file quote=\(quote?.text ?? "-")"
            case .user(let text, _, let quote, _):
                return "user \(text) quote=\(quote?.text ?? "-")"
            case .other(let text, _, _, _, let quote, _):
                return "other \(text) quote=\(quote?.text ?? "-")"
            default:
                return nil
            }
        }
    }

    func testPictureRepliesQuoteAboveTheFirstCard() {
        let original = text(0, from: "user:fx-ben", "look at this")
        let wordless = pictureReply(1, from: me, caption: "", replyTo: original.id)
        let captioned = pictureReply(2, from: me, caption: "same here", replyTo: original.id)
        let toPicture = text(3, from: "user:fx-ben", "nice shot", replyTo: wordless.id)
        XCTAssertEqual(shape(rows([original, wordless, captioned, toPicture])), [
            "other look at this quote=-",
            "image quote=look at this caption=-",
            "image quote=look at this caption=same here",
            "other nice shot quote=IMG_0042.jpeg",
        ])
    }

    /// A non-captionable file (PDF + note) keeps its own text bubble; the quote still sits
    /// on the card, not on the note.
    func testFileReplyQuotesOnTheCardNotTheNote() {
        let original = text(0, from: "user:fx-ben", "send the deck")
        var reply = pictureReply(1, from: me, caption: "here", replyTo: original.id)
        reply.parts[0] = .attachment(id: "m1-a", attachment: Attachment(name: "deck.pdf", mime: "application/pdf", blobRef: "blob-d"))
        XCTAssertEqual(shape(rows([original, reply])), [
            "other send the deck quote=-",
            "file quote=send the deck",
            "user here quote=-",
        ])
    }

    /// A forwarded picture carries its forward header and never a quote.
    func testForwardedPictureNeverQuotes() {
        let original = text(0, from: "user:fx-ben", "look at this")
        var forwarded = pictureReply(1, from: me, caption: "caption", replyTo: original.id)
        forwarded.forwardedFrom = "Sam Rivera"
        let out = rows([original, forwarded])
        XCTAssertEqual(shape(out), ["other look at this quote=-", "image quote=- caption=caption"])
        // Rows also hold the date stamp: find the card rather than index it.
        var header: String?
        for row in out {
            if case .image(_, _, _, _, _, let forwardedFrom, _, _, _) = row.kind { header = forwardedFrom; break }
        }
        XCTAssertEqual(header, "Sam Rivera")
    }

    /// The row the send writes through core keeps `replyTo` beside its attachment parts — a
    /// wordless picture reply included — so the recipient's device draws the quote too.
    func testRowBuilderKeepsReplyToWithAttachments() throws {
        let doc = CoreChatDoc(key: chat)
        try doc.insertUserMessage(id: "m1", text: "", deviceId: "ios:test", attachments: [picture],
                                  author: .init(sender: me, handle: "me", name: "Me", avatar: nil), replyTo: "original")
        try doc.insertUserMessage(id: "m2", text: "same here", deviceId: "ios:test", attachments: [picture],
                                  author: .init(sender: me, handle: "me", name: "Me", avatar: nil), replyTo: "original")
        doc.commit()
        let rows = doc.readAll()
        XCTAssertEqual(rows.count, 2)
        for row in rows {
            XCTAssertEqual(row.replyTo, "original", row.id)
            XCTAssertNil(row.replyRef)
            guard case .attachment(_, let a) = row.parts.first else { return XCTFail("picture first: \(row.parts)") }
            XCTAssertEqual(a.blobRef, "blob-42")
        }
        XCTAssertEqual(rows[0].plainText, "")
        XCTAssertEqual(rows[1].plainText, "same here")
    }

    /// The quote header takes the same slot as a forward header: it widens the bubble and
    /// pushes the picture down by its own height.
    func testQuoteHeaderSitsAboveThePicture() {
        let cap: CGFloat = 393 - Theme.bubbleInset
        let portrait = ImageBubble.fit(CGSize(width: 390, height: 844), maxWidth: min(280, cap))
        let header = CGSize(width: 220, height: 30)
        let g = PhotoBubbleLayout.geometry(picture: portrait, caption: nil, header: header, cap: cap,
                                           pictureInset: ImageBubble.pictureInset)
        XCTAssertEqual(g.tileTop, header.height, "the picture starts under the quote")
        XCTAssertGreaterThanOrEqual(g.size.width, header.width, "a long quote widens the bubble")
        XCTAssertEqual(g.size.height, header.height + g.tileHeight)
        let plain = PhotoBubbleLayout.geometry(picture: portrait, caption: nil, cap: cap, pictureInset: ImageBubble.pictureInset)
        XCTAssertEqual(plain.tileTop, 0)
    }
}
