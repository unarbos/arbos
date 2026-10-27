import XCTest
@testable import Arbos

/// The Secrets agent picker's pure half ([feedback secrets-agent-picker-ios-2026-10-26]):
/// the sheet filters through the shared matcher (`PeoplePicker.agents`), agents with access are not
/// offered, a pick adds a chip and × removes it, and every Secrets form draws the one
/// component — the Mac's `agent_picker::tests` / `picker_tests` twin.
final class AgentPickerTests: XCTestCase {
    private func agent(_ id: String, _ name: String, shared: Bool = false) -> Project {
        var p = Project(
            id: id, name: name, icon: "A", color: "#8b7cf6", state: .awake, stateSince: nil, stateDetail: nil,
            model: "", effort: .low, spendTodayUsd: 0, spendCapUsd: 0, avatar: nil, avatarColor: nil
        )
        p.shared = shared
        return p
    }

    private var registry: [Project] {
        [
            agent("a1", "Affine"), agent("a2", "Arbos"), agent("a3", "ArbosTest"), agent("a4", "Doug agent"),
            agent("a5", "Hezbola"), agent("a6", "Arbonites"), agent("a7", "Zoë"), agent("s1", "Shared in", shared: true),
        ]
    }

    func testCandidatesHideGrantedPickedAndSharedAgentsAndFollowTheQuery() {
        let all = AgentPickerMatch.candidates(registry, picked: [], exclude: [], query: "")
        XCTAssertEqual(all.map(\.name), ["Affine", "Arbos", "ArbosTest", "Doug agent", "Hezbola", "Arbonites", "Zoë"], "my agents only, registry order")
        let ar = AgentPickerMatch.candidates(registry, picked: ["a1", "a4"], exclude: ["a2"], query: "ar")
        XCTAssertEqual(ar.map(\.name), ["ArbosTest", "Arbonites"], "“ar”: Arbos has access, Affine / Doug are chips")
        XCTAssertTrue(AgentPickerMatch.candidates(registry, picked: [], exclude: [], query: "nothing here").isEmpty)
    }

    func testTheFilterIsTheSharedMatcher() {
        XCTAssertEqual(AgentPickerMatch.candidates(registry, picked: [], exclude: [], query: "@doug-ag").map(\.name), ["Doug agent"], "the @slug")
        XCTAssertEqual(AgentPickerMatch.candidates(registry, picked: [], exclude: [], query: "agent").map(\.name), ["Doug agent"], "a word start")
        XCTAssertEqual(AgentPickerMatch.candidates(registry, picked: [], exclude: [], query: "zoe").map(\.name), ["Zoë"], "diacritics folded")
        XCTAssertEqual(AgentPickerMatch.candidates(registry, picked: [], exclude: [], query: "arbos").map(\.name), ["Arbos", "ArbosTest"], "exact before prefix")
        XCTAssertTrue(AgentPickerMatch.candidates(registry, picked: [], exclude: [], query: "ffine").isEmpty, "no mid-word substrings — the matcher's rule, not ours")
    }

    func testAPickBecomesAChipAndLeavesTheListAndTheCrossRemovesIt() {
        var picked: Set<Id> = []
        // Tap "ArbosTest" in the sheet.
        picked.insert("a3")
        XCTAssertEqual(AgentPickerMatch.chips(registry, picked: picked).map(\.name), ["ArbosTest"])
        XCTAssertFalse(AgentPickerMatch.candidates(registry, picked: picked, exclude: [], query: "arbos").map(\.id).contains("a3"), "a chip is not offered again")
        XCTAssertEqual(AgentPickerMatch.candidates(registry, picked: picked, exclude: [], query: "arbos").map(\.name), ["Arbos"])
        // Its ×.
        picked.remove("a3")
        XCTAssertTrue(AgentPickerMatch.chips(registry, picked: picked).isEmpty)
        XCTAssertTrue(AgentPickerMatch.candidates(registry, picked: picked, exclude: [], query: "arbos").map(\.id).contains("a3"), "back in the list")
    }

    func testChipsFollowRegistryOrderNotPickOrder() {
        let picked: Set<Id> = ["a5", "a1", "a3"]
        XCTAssertEqual(AgentPickerMatch.chips(registry, picked: picked).map(\.name), ["Affine", "ArbosTest", "Hezbola"])
    }

    /// Every Secrets picker is the component: New secret, Connect 1Password, Give access
    /// (secret) and Give access (vault) each draw `AgentPickerField`; the toggle-chip wall
    /// (`AgentChips`) is gone. A source check, as `ControlTokenLintTests` does for sizes.
    func testEverySecretsFormUsesTheSharedPicker() throws {
        let file = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("Arbos/UI/SecretsViews.swift")
        guard FileManager.default.fileExists(atPath: file.path) else {
            throw XCTSkip("sources not beside the test bundle (CI without the tree)")
        }
        let text = try String(contentsOf: file, encoding: .utf8)
        XCTAssertFalse(text.contains("AgentChips"), "the toggle-chip wall is gone")
        let uses = text.components(separatedBy: "AgentPickerField(picked: $picked)").count - 1
        XCTAssertEqual(uses, 4, "New secret · Connect 1Password · Give access (secret) · Give access (vault)")
        for sheet in ["struct NewSecretSheet", "struct ConnectVaultSheet", "struct GiveAccessSheet", "struct GiveVaultAccessSheet"] {
            let after = text.components(separatedBy: sheet).dropFirst().joined()
            let body = after.components(separatedBy: "\nstruct ").first ?? after
            XCTAssertTrue(body.contains("AgentPickerField(picked: $picked)"), "\(sheet) draws the shared picker")
        }
    }
}
