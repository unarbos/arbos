import SwiftUI
import XCTest
@testable import Arbos

/// The presence row over the composer (typing-indicator-left-2026-10-23): one
/// line of text whatever the count, and one typing mark per handle per chat.
final class PresenceRowTests: XCTestCase {
    func testOneLineWhateverTheCount() {
        XCTAssertEqual(PresenceLine.sentence(typing: ["Sam"], working: []), "Sam is typing…")
        XCTAssertEqual(PresenceLine.sentence(typing: ["Sam", "Mira"], working: []), "Sam and Mira are typing…")
        XCTAssertEqual(PresenceLine.sentence(typing: ["Sam", "Mira", "Kai", "Ava"], working: []), "Sam, Mira and 2 others are typing…")
        XCTAssertEqual(PresenceLine.sentence(typing: [], working: ["Lium"]), "Lium is working")
        XCTAssertEqual(PresenceLine.sentence(typing: [], working: ["Lium", "Home"]), "Lium and Home are working")
        XCTAssertEqual(PresenceLine.sentence(typing: [], working: ["Lium", "Home", "Doug"]), "Lium, Home and Doug are working")
        XCTAssertEqual(PresenceLine.sentence(typing: ["Sam"], working: ["Lium"]), "Sam is typing… · Lium is working")
        XCTAssertEqual(PresenceLine.sentence(typing: [], working: []), "")
    }

    func testDetailAndQueueApplyToASingleWorkerOnly() {
        XCTAssertEqual(PresenceLine.sentence(typing: [], working: ["Lium"], detail: "Cloning RaoFoundation/subtensor"), "Cloning RaoFoundation/subtensor")
        XCTAssertEqual(PresenceLine.sentence(typing: [], working: ["Lium", "Home"], detail: "Cloning"), "Lium and Home are working")
        XCTAssertEqual(PresenceLine.sentence(typing: [], working: ["Lium"], queued: true), "Lium is waiting for the machine")
    }

    @MainActor
    func testTypingMarksAreOnePerHandleNewestFirstAndExpire() {
        let model = AppModel()
        model.noteTyping(["handle": "sam", "typing": true], chatId: "c1")
        model.noteTyping(["handle": "mira", "typing": true], chatId: "c1")
        model.noteTyping(["handle": "sam", "typing": true], chatId: "c1")   // refresh, not a second entry
        XCTAssertEqual(Set(model.typingHandles(in: "c1")), ["sam", "mira"])
        XCTAssertEqual(model.typingHandles(in: "c1").count, 2, "one mark per handle")
        XCTAssertEqual(model.typingHandles(in: "c2"), [], "per chat")
        model.noteTyping(["handle": "mira", "typing": false], chatId: "c1")
        XCTAssertEqual(model.typingHandles(in: "c1"), ["sam"])
        model.typing["c1"] = ["sam": nowMs() - 1]
        XCTAssertEqual(model.typingHandles(in: "c1"), [], "a stale mark is gone")
    }

    func testFirstNamesFromTwoParticipantsOn() {
        let pic = AnyView(EmptyView())
        let one = PresenceRow(participants: [PresenceParticipant(id: "a", name: "Sam Okafor", activity: .typing, picture: pic)])
        XCTAssertEqual(one.text, "Sam Okafor is typing…")
        let two = PresenceRow(participants: [
            PresenceParticipant(id: "a", name: "Mira Kaplan", activity: .typing, picture: pic),
            PresenceParticipant(id: "b", name: "Sam Okafor", activity: .typing, picture: pic),
        ])
        XCTAssertEqual(two.text, "Mira and Sam are typing…")
        XCTAssertEqual(PresenceLine.firstName("@sam"), "@sam")
    }

    func testPictureIsTheBubblePictureOnThePlusColumn() {
        // Same rule as Mac (`SENDER_DISC = PLUS_BUTTON`): the row's picture and the
        // bubble's picture are the `+` size at the `+` centre; the row's gap is the
        // bubble row's gap, so the text starts where the body does.
        XCTAssertEqual(Theme.memberAvatar, Theme.plusButton, "avatar disc = + disc")
        XCTAssertEqual(PresenceRow.avatar, Theme.memberAvatar)
        XCTAssertEqual(PresenceRow.gap, Theme.memberAvatarGap)
        // The column follows the `+` through both composer insets (keyboard down / up).
        for composerInset in [Theme.composerInset, Theme.composerInsetKeyboard] {
            let center = 16 + Theme.avatarColumnInset(composerInset: composerInset) + Theme.memberAvatar / 2
            let plusCenter = composerInset + Theme.plusButton / 2
            XCTAssertEqual(center, plusCenter, accuracy: 1, "inset \(composerInset)")
            XCTAssertEqual(16 + Theme.memberLane(composerInset: composerInset),
                           16 + Theme.avatarColumnInset(composerInset: composerInset) + Theme.memberAvatar + Theme.memberAvatarGap)
        }
        // Keyboard up: avatar spans the `+` (19–61), body after the gap at 74.
        XCTAssertEqual(16 + Theme.avatarColumnInset(composerInset: Theme.composerInsetKeyboard), 19)
        XCTAssertEqual(16 + Theme.memberLane(composerInset: Theme.composerInsetKeyboard), 74)
        XCTAssertEqual(PresenceRow.maxPictures, 3)
    }
}
