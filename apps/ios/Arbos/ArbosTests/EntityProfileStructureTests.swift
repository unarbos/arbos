import XCTest
@testable import Arbos

/// Snapshot / structure guards for the Grok overview · Edit hierarchy
/// ([feedback iphone-entity-profile-edit-2026-11-10],
/// [feedback iphone-create-entity-edit-style-2026-11-10]).
final class EntityProfileStructureTests: XCTestCase {
    func testOverviewKindsMatchGrokRowSets() {
        XCTAssertEqual(PaneKinds.agent, ["Coding", "Routines", "Library", "Computer"])
        XCTAssertEqual(PaneKinds.outsideAgent, ["Library", "Connection"])
        XCTAssertEqual(PaneKinds.group, ["Members", "Library"])
        XCTAssertEqual(PaneKinds.server, ["Channels", "Members", "Settings"])
        XCTAssertEqual(ProfilePane.layout(tabs: 0, footerSections: 0), [.hero, .options])
    }

    func testCreateKindTitlesAndActions() {
        XCTAssertEqual(CreateKind.agent.title, "Create New Agent")
        XCTAssertEqual(CreateKind.group.title, "Create New Group")
        XCTAssertEqual(CreateKind.server.title, "Create New Server")
        XCTAssertEqual(CreateKind.connect.title, "Connect an Agent")
        XCTAssertEqual(CreateKind.agent.action, "Create")
        XCTAssertEqual(CreateKind.group.action, "Next")
        XCTAssertEqual(CreateEntityView.options, [.upload, .generate])
    }

    func testEditChromePiecesExist() {
        // Compile-time presence of the shared kit used by every edit / create page.
        XCTAssertEqual(PaneKit.heroAvatar, 112)
        XCTAssertEqual(PaneKit.radius, 16)
        XCTAssertEqual(PaneKit.rowHeight, 48)
    }
}
