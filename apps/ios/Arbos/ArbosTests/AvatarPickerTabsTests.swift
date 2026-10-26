import XCTest
import ArbosFFI
@testable import Arbos

/// The avatar picker offers the same four controls to every target (Jacob, 2026-10-24: the
/// group / server picker had only Bot / Upload / Reset). My own picture (Jacob, 2026-10-25)
/// opens the same sheet without the leaf tab: Generate | Upload, Reset in the bar.
final class AvatarPickerTabsTests: XCTestCase {
    func testEveryTargetGetsBotGenerateUploadAndReset() {
        for target in [AvatarTarget.agent("a"), .group("g"), .server("s")] {
            XCTAssertEqual(AvatarPickerSheet.tabs(for: target), [.bot, .generate, .upload], "\(target)")
        }
        XCTAssertEqual(AvatarPickerTab.allCases.count, 3)
    }

    func testMyProfileGetsGenerateAndUploadWithoutALeafTab() {
        let tabs = AvatarPickerSheet.tabs(for: .me)
        XCTAssertEqual(tabs, [.generate, .upload])
        XCTAssertFalse(tabs.contains(.bot), "people are coloured initials, not leaves")
        XCTAssertEqual(tabs.first, .generate, "Generate is the landing tab")
    }

    /// Jacob, 2026-10-25 ([feedback create-sheet-avatar-upload-generate-ios-2026-10-25]): the
    /// Create New Agent / Group / Server sheets offer Upload and Generate only — no Bot tab, so
    /// no leaf grid and no pastel swatches — Upload first. The Mac twin is
    /// `crates/arbos-mac/src/ui/create_entity.rs` `tests`.
    func testTheCreateSheetOffersUploadAndGenerateOnly() {
        XCTAssertEqual(CreateEntityView.options, [.upload, .generate])
        XCTAssertFalse(CreateEntityView.options.contains(.bot), "no leaf grid, no swatches")
        XCTAssertEqual(Set(CreateEntityView.options).count, 2)
    }

    /// An entity created without a pick is stored with the neutral pair (255 / 255 — the relay's
    /// `leaf::NEUTRAL`), never an automatic leaf; the apps draw it as the name's first letter on a
    /// grey disc. Existing agents (nil = automatic leaf) are untouched.
    func testANewEntityIsStoredNeutralNeverWithAnAutomaticLeaf() {
        let stored = CreateEntityView.storedLeaf
        XCTAssertEqual(Int(stored.leaf), LeafArt.neutral)
        XCTAssertEqual(Int(stored.color), LeafArt.neutral)
        XCTAssertEqual(LeafArt.neutral, 255)
        XCTAssertTrue(LeafArt.isNeutral(255))
        XCTAssertFalse(LeafArt.isNeutral(nil), "nil stays the automatic leaf")
        XCTAssertFalse(LeafArt.isNeutral(3))
        for seed in ["new-agent-0", "new-group-3", "new-server-1"] {
            XCTAssertNotEqual(LeafArt.pick(seed: seed), stored, seed)
        }
        // The shared resolver ignores the sentinel for the leaf in effect (an older client draws
        // the automatic leaf); `agentProject` reads the neutral pair off the card instead.
        let card = AgentCard(id: "01N", name: "Field notes", handle: "field-notes",
                             leaf: UInt8(LeafArt.neutral), leafColor: UInt8(LeafArt.neutral), colorIndex: nil, imageKey: nil, avatarVersion: 0)
        let who = resolveAgent(sender: "agent:01N", handle: nil, name: nil, cards: [card])
        XCTAssertTrue(who.known)
        XCTAssertLessThan(who.leaf.leaf, UInt8(LeafArt.count))
    }

    /// The letter on the neutral disc: the first letter or digit, upper-cased; `?` when empty.
    func testTheNeutralInitialIsTheFirstLetter() {
        XCTAssertEqual(LeafArt.initial("Field notes"), "F")
        XCTAssertEqual(LeafArt.initial("  @ops-bot"), "O")
        XCTAssertEqual(LeafArt.initial("élan"), "É")
        XCTAssertEqual(LeafArt.initial("42 things"), "4")
        XCTAssertEqual(LeafArt.initial(""), "?")
        XCTAssertEqual(LeafArt.initial("—"), "?")
    }

    /// A group or server whose stored pair is neutral draws no leaf (`leafPick` is nil) and says so.
    func testNeutralGroupsAndServersDrawNoLeaf() {
        var chat = PeopleChat(id: "g1", kind: "group", createdBy: "u", createdAt: 0, title: "Launch crew", pending: false,
                              members: [], headSeq: 0, readSeq: 0, unread: false, lockState: "people_only", lastActivityAt: 0,
                              myRole: "owner", muted: false, silenced: false)
        chat.avatarLeaf = LeafArt.neutral
        chat.avatarLeafColor = LeafArt.neutral
        XCTAssertTrue(chat.isNeutralAvatar)
        XCTAssertNil(chat.leafPick)
        chat.avatarLeaf = 4
        chat.avatarLeafColor = 2
        XCTAssertFalse(chat.isNeutralAvatar)
        XCTAssertEqual(chat.leafPick, LeafPick(leaf: 4, color: 2))
    }

    /// The Create sheet hands the create exactly what was picked: nothing (→ neutral), the photo
    /// JPEG, or the generated key — never the other tab's choice.
    @MainActor
    func testTheDraftHandsOverItsPick() {
        let source = AvatarSourceState()
        XCTAssertNil(source.draftPicture(for: .upload))
        XCTAssertNil(source.draftPicture(for: .generate))
        source.uploadJpeg = Data([0xFF, 0xD8])
        XCTAssertEqual(source.draftPicture(for: .upload), .jpeg(Data([0xFF, 0xD8])))
        XCTAssertNil(source.draftPicture(for: .generate), "the other tab's choice is not sent")
        source.generatedKey = "avatar-gen-1.jpg"
        XCTAssertEqual(source.draftPicture(for: .generate), .generated(key: "avatar-gen-1.jpg"))
    }

    // MARK: - Upload fidelity (Jacob, 2026-10-26: a 4000 px mark came out jagged)

    /// A hard-edged black ring and bar on white, `side` px (what a vector export looks like).
    private func mark(side: Int) -> UIImage {
        let format = UIGraphicsImageRendererFormat()
        format.scale = 1
        format.opaque = true
        let s = CGFloat(side)
        return UIGraphicsImageRenderer(size: CGSize(width: s, height: s), format: format).image { ctx in
            let cg = ctx.cgContext
            cg.setShouldAntialias(false)
            UIColor.white.setFill()
            cg.fill(CGRect(x: 0, y: 0, width: s, height: s))
            UIColor.black.setStroke()
            cg.setLineWidth(s * 0.06)
            cg.strokeEllipse(in: CGRect(x: s * 0.07, y: s * 0.07, width: s * 0.86, height: s * 0.86))
            cg.move(to: CGPoint(x: s * 0.25, y: s * 0.25))
            cg.addLine(to: CGPoint(x: s * 0.75, y: s * 0.75))
            cg.strokePath()
        }
    }

    /// Grey values of `image` drawn at its own pixel size.
    private func gray(_ image: UIImage) -> (w: Int, h: Int, px: [UInt8]) {
        guard let cg = image.cgImage else { return (0, 0, []) }
        let (w, h) = (cg.width, cg.height)
        var px = [UInt8](repeating: 0, count: w * h)
        px.withUnsafeMutableBytes { buf in
            let ctx = CGContext(data: buf.baseAddress, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w,
                                space: CGColorSpaceCreateDeviceGray(), bitmapInfo: CGImageAlphaInfo.none.rawValue)!
            ctx.interpolationQuality = .none
            ctx.draw(cg, in: CGRect(x: 0, y: 0, width: w, height: h))
        }
        return (w, h, px)
    }

    /// Edge-contrast metric: of the neighbour pairs that cross an edge (differ by > 48), the share
    /// that are shaded (differ by < 224 — the edge passes through grey). ≈ 1 for a proper
    /// downscale, 0 for point sampling.
    private func edgeSoftness(_ image: UIImage) -> Double {
        let (w, h, px) = gray(image)
        var crossings = 0, shaded = 0
        func judge(_ a: UInt8, _ b: UInt8) {
            let d = abs(Int(a) - Int(b))
            if d > 48 {
                crossings += 1
                if d < 224 { shaded += 1 }
            }
        }
        for y in 0..<h {
            for x in 0..<w {
                if x + 1 < w { judge(px[y * w + x], px[y * w + x + 1]) }
                if y + 1 < h { judge(px[y * w + x], px[(y + 1) * w + x]) }
            }
        }
        return crossings == 0 ? 0 : Double(shaded) / Double(crossings)
    }

    /// A 2400 px two-colour mark → a 1024 px PNG master with shaded edges; `mime` / `ext` read
    /// the bytes; a smaller picture is not enlarged.
    func testALargeFlatMarkBecomesAShadedPngMaster() throws {
        let big = mark(side: 2400)
        let square = AvatarPickerSheet.square(big, side: PictureMaster.side)
        XCTAssertEqual(square.cgImage?.width, 1024)
        XCTAssertEqual(square.cgImage?.height, 1024)
        let soft = edgeSoftness(square)
        XCTAssertGreaterThan(soft, 0.8, "edges are shaded, not stair-stepped (\(soft))")

        let data = try XCTUnwrap(PictureMaster.encode(square))
        XCTAssertTrue(PictureMaster.isPNG(data), "a flat mark stays lossless")
        XCTAssertEqual(PictureMaster.mime(data), "image/png")
        XCTAssertEqual(PictureMaster.ext(data), "png")
        XCTAssertLessThanOrEqual(data.count, PictureMaster.maxBytes)
        let back = try XCTUnwrap(UIImage(data: data))
        XCTAssertEqual(back.cgImage?.width, 1024)

        let small = AvatarPickerSheet.square(mark(side: 300), side: PictureMaster.side)
        XCTAssertEqual(small.cgImage?.width, 300, "a small picture is never enlarged")

        let jpegHead = Data([0xFF, 0xD8, 0xFF, 0xE0])
        XCTAssertEqual(PictureMaster.mime(jpegHead), "image/jpeg")
        XCTAssertEqual(PictureMaster.ext(jpegHead), "jpg")
    }

    /// A photograph-like picture (gradients + noise) becomes a JPEG, never a multi-megabyte PNG.
    func testAPhotoBecomesAJpeg() throws {
        let format = UIGraphicsImageRendererFormat()
        format.scale = 1
        let s: CGFloat = 1024
        let photo = UIGraphicsImageRenderer(size: CGSize(width: s, height: s), format: format).image { ctx in
            let cg = ctx.cgContext
            var seed: UInt32 = 7
            let cell: CGFloat = 8
            var y: CGFloat = 0
            while y < s {
                var x: CGFloat = 0
                while x < s {
                    seed = seed &* 1_664_525 &+ 1_013_904_223
                    let noise = CGFloat(seed >> 24) / 255 * 0.3
                    cg.setFillColor(red: x / s, green: y / s, blue: min(1, (x + y) / (2 * s) + noise), alpha: 1)
                    cg.fill(CGRect(x: x, y: y, width: cell, height: cell))
                    x += cell
                }
                y += cell
            }
        }
        XCTAssertFalse(PictureMaster.isFlatGraphic(photo))
        let data = try XCTUnwrap(PictureMaster.encode(photo))
        XCTAssertFalse(PictureMaster.isPNG(data))
        XCTAssertEqual(PictureMaster.mime(data), "image/jpeg")
        XCTAssertLessThanOrEqual(data.count, PictureMaster.maxBytes)
    }

    /// Relay picture URLs are asked for the 384 px bucket (`?w=`), with the cache-busting `v`
    /// kept; GitHub's CDN is fetched as given.
    func testRelayPicturesAreFetchedAtTheDrawnSize() {
        let me = AvatarImageCache.sizedURL("https://relay.arbos.life/avatars/gh_1.jpg?v=1700", width: 384)
        XCTAssertEqual(me?.absoluteString, "https://relay.arbos.life/avatars/gh_1.jpg?v=1700&w=384")
        let agent = AvatarImageCache.sizedURL("https://relay.arbos.life/projects/01X/avatar.jpg?v=3", width: 384)
        XCTAssertEqual(agent?.absoluteString, "https://relay.arbos.life/projects/01X/avatar.jpg?v=3&w=384")
        let blob = AvatarImageCache.sizedURL("https://relay.arbos.life/blob/people:01C/group-1.png?token=t", width: 384)
        XCTAssertEqual(blob?.absoluteString, "https://relay.arbos.life/blob/people:01C/group-1.png?token=t&w=384")
        let github = AvatarImageCache.sizedURL("https://avatars.githubusercontent.com/u/1?v=4", width: 384)
        XCTAssertEqual(github?.absoluteString, "https://avatars.githubusercontent.com/u/1?v=4")
    }
}
