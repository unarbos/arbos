import XCTest
@testable import Arbos

/// Profile targets toggle the sheet ([feedback profile-pane-toggle-ios-2026-10-26];
/// the Mac's `profile_target::tests` twin): a sender name, the avatar beside a
/// bubble, an @mention chip, the header pill and a status-row picture all go
/// through `AppModel.toggleProfile` — open, dismiss on the same subject from any
/// target, swap for someone else — and carry one label.
final class ProfileToggleTests: XCTestCase {
    private let allanFromChat = ProfileTarget.user(ProfilePerson(id: "gh_2", handle: "allan", name: "Allan", avatarUrl: nil))
    /// A chip's person: the same account carried with the handle as its id.
    private let allanFromChip = ProfileTarget.user(ProfilePerson(id: "allan", handle: "Allan", name: nil, avatarUrl: nil))
    private let sami = ProfileTarget.user(ProfilePerson(id: "gh_3", handle: "sami", name: "Sami", avatarUrl: nil))
    private let affine = ProfileTarget.agent(projectId: "p-affine")

    func testNothingUpOpensTheTarget() {
        XCTAssertEqual(ProfileToggle.next(showing: nil, tapped: allanFromChat), allanFromChat)
        XCTAssertEqual(ProfileToggle.next(showing: nil, tapped: affine), affine)
    }

    func testTheSameSubjectDismissesFromAnyTarget() {
        XCTAssertNil(ProfileToggle.next(showing: allanFromChat, tapped: allanFromChat), "the same target again")
        XCTAssertNil(ProfileToggle.next(showing: allanFromChat, tapped: allanFromChip), "his chip while his page is up")
        XCTAssertNil(ProfileToggle.next(showing: allanFromChip, tapped: allanFromChat), "his name while the chip's page is up")
        XCTAssertNil(ProfileToggle.next(showing: affine, tapped: .agent(projectId: "p-affine")), "the agent again")
    }

    func testADifferentPersonSwapsInPlace() {
        XCTAssertEqual(ProfileToggle.next(showing: allanFromChat, tapped: sami), sami)
        XCTAssertEqual(ProfileToggle.next(showing: allanFromChat, tapped: affine), affine)
        XCTAssertEqual(ProfileToggle.next(showing: affine, tapped: allanFromChip), allanFromChip)
        XCTAssertEqual(ProfileToggle.next(showing: affine, tapped: .agent(projectId: "p-other")), .agent(projectId: "p-other"))
    }

    func testSameSubjectIsByPersonNotByPage() {
        XCTAssertTrue(allanFromChat.sameSubject(as: allanFromChip))
        XCTAssertFalse(allanFromChat.sameSubject(as: sami))
        XCTAssertFalse(allanFromChat.sameSubject(as: affine))
        XCTAssertTrue(ProfileTarget.group(chatId: "g1", projectId: nil).sameSubject(as: .group(chatId: "g1", projectId: "p1")), "a group is its chat")
        XCTAssertFalse(ProfileTarget.channel(channelId: "c1").sameSubject(as: .server(serverId: "c1")))
    }

    func testTheLabelSaysWhatTheTapDoes() {
        XCTAssertEqual(ProfileToggle.label(name: "Allan", showing: false), "Open Allan's profile")
        XCTAssertEqual(ProfileToggle.label(name: " Affine ", showing: false), "Open Affine's profile")
        XCTAssertEqual(ProfileToggle.label(name: "Allan", showing: true), "Close profile")
    }

    /// The model: open, the same person from another target dismisses (path and
    /// root cleared), a different one swaps with nothing pushed.
    @MainActor
    func testTheModelTogglesAndSwaps() {
        let model = AppModel()
        XCTAssertNil(model.profile)
        model.toggleProfile(allanFromChat)
        XCTAssertEqual(model.profile, allanFromChat)
        XCTAssertTrue(model.profileShows(allanFromChip), "the chip's page is his too")
        model.toggleProfile(allanFromChip)
        XCTAssertNil(model.profile, "the same person again dismisses")
        XCTAssertEqual(model.profilePath, [])
        model.toggleProfile(allanFromChat)
        model.toggleProfile(affine)
        XCTAssertEqual(model.profile, affine, "swapped in place")
        XCTAssertEqual(model.profilePath, [], "nothing pushed")
        XCTAssertFalse(model.profileShows(allanFromChat))
        // A page pushed over the root (a member row): the top page is what a tap toggles against.
        model.showProfile(.group(chatId: "g1", projectId: nil))
        model.showProfile(allanFromChat)
        XCTAssertEqual(model.profilePath, [allanFromChat], "a group's row still pushes")
        XCTAssertTrue(model.profileShows(allanFromChat))
        model.toggleProfile(allanFromChip)
        XCTAssertNil(model.profile, "his target while his pushed page is up dismisses the sheet")
        XCTAssertEqual(model.profilePath, [])
    }

    // MARK: - Every chat target goes through the one action

    private var sourceRoot: URL {
        // …/apps/ios/Arbos/ArbosTests/ProfileToggleTests.swift → …/apps/ios/Arbos/Arbos
        URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().appendingPathComponent("Arbos")
    }

    private func source(_ rel: String) throws -> String? {
        let url = sourceRoot.appendingPathComponent(rel)
        guard FileManager.default.fileExists(atPath: url.path) else { return nil }
        return try String(contentsOf: url, encoding: .utf8)
    }

    /// The transcript's sender avatar and name, the presence row's pictures and the
    /// chip route (`ChatView`'s `openURL`) call `toggleProfile`, never `showProfile`;
    /// the header pill's action is `toggleProfile(headerTarget)`.
    func testEveryChatTargetRoutesThroughToggleProfile() throws {
        guard let transcript = try source("UI/Transcript.swift"),
              let chat = try source("UI/ChatView.swift"),
              let presence = try source("UI/PresenceRow.swift") else {
            throw XCTSkip("sources not beside the test bundle (CI without the tree)")
        }
        XCTAssertFalse(transcript.contains("showProfile("), "Transcript.swift opens profiles only through toggleProfile")
        XCTAssertTrue(transcript.contains("model.toggleProfile(personTarget)"), "the person's avatar and name toggle")
        XCTAssertTrue(transcript.contains("model.toggleProfile(memberTarget)"), "the agent's disc and name toggle")
        XCTAssertTrue(transcript.contains("ProfileToggle.label(name: personName"), "the person's targets carry the shared label")
        XCTAssertTrue(transcript.contains("ProfileToggle.label(name: agent.name"), "the agent's targets carry the shared label")
        XCTAssertTrue(chat.contains("model.toggleProfile(headerTarget)"), "the header pill toggles")
        XCTAssertTrue(chat.contains(".accessibilityLabel(headerLabel)"), "the header pill carries the shared label")
        let chipRoute = chat.range(of: "MentionTable.target(from: url, model: model)")
        XCTAssertNotNil(chipRoute, "the chip route is still in ChatView")
        if let chipRoute {
            let after = chat[chipRoute.upperBound...].prefix(400)
            XCTAssertTrue(after.contains("model.toggleProfile(target)"), "a chip tap toggles")
            XCTAssertFalse(after.contains("model.showProfile(target)"), "and never plain-opens")
        }
        XCTAssertTrue(chat.contains("onPicture: { model.toggleProfile($0) }"), "a status-row picture toggles")
        XCTAssertTrue(presence.contains("Button { onPicture(target) }"), "the row's pictures are buttons when the chat names them")
        XCTAssertTrue(presence.contains("ProfileToggle.label(name: p.name"), "with the shared label")
        XCTAssertFalse(presence.contains("showProfile("), "PresenceRow never opens a profile by itself")
    }
}
