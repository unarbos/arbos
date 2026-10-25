import XCTest
@testable import Arbos

/// #1544 (feedback `shell-approval-grants-2026-10-23`): a destructive-shell approval part with
/// `grant_options` becomes a four-row card — A Allow once · B Always allow <agent> in <class> ·
/// C Always allow <agent> anywhere · D Deny — and an answered card knows which row was picked
/// from the part's `grant`. Cards without options keep Approve / Reject.
final class ShellApprovalCardTests: XCTestCase {
    private func part(state: String, grant: String? = nil, options: [[String: Any]]) -> [String: Any] {
        var pm: [String: Any] = [
            "id": "p1", "kind": "approval", "approvalId": "ap1", "state": state,
            "title": "Run a destructive command (file-delete)", "risk": "destructive",
            "payload": ["kind": "tool_call", "tool": "bash", "class": "file-delete", "path_class": "workspace",
                        "input": ["command": "rm -rf build"], "grant_options": options] as [String: Any],
        ]
        if let grant { pm["grant"] = grant }
        return pm
    }

    private let twoOptions: [[String: Any]] = [
        ["path_class": "workspace", "label": "its workspace", "detail": "the agent's own /work folder, including scratch"],
        ["path_class": "anywhere", "label": "anywhere", "detail": "any folder of its box"],
    ]

    func testShellCardOffersFourRowsAndNamesTheAgent() throws {
        guard case .question(_, let q) = DocJSON.part(part(state: "pending", options: twoOptions)) else {
            return XCTFail("approval parts render as question cards")
        }
        XCTAssertTrue(q.shellCard)
        XCTAssertTrue(q.open)
        XCTAssertEqual(q.options.map(\.id), ["once", "grant:workspace", "grant:anywhere", "deny"])
        XCTAssertEqual(q.options[1].label, "Always allow {agent} in its workspace")
        XCTAssertEqual(q.options[1].hint, "the agent's own /work folder, including scratch")
        XCTAssertEqual(q.options[2].label, "Always allow {agent} anywhere")
        XCTAssertTrue(q.options[2].hint?.contains("no more cards for file deletes from {agent}") == true)
        XCTAssertTrue(q.options[3].destructive)
        XCTAssertEqual(q.answer, [])
    }

    func testAnsweredShellCardKnowsItsRow() throws {
        guard case .question(_, let granted) = DocJSON.part(part(state: "approved", grant: "workspace", options: twoOptions)),
              case .question(_, let once) = DocJSON.part(part(state: "approved", options: twoOptions)),
              case .question(_, let denied) = DocJSON.part(part(state: "rejected", options: twoOptions)) else {
            return XCTFail("approval parts render as question cards")
        }
        XCTAssertEqual(granted.answer, ["grant:workspace"])
        XCTAssertFalse(granted.open)
        XCTAssertEqual(once.answer, ["once"])
        XCTAssertEqual(denied.answer, ["deny"])
    }

    func testCardsWithoutOptionsKeepApproveReject() throws {
        guard case .question(_, let q) = DocJSON.part(part(state: "pending", options: [])) else {
            return XCTFail("approval parts render as question cards")
        }
        XCTAssertFalse(q.shellCard)
        XCTAssertEqual(q.options.map(\.id), ["approve", "reject"])
    }

    func testSummaryLine() {
        XCTAssertEqual(ShellGrant.summary([]), "Asks every time")
        let g = ShellGrant(id: "g", ruleClass: "file-delete", pathClass: "scratch", chatId: nil, grantedBy: "u",
                           createdAt: 0, label: "File deletes in /tmp", detail: "")
        XCTAssertEqual(ShellGrant.summary([g]), "Destructive · /tmp")
    }
}
