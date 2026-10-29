import SwiftUI
import XCTest
@testable import Arbos

/// [feedback mention-chip-highlight-2026-10-28] (Jacob's iPhone shot, "odd pfp highlights … clearly
/// not centered"): every mention chip is ONE pill behind its picture and name — no second box behind
/// the name — with the same inset at both ends and the picture on the pill's vertical centre.
/// The line is rendered for real (`ImageRenderer`, the bubble's font and line spacing) with the pill
/// in an opaque marker colour, then measured; the PNG is attached to the test result.
@MainActor
final class MentionChipSnapshotTests: XCTestCase {
    private let scale: CGFloat = 3
    private let marker = Pixel(r: 0, g: 255, b: 0)

    func testEachChipIsOnePillWithEvenInsetsAndThePictureOnItsCentre() throws {
        guard #available(iOS 18.0, *) else { throw XCTSkip("the pill renderer needs iOS 18") }
        let shot = try render("@comms and @unconst, the embargo lifts at 9.", name: "mention-chips-one-line")
        let pills = pillBoxes(in: shot)
        XCTAssertEqual(pills.count, 2, "one pill per chip, nothing else in the marker colour")
        for pill in pills {
            try assertOnePill(pill, in: shot)
        }
    }

    func testAChipOnALaterLineSitsOnThatLine() throws {
        guard #available(iOS 18.0, *) else { throw XCTSkip("the pill renderer needs iOS 18") }
        let shot = try render("a first line of plain words that wraps well before it reaches @comms near the end",
                              name: "mention-chips-second-line", width: 230)
        let pills = pillBoxes(in: shot)
        XCTAssertEqual(pills.count, 1)
        let pill = try XCTUnwrap(pills.first)
        XCTAssertGreaterThan(CGFloat(pill.minY) / scale, 12 + Theme.body, "the chip is on a later line, not the first")
        try assertOnePill(pill, in: shot)
    }

    // MARK: Measuring

    /// The marker regions at least half a pill tall: the counters of letters such as "o" on the
    /// pill are marker colour too, but closed off from it by the glyph.
    private func pillBoxes(in shot: Bitmap) -> [Box] {
        shot.components(near: marker).filter { CGFloat($0.height) / scale > MentionTable.pillHeight / 2 }
    }

    private func assertOnePill(_ pill: Box, in shot: Bitmap, file: StaticString = #filePath, line: UInt = #line) throws {
        let pt = scale
        XCTAssertEqual(CGFloat(pill.height) / pt, MentionTable.pillHeight, accuracy: 1, "pill height", file: file, line: line)
        // A single box: inside the rounded ends every column's marker starts on the top edge and ends
        // on the bottom edge. A second, offset box behind the name (the bug) breaks this.
        let inset = Int((MentionTable.radius * pt).rounded(.up))
        for x in (pill.minX + inset)...(pill.maxX - inset) {
            guard let top = shot.firstY(near: marker, x: x, in: pill.minY...pill.maxY),
                  let bottom = shot.lastY(near: marker, x: x, in: pill.minY...pill.maxY) else {
                return XCTFail("column \(x) has no pill", file: file, line: line)
            }
            XCTAssertLessThanOrEqual(top - pill.minY, 1, "column \(x) starts below the pill's top edge", file: file, line: line)
            XCTAssertLessThanOrEqual(pill.maxY - bottom, 1, "column \(x) ends above the pill's bottom edge", file: file, line: line)
        }
        // What sits on the pill: the disc on the left, the name's glyphs after it.
        let content = try XCTUnwrap(shot.content(in: pill), "a picture and a name on the pill", file: file, line: line)
        let left = CGFloat(content.minX - pill.minX) / pt
        let right = CGFloat(pill.maxX - content.maxX) / pt
        XCTAssertEqual(left, MentionTable.padX, accuracy: 1, "inset before the picture", file: file, line: line)
        // The glyph's side bearing adds under a point after the name.
        XCTAssertEqual(right, left, accuracy: 1.5, "inset after the name matches the one before the picture", file: file, line: line)
        let disc = try XCTUnwrap(shot.content(in: Box(minX: content.minX, minY: pill.minY,
                                                      maxX: content.minX + Int(MentionTable.picture * pt) - 1, maxY: pill.maxY)),
                                  file: file, line: line)
        XCTAssertEqual(CGFloat(disc.height) / pt, MentionTable.picture, accuracy: 1, "the whole disc is on the pill", file: file, line: line)
        XCTAssertEqual(disc.midY, pill.midY, accuracy: Double(pt) * 0.75, "the disc is on the pill's vertical centre", file: file, line: line)
    }

    @available(iOS 18.0, *)
    private func render(_ message: String, name: String, width: CGFloat = 360) throws -> Bitmap {
        let model = AppModel()
        model.loadFixture()
        let table = MentionTable(model: model, theme: Theme.dark)
        let attr = Markdown.inline(table.display(message), color: .white, link: .white, codeChip: .gray)
        let text = table.text(attr, link: .white, pill: Color(red: 0, green: 1, blue: 0), lineHeight: Theme.body * 1.2)
        let view = text
            .font(.system(size: Theme.body))
            .lineSpacing(Theme.bodyLineSpacing)
            .mentionPills()
            .frame(width: width, alignment: .leading)
            .padding(12)
            .background(Color.black)
            .environment(\.colorScheme, .dark)
        let renderer = ImageRenderer(content: view)
        renderer.scale = scale
        let image = try XCTUnwrap(renderer.uiImage, "rendered")
        let attachment = XCTAttachment(image: image)
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
        return try Bitmap(image)
    }
}

private struct Pixel: Equatable {
    var r: Int, g: Int, b: Int

    func near(_ o: Pixel, _ tolerance: Int = 24) -> Bool {
        abs(r - o.r) <= tolerance && abs(g - o.g) <= tolerance && abs(b - o.b) <= tolerance
    }

    /// The marker green, the black ground, or a blend of the two (the pill's antialiased rim).
    var isPillOrGround: Bool { r < 40 && b < 40 }
}

private struct Box {
    var minX: Int, minY: Int, maxX: Int, maxY: Int
    var height: Int { maxY - minY + 1 }
    var midY: Double { Double(minY + maxY) / 2 }
}

/// An RGBA copy of the rendered image, addressed in pixels.
private struct Bitmap {
    let width: Int
    let height: Int
    private let bytes: [UInt8]

    init(_ image: UIImage) throws {
        let cg = try XCTUnwrap(image.cgImage)
        width = cg.width
        height = cg.height
        var data = [UInt8](repeating: 0, count: width * height * 4)
        let ctx = try XCTUnwrap(CGContext(data: &data, width: width, height: height, bitsPerComponent: 8, bytesPerRow: width * 4,
                                          space: CGColorSpaceCreateDeviceRGB(),
                                          bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue))
        ctx.draw(cg, in: CGRect(x: 0, y: 0, width: width, height: height))
        bytes = data
    }

    func at(_ x: Int, _ y: Int) -> Pixel {
        let i = (y * width + x) * 4
        return Pixel(r: Int(bytes[i]), g: Int(bytes[i + 1]), b: Int(bytes[i + 2]))
    }

    /// Bounding boxes of the 4-connected regions in `colour` (antialiased rims excluded).
    func components(near colour: Pixel) -> [Box] {
        var seen = [Bool](repeating: false, count: width * height)
        var boxes: [Box] = []
        for y in 0..<height {
            for x in 0..<width where !seen[y * width + x] && at(x, y).near(colour) {
                var box = Box(minX: x, minY: y, maxX: x, maxY: y)
                var stack = [(x, y)]
                seen[y * width + x] = true
                while let (cx, cy) = stack.popLast() {
                    box.minX = min(box.minX, cx); box.maxX = max(box.maxX, cx)
                    box.minY = min(box.minY, cy); box.maxY = max(box.maxY, cy)
                    for (nx, ny) in [(cx + 1, cy), (cx - 1, cy), (cx, cy + 1), (cx, cy - 1)]
                    where nx >= 0 && ny >= 0 && nx < width && ny < height && !seen[ny * width + nx] && at(nx, ny).near(colour) {
                        seen[ny * width + nx] = true
                        stack.append((nx, ny))
                    }
                }
                // Specks from antialiasing elsewhere are not pills.
                if box.height > 6 && box.maxX - box.minX > 6 { boxes.append(box) }
            }
        }
        return boxes
    }

    func firstY(near colour: Pixel, x: Int, in ys: ClosedRange<Int>) -> Int? {
        ys.first { at(x, $0).near(colour) }
    }

    func lastY(near colour: Pixel, x: Int, in ys: ClosedRange<Int>) -> Int? {
        ys.reversed().first { at(x, $0).near(colour) }
    }

    /// The box around everything inside `box` that is neither the pill nor the background
    /// behind its rounded ends: the picture and the name.
    func content(in box: Box) -> Box? {
        var out: Box?
        for y in box.minY...box.maxY {
            for x in box.minX...box.maxX {
                guard !at(x, y).isPillOrGround else { continue }
                if out == nil { out = Box(minX: x, minY: y, maxX: x, maxY: y) }
                out!.minX = min(out!.minX, x); out!.maxX = max(out!.maxX, x)
                out!.minY = min(out!.minY, y); out!.maxY = max(out!.maxY, y)
            }
        }
        return out
    }
}
