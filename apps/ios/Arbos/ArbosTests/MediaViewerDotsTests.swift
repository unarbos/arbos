import XCTest
@testable import Arbos

/// The viewer's dots are iMessage's: the pictures sent together with the one on screen, while
/// paging still runs through the whole chat's album.
final class MediaViewerDotsTests: XCTestCase {
    private func album(_ messages: [(String, Int)]) -> [MediaItem] {
        messages.flatMap { m, n in
            (0..<n).map { i in
                MediaItem(id: "\(m)#p\(i)", name: "p.png", mime: "image/png", kind: .image, blobRef: "r",
                          slot: "people:c", by: "Sam", at: 0)
            }
        }
    }

    func testDotsCoverOnlyTheMessageOnScreen() {
        // A lone picture, then four sent together, then another lone one: 6 in the album.
        let a = album([("m1", 1), ("m2", 4), ("m3", 1)])
        XCTAssertNil(MediaViewer.dots(a, at: 0), "a lone picture has no dots")
        XCTAssertEqual(MediaViewer.dots(a, at: 1)?.count, 4)
        XCTAssertEqual(MediaViewer.dots(a, at: 1)?.current, 0)
        XCTAssertEqual(MediaViewer.dots(a, at: 4)?.current, 3)
        XCTAssertNil(MediaViewer.dots(a, at: 5), "paging on past the group: back to none")
    }

    func testAHugeSendShowsNoDots() {
        XCTAssertNil(MediaViewer.dots(album([("m1", MediaViewer.maxDots + 1)]), at: 3))
        XCTAssertNil(MediaViewer.dots([], at: 0))
    }
}
