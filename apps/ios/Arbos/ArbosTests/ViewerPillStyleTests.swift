import XCTest
@testable import Arbos

/// The media viewer's bottom pills fit their labels on every iPhone ([feedback
/// viewer-pill-labels-ios-2026-10-29]: the labels broke inside a word — "Clos e",
/// "Downl oad"). The row is laid out as capsule padding + glyph + inner spacing +
/// label, with a gap between pills and `ViewerPill.rowInset` at each screen edge;
/// these measure that row with the system font and check it against the real widths.
final class ViewerPillStyleTests: XCTestCase {
    /// The pills, in the order the viewer shows them for a chat attachment.
    private let pills: [(label: String, symbol: String)] = [
        ("Close", "xmark"),
        ("Copy", "doc.on.doc"),
        ("Download", "arrow.down.to.line"),
        ("Forward", "arrowshape.turn.up.right"),
    ]

    /// Portrait widths in points: iPhone 17 / 16 / 15, and the narrowest iPhones (SE, mini).
    private let iPhone17: CGFloat = 402
    private let narrowest: CGFloat = 375

    private func labelWidth(_ text: String, size: CGFloat, scale: CGFloat) -> CGFloat {
        // `theme.font` rounds the scaled point size to a half point (`Theme.pt`).
        let pt = (size * scale * 2).rounded() / 2
        let font = UIFont.systemFont(ofSize: pt, weight: .medium)
        return (text as NSString).size(withAttributes: [.font: font]).width
    }

    private func glyphWidth(_ name: String, size: CGFloat) -> CGFloat {
        let config = UIImage.SymbolConfiguration(pointSize: size, weight: .semibold)
        return UIImage(systemName: name, withConfiguration: config)?.size.width ?? size
    }

    /// The row's own width, before the screen inset.
    private func rowWidth(_ size: ViewerPill, scale: CGFloat = 1) -> CGFloat {
        var width: CGFloat = 0
        for pill in pills {
            width += size.padding * 2
            width += glyphWidth(pill.symbol, size: size.glyph)
            width += size.innerSpacing
            width += labelWidth(pill.label, size: size.label, scale: scale)
        }
        return width + size.gap * CGFloat(pills.count - 1)
    }

    /// What a screen of `width` leaves the row.
    private func available(_ width: CGFloat) -> CGFloat { width - ViewerPill.rowInset * 2 }

    func testTheFullRowIsWiderThanAnyIPhone() {
        // The reported bug, as numbers: this is why the labels were squeezed.
        XCTAssertGreaterThan(rowWidth(.full), available(iPhone17),
                             "full row \(rowWidth(.full)) pt vs \(available(iPhone17)) pt")
    }

    func testTheCompactRowFitsEveryIPhone() {
        for width in [narrowest, iPhone17, 430] as [CGFloat] {
            XCTAssertLessThanOrEqual(rowWidth(.compact), available(width),
                                     "compact row \(rowWidth(.compact)) pt vs \(available(width)) pt on \(width) pt")
        }
    }

    /// Close only (a profile picture): the full row must still be the one that shows.
    func testOneFullPillFitsTheNarrowestIPhone() {
        let close = pills[0]
        let width = ViewerPill.full.padding * 2
            + glyphWidth(close.symbol, size: ViewerPill.full.glyph)
            + ViewerPill.full.innerSpacing
            + labelWidth(close.label, size: ViewerPill.full.label, scale: 1)
        XCTAssertLessThanOrEqual(width, available(narrowest))
    }

    /// The app's own text sizes (`TextScale.steps`), not Dynamic Type.
    func testTheCompactRowHoldsTheDefaultAndSmallerTextSizes() {
        for scale in TextScale.steps where scale <= 1 {
            XCTAssertLessThanOrEqual(rowWidth(.compact, scale: scale), available(narrowest),
                                     "compact row at text scale \(scale)")
        }
    }

    func testTheCompactSizeIsSmallerEverywhere() {
        let (full, compact) = (ViewerPill.full, ViewerPill.compact)
        XCTAssertLessThan(compact.label, full.label)
        XCTAssertLessThan(compact.glyph, full.glyph)
        XCTAssertLessThan(compact.padding, full.padding)
        XCTAssertLessThan(compact.innerSpacing, full.innerSpacing)
        XCTAssertLessThan(compact.gap, full.gap)
        XCTAssertLessThan(compact.height, full.height)
    }

    /// The full size is the design size the viewer shipped with; only the inset is new.
    func testTheFullSizeIsUnchanged() {
        XCTAssertEqual(ViewerPill.full.label, 15)
        XCTAssertEqual(ViewerPill.full.glyph, 14)
        XCTAssertEqual(ViewerPill.full.padding, 16)
        XCTAssertEqual(ViewerPill.full.height, 38)
        XCTAssertEqual(ViewerPill.full.gap, 12)
        XCTAssertGreaterThan(ViewerPill.rowInset, 0)
    }
}
