import XCTest
@testable import Arbos

/// Secret hand-off (feedback `secret-handoff-ios-2026-10-26`, Jacob 2026-10-26): the request
/// card (`payload.kind = secret_offer`) decodes without answer rows and without any value
/// field; a `secret_redacted` part becomes the ghost row "Secret saved as NAME" (mine on the
/// right, another member's on the left) and leaves no text to copy or search; a secret given
/// in chat says so in Settings.
final class SecretHandoffTests: XCTestCase {
    private func offerPart(state: String) -> [String: Any] {
        var pm: [String: Any] = [
            "id": "p1", "kind": "approval", "approvalId": "ap-offer", "state": state,
            "title": "Give Comms a secret", "risk": "credential", "summary": "the refunds report",
            "payload": ["kind": "secret_offer", "name": "STRIPE_KEY", "purpose": "the refunds report",
                        "agent": "Comms", "projectId": "fx-comms", "chatId": "c1", "answers": []] as [String: Any],
        ]
        if state == "approved" { pm["decidedBy"] = "user:fx-me" }
        return pm
    }

    private func message(_ id: String, role: Role, sender: String, parts: [Part], handle: String? = nil, name: String? = nil) -> Message {
        Message(id: id, chatId: "c1", role: role, sender: sender, status: .done, turn: nil, decision: nil, subagentId: nil,
                subagentName: nil, inReplyTo: nil, usage: [:], model: nil, createdAt: 1_700_000_000_000, completedAt: nil,
                pending: false, parts: parts, hiddenCount: 0, senderHandle: handle, senderName: name)
    }

    func testRequestCardDecodesWithoutRowsOrAValueField() throws {
        guard case .question(_, let q) = DocJSON.part(offerPart(state: "pending")) else {
            return XCTFail("approval parts render as question cards")
        }
        let offer = try XCTUnwrap(q.secretOffer)
        XCTAssertEqual(offer.name, "STRIPE_KEY")
        XCTAssertEqual(offer.purpose, "the refunds report")
        XCTAssertEqual(offer.agent, "Comms")
        XCTAssertEqual(offer.projectId, "fx-comms")
        XCTAssertTrue(q.options.isEmpty, "the card draws its own fields, not Approve / Reject")
        XCTAssertTrue(q.open)
        XCTAssertEqual(q.question, "Give Comms a secret", "no question mark forced onto a request title")
        XCTAssertFalse(String(describing: q).contains("value"), "nothing in the model is a value field")

        guard case .question(_, let saved) = DocJSON.part(offerPart(state: "approved")) else {
            return XCTFail("approval parts render as question cards")
        }
        XCTAssertFalse(saved.open)
        XCTAssertEqual(saved.answer, ["saved"])
    }

    func testRedactedPartIsTheGhostRowAndHasNoText() throws {
        let pm: [String: Any] = ["id": "r1", "kind": "secret_redacted", "name": "STRIPE_KEY", "env": "STRIPE_KEY",
                                 "secretId": "s1", "savedBy": "user:fx-me", "agent": "fx-comms"]
        let part = DocJSON.part(pm)
        guard case .secretRedacted(_, let name, let savedBy, let agent) = part else {
            return XCTFail("secret_redacted decodes to its own part, got \(part)")
        }
        XCTAssertEqual(name, "STRIPE_KEY")
        XCTAssertEqual(savedBy, "user:fx-me")
        XCTAssertEqual(agent, "fx-comms")
        XCTAssertEqual(part.secretRedactedName, "STRIPE_KEY")

        let mine = message("m1", role: .user, sender: "user", parts: [part])
        XCTAssertEqual(mine.plainText, "", "nothing to copy or search")
        // (the first row is the date stamp; the message's own row follows it)
        let rows = Transcript.rows([mine], projectId: "fx-comms")
        let ghost = try XCTUnwrap(rows.last)
        guard case .secretGhost(let n, let isMine, _, _, _) = ghost.kind else {
            return XCTFail("a redacted user row is the ghost, got \(ghost.kind)")
        }
        XCTAssertEqual(n, "STRIPE_KEY")
        XCTAssertTrue(isMine)
        XCTAssertEqual(ghost.role, .user)
        XCTAssertEqual(rows.filter { if case .user = $0.kind { return true }; return false }.count, 0, "no text bubble beside the ghost")

        // another member's row in a people chat: the ghost on their side, under their name
        let theirs = message("m2", role: .user, sender: "user:fx-u4", parts: [part], handle: "sam", name: "Sam Okafor")
        let groupRows = Transcript.rows([theirs], projectId: "people:g1", me: "user:fx-me", people: true, multiParty: true)
        let theirGhost = try XCTUnwrap(groupRows.last)
        guard case .secretGhost(_, let theirMine, let handle, _, let named) = theirGhost.kind else {
            return XCTFail("the other member's redacted row is the ghost, got \(theirGhost.kind)")
        }
        XCTAssertFalse(theirMine)
        XCTAssertEqual(handle, "sam")
        XCTAssertTrue(named)
        XCTAssertEqual(theirGhost.role, .project)
    }

    func testAnUnknownPartStillDecodesToUnknown() {
        guard case .unknown = DocJSON.part(["id": "x", "kind": "something_new"]) else {
            return XCTFail("unknown kinds stay additive")
        }
    }

    func testASecretGivenInChatSaysSoInSettings() throws {
        let given = try XCTUnwrap(SecretItem.parse([
            "id": "s1", "name": "STRIPE_KEY", "env": "STRIPE_KEY", "kind": "key", "note": "the refunds report",
            "hosts": [], "ask_every_time": false, "inject_env": false, "version": 1, "created_at": 1, "updated_at": 1,
            "vault_name": "Arbos", "ref": "arbos-secret://STRIPE_KEY/value", "agents": [], "directors": [], "people": [],
            "source": ["kind": "chat", "project_id": "fx-comms", "project_name": "Comms", "chat_id": "c1"],
        ] as [String: Any]))
        XCTAssertEqual(given.sourceLabel, "given in chat")
        XCTAssertEqual(given.addedLabel, "In chat with Comms")

        let settings = try XCTUnwrap(SecretItem.parse([
            "id": "s2", "name": "MAILGUN_API_KEY", "env": "MAILGUN_API_KEY", "kind": "key", "note": "",
            "hosts": [], "ask_every_time": false, "inject_env": false, "version": 1, "created_at": 1, "updated_at": 1,
            "vault_name": "Arbos", "ref": "arbos-secret://MAILGUN_API_KEY/value", "agents": [], "directors": [],
        ] as [String: Any]))
        XCTAssertNil(settings.sourceLabel, "an older relay sends no source: settings")
        XCTAssertNil(settings.addedLabel)
    }
}
