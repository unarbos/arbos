import XCTest
@testable import Arbos

/// The media viewer's caption (sender + when), counter and current page dot stay readable
/// over any picture ([feedback viewer-caption-white-2026-10-29]: after #2288 gave them a
/// dark halo, white text over a pure white picture was still faint). They now sit on the
/// controls' plate (`ViewerChrome`); each part must hold its WCAG minimum on that face over
/// every grey from black to white, on Glass, frost and Matte.
final class ViewerCaptionContrastTests: XCTestCase {
    private let materials: [HeaderMaterial] = [.liquidGlass, .frosted, .solid]

    private func contrast(_ part: ViewerChromeText, _ material: HeaderMaterial, over under: Double) -> Double {
        let face = ViewerChrome.faceLevel(material: material, over: under)
        return ViewerChrome.contrast(ViewerChrome.textLevel(alpha: part.alpha, onFace: face), face)
    }

    func testEveryPartReadsOnAWhitePicture() {
        for part in ViewerChromeText.allCases {
            for m in materials {
                let c = contrast(part, m, over: 1)
                XCTAssertGreaterThanOrEqual(c, part.minContrast, "\(part) on \(m) over white: \(c)")
            }
        }
    }

    func testEveryPartReadsOnEveryGrey() {
        for part in ViewerChromeText.allCases {
            for m in materials {
                for step in 0...20 {
                    let under = Double(step) / 20
                    XCTAssertGreaterThanOrEqual(contrast(part, m, over: under), part.minContrast, "\(part) on \(m) over grey \(under)")
                }
            }
        }
    }

    func testTheOldHaloCaptionFailedOnWhite() {
        // White text straight on a white picture (the halo only blurs a thin edge): 1:1.
        XCTAssertLessThan(ViewerChrome.contrast(1, 1), 1.01)
        // And the old 67 % subline on today's Glass face over white would still fall under AA.
        let face = ViewerChrome.faceLevel(material: .liquidGlass, over: 1)
        XCTAssertLessThan(ViewerChrome.contrast(ViewerChrome.textLevel(alpha: 0.67, onFace: face), face), ViewerChrome.minContrast)
    }

    func testTheCurrentDotStandsOutFromTheOthers() {
        XCTAssertGreaterThan(ViewerChromeText.activeDot.alpha, ViewerChrome.inactiveDotAlpha)
    }

    func testContrastIsSymmetric() {
        XCTAssertEqual(ViewerChrome.contrast(0.2, 0.9), ViewerChrome.contrast(0.9, 0.2), accuracy: 1e-12)
        XCTAssertEqual(ViewerChrome.contrast(1, 0), 21, accuracy: 0.01)
        XCTAssertEqual(ViewerChrome.glyphContrast(onFace: 0.3), ViewerChrome.contrast(1, 0.3), accuracy: 1e-12)
    }
}
