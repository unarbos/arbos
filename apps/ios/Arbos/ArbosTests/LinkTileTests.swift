import SwiftUI
import UIKit
import XCTest
@testable import Arbos

/// The one link-icon tile ([feedback link-icons-ios-tile-2026-10-27]): known hosts map to a
/// bundled brand mark that exists in the asset catalog; the Google editors are told apart by
/// path; unknown hosts have none; the fill flips only across a mark's own tone; metrics follow
/// the pane kit (36 → corner 10, mark 20).
@MainActor
final class LinkTileTests: XCTestCase {
    private func url(_ s: String) -> URL { URL(string: s)! }

    func testKnownHostsMapToTheirBrand() {
        XCTAssertEqual(LinkBrand.of(url: url("https://github.com/unarbos/arbos")), .github)
        XCTAssertEqual(LinkBrand.of(url: url("https://www.github.com/x")), .github)
        XCTAssertEqual(LinkBrand.of(url: url("https://gist.github.com/a/b")), .github)
        XCTAssertEqual(LinkBrand.of(url: url("https://x.com/arbos/status/1")), .x)
        XCTAssertEqual(LinkBrand.of(url: url("https://twitter.com/arbos")), .x)
        XCTAssertEqual(LinkBrand.of(url: url("https://youtu.be/dQw4w9WgXcQ")), .youtube)
        XCTAssertEqual(LinkBrand.of(url: url("https://open.spotify.com/track/1?si=x")), .spotify)
        XCTAssertEqual(LinkBrand.of(url: url("https://music.apple.com/us/album/1")), .appleMusic)
        XCTAssertEqual(LinkBrand.of(url: url("https://soundcloud.com/a/b")), .soundcloud)
        XCTAssertEqual(LinkBrand.of(url: url("https://vimeo.com/1")), .vimeo)
        XCTAssertEqual(LinkBrand.of(url: url("https://www.loom.com/share/1")), .loom)
        XCTAssertEqual(LinkBrand.of(url: url("https://bsky.app/profile/a")), .bluesky)
        XCTAssertEqual(LinkBrand.of(url: url("https://old.reddit.com/r/rust")), .reddit)
        XCTAssertEqual(LinkBrand.of(url: url("https://www.figma.com/file/1")), .figma)
        XCTAssertEqual(LinkBrand.of(url: url("https://arbos.notion.site/page")), .notion)
        XCTAssertEqual(LinkBrand.of(url: url("https://linear.app/arbos/issue/ARB-1")), .linear)
        XCTAssertEqual(LinkBrand.of(url: url("https://en.wikipedia.org/wiki/Tree")), .wikipedia)
        XCTAssertEqual(LinkBrand.of(url: url("https://arbos.life/invite/abc")), .arbos)
        XCTAssertEqual(LinkBrand.of(url: url("https://jacob.arbos.life/app/")), .arbos)
        XCTAssertEqual(LinkBrand.of(url: url("https://maps.app.goo.gl/abc")), .googleMaps)
        XCTAssertEqual(LinkBrand.of(url: url("https://www.google.com/maps/place/Lima")), .googleMaps)
        XCTAssertNil(LinkBrand.of(url: url("https://www.google.com/search?q=x")))
        XCTAssertEqual(LinkBrand.of(host: "www.github.com"), .github)
    }

    func testTheGoogleEditorsAreToldApartByPath() {
        XCTAssertEqual(LinkBrand.of(url: url("https://docs.google.com/document/d/1/edit")), .googleDocs)
        XCTAssertEqual(LinkBrand.of(url: url("https://docs.google.com/spreadsheets/d/1/edit")), .googleSheets)
        XCTAssertEqual(LinkBrand.of(url: url("https://docs.google.com/presentation/d/1/edit")), .googleSlides)
        XCTAssertEqual(LinkBrand.of(host: "docs.google.com"), .googleDocs)
    }

    func testUnknownHostsHaveNoBrand() {
        for s in ["https://nypost.com/2026/10/27/story", "https://example.com", "https://githubusercontent.evil.com/x",
                  "https://notgithub.com", "https://x.company.com", "mailto:a@b.com"] {
            XCTAssertNil(LinkBrand.of(url: url(s)), s)
        }
    }

    func testEveryBrandHasAVectorAssetAndAnInk() {
        for brand in LinkBrand.allCases {
            XCTAssertNotNil(UIImage(named: brand.assetName), "\(brand) → \(brand.assetName) in the asset catalog")
            XCTAssertFalse(brand.name.isEmpty)
        }
        XCTAssertEqual(LinkBrand.github.ink, .mono)
        XCTAssertEqual(LinkBrand.x.ink, .mono)
        XCTAssertEqual(LinkBrand.notion.ink, .mono)
        XCTAssertEqual(LinkBrand.wikipedia.ink, .mono)
        XCTAssertEqual(LinkBrand.arbos.ink, .mono)
        XCTAssertEqual(LinkBrand.spotify.ink, .color(0x1ED760))
        XCTAssertEqual(LinkBrand.youtube.ink, .color(0xFF0000))
    }

    func testMetricsFollowThePaneKit() {
        XCTAssertEqual(LinkTile.radius(36), 10, "pane row tile = kit corner")
        XCTAssertEqual(LinkTile.radius(32), 9, "library thumb")
        XCTAssertEqual(LinkTile.radius(14), 4, "inline icon = quarter")
        XCTAssertEqual(LinkTile.markSide(36), 20)
        XCTAssertEqual(LinkTile.initial("nypost.com"), "N")
        XCTAssertEqual(LinkTile.initial("www.elvirrey.com"), "E")
        XCTAssertEqual(LinkTile.initial("3dprint.io"), "3")
        XCTAssertEqual(LinkTile.initial(""), "#")
    }

    private func picture(side: Int, paint: (Int, Int) -> UIColor?) -> UIImage {
        let r = UIGraphicsImageRenderer(size: CGSize(width: side, height: side), format: {
            let f = UIGraphicsImageRendererFormat.default(); f.scale = 1; f.opaque = false; return f
        }())
        return r.image { ctx in
            for y in 0..<side {
                for x in 0..<side {
                    if let c = paint(x, y) {
                        c.setFill()
                        ctx.fill(CGRect(x: x, y: y, width: 1, height: 1))
                    }
                }
            }
        }
    }

    func testTonesTellADarkMarkFromALightOneAndAnOpaqueIcon() {
        // The GitHub favicon shape: black octocat on a transparent field.
        let octocat = picture(side: 32) { x, y in (x - 16) * (x - 16) + (y - 16) * (y - 16) < 120 ? UIColor(white: 0.08, alpha: 1) : nil }
        XCTAssertEqual(LinkIconTone.of(octocat), .darkMark)
        let whiteMark = picture(side: 32) { x, _ in x < 20 ? UIColor(white: 0.98, alpha: 1) : nil }
        XCTAssertEqual(LinkIconTone.of(whiteMark), .lightMark)
        // A full-bleed apple-touch-icon: opaque, whatever its colour.
        let opaque = picture(side: 32) { _, _ in UIColor(white: 0.04, alpha: 1) }
        XCTAssertEqual(LinkIconTone.of(opaque), .neutral)
        // A green mark on transparency is mid-tone: the theme's own fill is fine.
        let green = picture(side: 32) { x, _ in x < 24 ? UIColor(red: 30 / 255, green: 215 / 255, blue: 96 / 255, alpha: 1) : nil }
        XCTAssertEqual(LinkIconTone.of(green), .neutral)
    }

    func testFillsFlipSoAMarkNeverSitsOnItsOwnTone() {
        let dark = Theme.dark, light = Theme.light
        XCTAssertTrue(dark.isDark)
        XCTAssertFalse(light.isDark)
        XCTAssertEqual(LinkTile.fill(.brand(.github), theme: dark), dark.field)
        XCTAssertEqual(LinkTile.fill(.brand(.github), theme: light), light.field)
        XCTAssertEqual(LinkTile.fill(.picture(.neutral), theme: dark), dark.field)
        XCTAssertEqual(LinkTile.fill(.picture(.darkMark), theme: dark), Color(hex: "#ECECEE")!)
        XCTAssertEqual(LinkTile.fill(.picture(.darkMark), theme: light), light.field)
        XCTAssertEqual(LinkTile.fill(.picture(.lightMark), theme: light), Color(hex: "#2C2C2E")!)
        XCTAssertEqual(LinkTile.fill(.picture(.lightMark), theme: dark), dark.field)
        // GitHub's mark is the text colour on both; Spotify keeps its green.
        XCTAssertEqual(LinkBrand.github.color(dark), dark.text)
        XCTAssertEqual(LinkBrand.github.color(light), light.text)
        XCTAssertEqual(LinkBrand.spotify.color(dark), LinkBrand.spotify.color(light))
    }

    func testTheKindPrefersTheBrandThenThePictureThenTheLetter() {
        let img = picture(side: 4) { _, _ in .red }
        XCTAssertEqual(LinkTile.kind(url: url("https://github.com/a"), host: "github.com", picture: (img, .neutral)), .brand(.github))
        XCTAssertEqual(LinkTile.kind(url: url("https://nypost.com/a"), host: "nypost.com", picture: (img, .darkMark)), .picture(.darkMark))
        XCTAssertEqual(LinkTile.kind(url: url("https://nypost.com/a"), host: "nypost.com", picture: nil), .initial("N"))
        XCTAssertEqual(LinkRowTile.iconHost(url("https://www.NYPost.com/x")), "www.nypost.com")
    }
}
