import XCTest
import UIKit
@testable import Arbos

/// The media viewer's bottom pills fit their labels on every iPhone, at every step of the
/// app's Text Size ([feedback viewer-pill-labels-ios-2026-10-29]: the labels broke inside a
/// word — "Clos e", "Downl oad"). Each pill is laid out as capsule padding + glyph + inner
/// spacing + label, with a gap between pills and the size's own `inset` at each screen edge;
/// these measure that row with the system font and check it against the real widths.
///
/// The viewer has no Dynamic Type: every label goes through `theme.font`, which multiplies a
/// design point size by the user's Text Size (`TextScale.steps`, 0.85 … 1.3). The largest
/// step, 1.3, is the largest size the app ships, so that is where the labels must still fit.
final class ViewerPillStyleTests: XCTestCase {
    /// The pills, in the order the viewer shows them for a chat attachment.
    private let pills: [(label: String, symbol: String)] = [
        ("Close", "xmark"),
        ("Copy", "doc.on.doc"),
        ("Download", "arrow.down.to.line"),
        ("Forward", "arrowshape.turn.up.right"),
    ]

    /// Portrait widths in points, narrowest first: iPhone SE under Display Zoom, SE / mini,
    /// iPhone 17 / 16 / 15, and the Pro Max. The sheet header measures the same set.
    private let widths: [CGFloat] = [320, 375, 393, 402, 430]
    private let iPhone17: CGFloat = 402
    private let narrowest: CGFloat = 320

    private func labelWidth(_ text: String, size: CGFloat, scale: CGFloat) -> CGFloat {
        let font = UIFont.systemFont(ofSize: labelPoints(size, scale: scale), weight: .medium)
        return (text as NSString).size(withAttributes: [.font: font]).width
    }

    /// The point size `theme.font` asks for: the design size times the Text Size step,
    /// rounded to a half point (`Theme.pt`).
    private func labelPoints(_ size: CGFloat, scale: CGFloat) -> CGFloat {
        (size * scale * 2).rounded() / 2
    }

    private func glyphWidth(_ name: String, size: CGFloat) -> CGFloat {
        let config = UIImage.SymbolConfiguration(pointSize: size, weight: .semibold)
        return UIImage(systemName: name, withConfiguration: config)?.size.width ?? size
    }

    /// Everything in the row that the Text Size does not move: the capsule padding, the
    /// glyphs, the glyph → label spacing, and the gaps between pills.
    private func chromeWidth(_ size: ViewerPill) -> CGFloat {
        var width = size.gap * CGFloat(pills.count - 1)
        for pill in pills {
            width += size.padding * 2 + glyphWidth(pill.symbol, size: size.glyph) + size.innerSpacing
        }
        return width
    }

    private func labelsWidth(_ size: ViewerPill, scale: CGFloat) -> CGFloat {
        pills.reduce(0) { $0 + labelWidth($1.label, size: size.label, scale: scale) }
    }

    /// The row's own width, before the screen inset.
    private func rowWidth(_ size: ViewerPill, scale: CGFloat = 1) -> CGFloat {
        chromeWidth(size) + labelsWidth(size, scale: scale)
    }

    /// What a screen of `width` leaves this size's row.
    private func available(_ size: ViewerPill, _ width: CGFloat) -> CGFloat {
        width - size.inset * 2
    }

    /// The factor the labels have to be drawn at to fit — what `minimumScaleFactor` is asked
    /// for. 1 means they fit at their own width; below `ViewerPill.minimumLabelScale` a label
    /// would be cut with an ellipsis instead.
    private func requiredLabelScale(_ size: ViewerPill, width: CGFloat, scale: CGFloat) -> CGFloat {
        let labels = labelsWidth(size, scale: scale)
        guard labels > 0 else { return 1 }
        let room = available(size, width) - chromeWidth(size)
        return min(1, max(0, room) / labels)
    }

    /// The size `ViewThatFits` lands on: the first of `ViewerPill.sizes` whose row fits the
    /// screen whole, and the last one when none does.
    private func chosenSize(width: CGFloat, scale: CGFloat) -> ViewerPill {
        ViewerPill.sizes.first { rowWidth($0, scale: scale) <= available($0, width) } ?? .tight
    }

    private func context(_ width: CGFloat, _ scale: CGFloat) -> String {
        "\(Int(width)) pt at Text Size \(scale)"
    }

    // MARK: The bug

    func testTheFullRowIsWiderThanAnyIPhone() {
        // The reported bug, as numbers: this is why the labels were squeezed.
        XCTAssertGreaterThan(rowWidth(.full), available(.full, iPhone17),
                             "full row \(rowWidth(.full)) pt vs \(available(.full, iPhone17)) pt")
    }

    /// The whole requirement in one test: on every iPhone width, at every Text Size step the
    /// app ships — up to the largest, 1.3 — the row the viewer draws keeps all four labels on
    /// one line and whole. A label is whole when it needs no more shrink than
    /// `ViewerPill.minimumLabelScale`; past that point a one-line label is cut.
    func testEveryLabelStaysWholeOnOneLineAtEveryTextSize() {
        for width in widths {
            for scale in TextScale.steps {
                let size = chosenSize(width: width, scale: scale)
                let required = requiredLabelScale(size, width: width, scale: scale)
                XCTAssertGreaterThanOrEqual(
                    required, ViewerPill.minimumLabelScale,
                    "\(context(width, scale)): the labels need \(required) of their width, "
                        + "below the \(ViewerPill.minimumLabelScale) floor, so one would be cut")
            }
        }
    }

    /// The largest Text Size the app ships, on the narrowest iPhone — the corner the tester
    /// hit. The labels fit, and a shrunk label is still no smaller than the same label at the
    /// app's smallest Text Size step, so the largest step never reads smaller than the
    /// smallest one.
    func testTheLargestTextSizeKeepsTheLabelsWholeOnTheNarrowestIPhone() throws {
        let largest = try XCTUnwrap(TextScale.steps.max())
        let smallest = try XCTUnwrap(TextScale.steps.min())
        XCTAssertEqual(largest, 1.3, "the largest step the app ships")
        let size = chosenSize(width: narrowest, scale: largest)
        let required = requiredLabelScale(size, width: narrowest, scale: largest)
        XCTAssertGreaterThanOrEqual(required, ViewerPill.minimumLabelScale,
                                    "needs \(required) of the label width on \(Int(narrowest)) pt")
        let drawn = labelPoints(size.label, scale: largest) * required
        XCTAssertGreaterThanOrEqual(drawn, labelPoints(size.label, scale: smallest),
                                    "\(drawn) pt drawn at the largest step vs "
                                        + "\(labelPoints(size.label, scale: smallest)) pt at the smallest")
    }

    /// Close only (a profile picture): the full row must still be the one that shows, at every
    /// Text Size — one pill is narrow enough that nothing ever gives way.
    func testOneFullPillFitsEveryIPhoneAtEveryTextSize() {
        let close = pills[0]
        for width in widths {
            for scale in TextScale.steps {
                let pillWidth = ViewerPill.full.padding * 2
                    + glyphWidth(close.symbol, size: ViewerPill.full.glyph)
                    + ViewerPill.full.innerSpacing
                    + labelWidth(close.label, size: ViewerPill.full.label, scale: scale)
                XCTAssertLessThanOrEqual(pillWidth, available(.full, width), context(width, scale))
            }
        }
    }

    // MARK: The sizes

    func testTheCompactRowFitsEveryIPhoneAtTheDefaultTextSizeAndBelow() {
        for width in widths where width >= 375 {
            for scale in TextScale.steps where scale <= 1 {
                XCTAssertLessThanOrEqual(rowWidth(.compact, scale: scale), available(.compact, width),
                                         "compact row at \(context(width, scale))")
            }
        }
    }

    func testEachSizeIsSmallerThanTheOneAboveIt() {
        for (wider, narrower) in zip(ViewerPill.sizes, ViewerPill.sizes.dropFirst()) {
            XCTAssertLessThanOrEqual(narrower.label, wider.label)
            XCTAssertLessThan(narrower.glyph, wider.glyph)
            XCTAssertLessThan(narrower.padding, wider.padding)
            XCTAssertLessThan(narrower.innerSpacing, wider.innerSpacing)
            XCTAssertLessThan(narrower.gap, wider.gap)
            XCTAssertLessThanOrEqual(narrower.height, wider.height)
            XCTAssertLessThanOrEqual(narrower.inset, wider.inset)
            XCTAssertLessThan(rowWidth(narrower), rowWidth(wider))
        }
    }

    /// Only the widest size keeps its labels at their own width; the others let the label give.
    func testOnlyTheFullSizeRefusesToSqueezeItsLabels() {
        XCTAssertTrue(ViewerPill.sizes[0].labelKeepsItsWidth)
        XCTAssertEqual(ViewerPill.sizes[0].label, ViewerPill.full.label)
        for size in ViewerPill.sizes.dropFirst() {
            XCTAssertFalse(size.labelKeepsItsWidth)
        }
        XCTAssertEqual(ViewerPill.sizes.last?.padding, ViewerPill.tight.padding, "tight is the floor")
        XCTAssertGreaterThan(ViewerPill.minimumLabelScale, 0)
        XCTAssertLessThan(ViewerPill.minimumLabelScale, 1)
    }

    /// The full size is the design size the viewer shipped with; only the inset is new.
    func testTheFullSizeIsUnchanged() {
        XCTAssertEqual(ViewerPill.full.label, 15)
        XCTAssertEqual(ViewerPill.full.glyph, 14)
        XCTAssertEqual(ViewerPill.full.padding, 16)
        XCTAssertEqual(ViewerPill.full.height, 38)
        XCTAssertEqual(ViewerPill.full.gap, 12)
        XCTAssertGreaterThan(ViewerPill.full.inset, 0)
    }

    // MARK: Source structure

    private var sourceRoot: URL {
        URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().appendingPathComponent("Arbos/UI")
    }

    /// The two lines that make the guarantee hold at run time: one line per label, and the
    /// label shrinking to the floor rather than being cut. Plus every size in `sizes` being
    /// offered to `ViewThatFits`, which is what `chosenSize` above models.
    func testTheViewerAppliesTheOneLineRule() throws {
        let file = sourceRoot.appendingPathComponent("MediaViewer.swift")
        try XCTSkipIf(!FileManager.default.fileExists(atPath: file.path), "sources not beside the test bundle")
        let text = try String(contentsOf: file, encoding: .utf8)
        XCTAssertTrue(text.contains(".lineLimit(1)"), "a pill label is one line")
        XCTAssertTrue(text.contains(".minimumScaleFactor(ViewerPill.minimumLabelScale)"),
                      "the label shrinks to the floor instead of being cut")
        for name in ["pillRow(.full)", "pillRow(.compact)", "pillRow(.tight)"] {
            XCTAssertTrue(text.contains(name), "\(name) is offered to ViewThatFits")
        }
    }
}
