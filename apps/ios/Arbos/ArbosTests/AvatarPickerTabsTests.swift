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
}
