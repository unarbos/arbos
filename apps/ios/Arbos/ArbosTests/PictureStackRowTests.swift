import XCTest
@testable import Arbos

/// Jacob, 2026-11-03: a send of two pictures (a "Flip credit" chart and a "Weights v9"
/// chart) drew two full-size cards stacked down the chat. The message's pictures are one
/// "+N" tile now, and "+N" counts **every** picture of the message — his two-picture send
/// reads "+2", not "+1". Files beside the pictures keep their own cards.
final class PictureStackRowTests: XCTestCase {
    private let chat = "01M5MHDEZFB5SS6VRK9T12NVAC"
    private let t0: Int64 = 1_700_000_000_000

    private func send(_ names: [String], extra: [[String: Any]] = []) throws -> Message {
        let parts: [[String: Any]] = names.enumerated().map { i, name -> [String: Any] in
            ["id": "a\(i)", "kind": "attachment",
             "attachment": ["name": name, "mime": "image/png", "ref": "blob-\(i)", "kind": "image"]]
        } + extra
        let raw: [String: Any] = ["id": "m1", "role": "user", "sender": "user:gh_jacob", "senderHandle": "jacob",
                                  "status": "done", "createdAt": t0, "parts": parts]
        return try XCTUnwrap(DocJSON.message(raw, chatId: chat))
    }

    /// Every picture row's file name, in order.
    private func names(_ rows: [Row]) -> [String] {
        rows.compactMap { row -> String? in
            guard case .image(item: let item, fromUser: _, caption: _, lane: _, source: _, forwardedFrom: _,
                              sender: _, named: _, quote: _, stack: _) = row.kind else { return nil }
            return item.name
        }
    }

    /// How many pictures each picture row stands for, in order.
    private func stacks(_ rows: [Row]) -> [Int] {
        rows.compactMap { row -> Int? in
            guard case .image(item: _, fromUser: _, caption: _, lane: _, source: _, forwardedFrom: _,
                              sender: _, named: _, quote: _, stack: let stack) = row.kind else { return nil }
            return stack
        }
    }

    func testTwoPicturesInOneSendCollapseIntoOneTile() throws {
        let rows = Transcript.rows([try send(["flip-credit.png", "weights-v9.png"])],
                                   projectId: "arbos", people: false, multiParty: false)
        XCTAssertEqual(names(rows), ["flip-credit.png"], "one row showing the first picture")
        XCTAssertEqual(stacks(rows), [2], "+N counts both pictures, so Jacob's send reads +2")
        XCTAssertEqual(MediaStack.text(2), "+2")
    }

    func testOnePictureIsStillItsOwnCard() throws {
        let rows = Transcript.rows([try send(["only.png"])],
                                   projectId: "arbos", people: false, multiParty: false)
        XCTAssertEqual(stacks(rows), [1])
    }

    func testThreePicturesReadPlusThree() throws {
        let rows = Transcript.rows([try send(["a.png", "b.png", "c.png"])],
                                   projectId: "arbos", people: false, multiParty: false)
        XCTAssertEqual(stacks(rows), [3])
        XCTAssertEqual(MediaStack.text(3), "+3")
    }

    /// Only pictures collapse: a document beside them keeps its own card.
    func testAFileBesideTwoPicturesKeepsItsCard() throws {
        let doc: [[String: Any]] = [["id": "d1", "kind": "attachment",
                                     "attachment": ["name": "deck.pdf", "mime": "application/pdf",
                                                    "ref": "blob-pdf", "kind": "file"]]]
        let rows = Transcript.rows([try send(["a.png", "b.png"], extra: doc)],
                                   projectId: "arbos", people: false, multiParty: false)
        XCTAssertEqual(stacks(rows), [2], "the two pictures are one tile")
        let files = rows.compactMap { row -> String? in
            guard case .file(item: let item, subtitle: _, caption: _, forwardedFrom: _,
                             sender: _, named: _, quote: _) = row.kind else { return nil }
            return item.name
        }
        XCTAssertEqual(files, ["deck.pdf"], "the document still draws: \(rows.map(\.id))")
    }

    /// The collapse rule itself, away from row building.
    func testCollapsingPicturesKeepsOrderAndCounts() {
        func picture(_ name: String) -> Attachment {
            Attachment(name: name, mime: "image/png", blobRef: "b-\(name)", kind: "image")
        }
        func file(_ name: String) -> Attachment {
            Attachment(name: name, mime: "application/pdf", blobRef: "b-\(name)", kind: "pdf")
        }
        let mixed: [(id: String, a: Attachment)] = [
            ("1", file("first.pdf")), ("2", picture("a.png")), ("3", picture("b.png")), ("4", file("last.pdf")),
        ]
        let out = Transcript.collapsingPictures(mixed)
        XCTAssertEqual(out.map(\.attachment.name), ["first.pdf", "a.png", "last.pdf"])
        XCTAssertEqual(out.map(\.stack), [1, 2, 1])
        // One picture: nothing collapses.
        let single: [(id: String, a: Attachment)] = [("1", picture("only.png"))]
        XCTAssertEqual(Transcript.collapsingPictures(single).map(\.stack), [1])
    }
}
