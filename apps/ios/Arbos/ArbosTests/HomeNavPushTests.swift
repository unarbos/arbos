import XCTest
@testable import Arbos

/// Home's account circle and "+" push Settings / New Message onto the stack rather than
/// raising bottom sheets (Jacob, 2026-11-10:
/// [feedback iphone-settings-nav-push-2026-11-10],
/// [feedback iphone-compose-nav-push-2026-11-10]).
///
/// The UI tests exercise the gesture; these cases pin the route helpers so a regression
/// that puts the sheet presentation back fails at compile/unit time too.
@MainActor
final class HomeNavPushTests: XCTestCase {
    func testShowOwnProfilePushesOwnProfileRoute() {
        let model = AppModel()
        model.showOwnProfile()
        guard case .ownProfile(let landing)? = model.path.last else {
            return XCTFail("expected .ownProfile on the path, got \(model.path)")
        }
        XCTAssertNil(landing)
        model.showOwnProfile(landing: "credits")
        // Already on own profile: a second tap must not stack a duplicate.
        XCTAssertEqual(model.path.count, 1)
        guard case .ownProfile? = model.path.last else {
            return XCTFail("still .ownProfile")
        }
    }

    func testShowOwnProfileCarriesLanding() {
        let model = AppModel()
        model.showOwnProfile(landing: "machine")
        guard case .ownProfile(let landing)? = model.path.last else {
            return XCTFail("expected .ownProfile")
        }
        XCTAssertEqual(landing, "machine")
    }

    func testShowNewMessagePushesNewMessageRoute() {
        let model = AppModel()
        model.showNewMessage()
        guard case .newMessage? = model.path.last else {
            return XCTFail("expected .newMessage on the path, got \(model.path)")
        }
        model.showNewMessage()
        XCTAssertEqual(model.path.count, 1, "a second + tap must not stack a duplicate")
    }

    func testDismissNewMessageClearsComposeAndAnythingAboveIt() {
        let model = AppModel()
        model.path = [
            .newMessage,
            .chat(chatId: "fx-dm", projectId: peopleSlot),
        ]
        model.dismissNewMessage()
        XCTAssertTrue(model.path.isEmpty, "compose and everything above it leave together")
    }

    /// HomeView must not present Settings or New Message as `.sheet` any more.
    func testHomeViewSourceDoesNotSheetSettingsOrNewMessage() throws {
        let root = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent() // ArbosTests
            .deletingLastPathComponent() // Arbos
            .appendingPathComponent("Arbos/UI/HomeView.swift")
        let source = try String(contentsOf: root, encoding: .utf8)
        XCTAssertFalse(source.contains(".sheet(isPresented: $showSettings"),
                       "Settings must push, not sheet")
        XCTAssertFalse(source.contains(".sheet(isPresented: $showNewChat"),
                       "New Message must push, not sheet")
        XCTAssertTrue(source.contains("showOwnProfile"), "avatar opens via showOwnProfile")
        XCTAssertTrue(source.contains("showNewMessage"), "+ opens via showNewMessage")
        XCTAssertTrue(source.contains(".backSwipeToPop()"),
                      "pushed panes keep the interactive edge swipe")
    }

    /// Back from a chat opened in Archived chats returns to that list, not to Home
    /// (tester report, 2026-11-17): the list is a route, and it stays under what opens.
    func testAChatOpenedFromArchivedKeepsTheListUnderIt() {
        let model = AppModel()
        model.showArchived()
        model.showArchived()
        XCTAssertEqual(model.path, [.archived], "a second tap must not stack a duplicate")
        let chat = AppModel.Route.chat(chatId: "fx-dm", projectId: peopleSlot)
        // Every `open(…)` replaces the path, as this does.
        model.openFromArchived { model.path = [chat] }
        XCTAssertEqual(model.path, [.archived, chat])
        model.path.removeLast()
        XCTAssertEqual(model.path, [.archived], "back lands on the archived list")
        let server = AppModel.Route.server(serverId: "s1")
        model.openFromArchived { model.path = [.archived, server] }
        XCTAssertEqual(model.path, [.archived, server], "the list is never put under itself")
    }
}
