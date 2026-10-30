import XCTest
@testable import Arbos

/// The media viewer's ×, pills and toast stay readable over any picture
/// ([feedback image-viewer-close-button-2026-10-29]: "when someone sends a white image the
/// close image button is unreadable"). The white glyph must hold WCAG AA on the control's
/// face over a white picture (the worst case) on Glass and on Matte.
final class ViewerChromeTests: XCTestCase {
    private let materials: [HeaderMaterial] = [.liquidGlass, .frosted, .solid]

    func testGlyphReadsOnAWhitePicture() {
        for m in materials {
            let face = ViewerChrome.faceLevel(material: m, over: 1)
            let contrast = ViewerChrome.glyphContrast(onFace: face)
            XCTAssertGreaterThanOrEqual(contrast, ViewerChrome.minContrast, "\(m): face \(face), contrast \(contrast)")
        }
    }

    func testGlyphReadsOnEveryGrey() {
        for m in materials {
            for step in 0...20 {
                let under = Double(step) / 20
                let contrast = ViewerChrome.glyphContrast(onFace: ViewerChrome.faceLevel(material: m, over: under))
                XCTAssertGreaterThanOrEqual(contrast, ViewerChrome.minContrast, "\(m) over grey \(under)")
            }
        }
    }

    func testTheOldDiscFailedOnWhite() {
        // White 22 % over a white picture is white: the reported bug, as numbers.
        XCTAssertLessThan(ViewerChrome.glyphContrast(onFace: 1), 1.01)
    }

    func testSurfaceKeepsItsMaterial() {
        XCTAssertEqual(ViewerChrome.material(surface: .glass, liquidGlassAvailable: true), .liquidGlass)
        XCTAssertEqual(ViewerChrome.material(surface: .glass, liquidGlassAvailable: false), .frosted)
        XCTAssertEqual(ViewerChrome.material(surface: .matte, liquidGlassAvailable: true), .solid)
    }

    func testRimReadsOnABlackPicture() {
        XCTAssertGreaterThan(ViewerChrome.rimAlpha, ViewerChrome.glassWash)
    }
}
