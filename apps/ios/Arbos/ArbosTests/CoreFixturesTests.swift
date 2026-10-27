import XCTest
import ArbosFFI
@testable import Arbos

/// Core golden fixtures under `crates/arbos-core/fixtures` (host path via `#filePath`).
final class CoreFixturesTests: XCTestCase {
    private static let repoRoot: URL = {
        // apps/ios/Arbos/ArbosTests/ThisFile.swift → repo root
        URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
    }()

    private static let fixturesDir: URL = {
        repoRoot.appendingPathComponent("crates/arbos-core/fixtures")
    }()

    private static let apiFixturesDir: URL = {
        repoRoot.appendingPathComponent("crates/arbos-relay/tests/fixtures/api")
    }()

    private func fixtureData(_ name: String) throws -> Data {
        let url = Self.fixturesDir.appendingPathComponent(name)
        return try Data(contentsOf: url)
    }

    private func apiFixtureData(_ name: String) throws -> Data {
        let url = Self.apiFixturesDir.appendingPathComponent(name)
        return try Data(contentsOf: url)
    }

    private func apiFixtureJSON(_ name: String) throws -> Any {
        try JSONSerialization.jsonObject(with: apiFixtureData(name))
    }

    private func expectedRows(_ name: String = "chat_doc_expected.json") throws -> [[String: Any]] {
        let data = try fixtureData(name)
        let json = try JSONSerialization.jsonObject(with: data)
        guard let rows = json as? [[String: Any]] else {
            XCTFail("expected array"); return []
        }
        return rows
    }

    private func assertCoreChatDocRows(_ messages: [Message], expected: [[String: Any]], file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertEqual(messages.count, expected.count, file: file, line: line)
        for (i, exp) in expected.enumerated() {
            let msg = messages[i]
            XCTAssertEqual(msg.id, exp["id"] as? String, "row \(i) id", file: file, line: line)
            XCTAssertEqual(msg.role.rawValue, exp["role"] as? String, "row \(i) role", file: file, line: line)
            XCTAssertEqual(msg.plainText, exp["text"] as? String ?? "", "row \(i) text", file: file, line: line)
            XCTAssertEqual(msg.hiddenCount, exp["hiddenCount"] as? Int ?? 0, "row \(i) hiddenCount", file: file, line: line)
            if let greeting = exp["greeting"] as? Bool {
                XCTAssertEqual(msg.greeting, greeting, "row \(i) greeting", file: file, line: line)
            }
            if let qid = exp["answersQuestionId"] as? String {
                // answersQuestionId is not a Message field; presence is enough via id/text.
                XCTAssertFalse(qid.isEmpty, file: file, line: line)
            }
            if let reactions = exp["reactions"] as? [String: Any], !reactions.isEmpty {
                XCTAssertFalse(msg.reactions.isEmpty, "row \(i) reactions", file: file, line: line)
                for (emoji, people) in reactions {
                    guard let people = people as? [String: Any] else { continue }
                    let got = msg.reactions.first { $0.emoji == emoji }
                    XCTAssertNotNil(got, "row \(i) missing emoji \(emoji)", file: file, line: line)
                    for (sender, flag) in people {
                        guard (flag as? Bool) == true else { continue }
                        XCTAssertTrue(got?.senders.contains(sender) == true, "row \(i) \(emoji)/\(sender)", file: file, line: line)
                    }
                }
            }
        }
    }

    /// CoreChatDoc path: import the snapshot and compare shape.
    func testCoreChatDocReadsFixture() throws {
        let snap = try fixtureData("chat_doc.bin")
        let doc = CoreChatDoc(key: "chat-fixture")
        try doc.importUpdate(snap)
        let messages = doc.readAll()
        let expected = try expectedRows()
        assertCoreChatDocRows(messages, expected: expected)
        XCTAssertEqual(messages[1].hiddenCount, 1, "assistant fixture must expose one hidden tool part")
    }

    /// Older shapes (Swift placeholders, turn string/object, flat attachment, plain text,
    /// status+question, no reactions map) still decode through CoreChatDoc.
    func testCoreChatDocReadsLegacyFixture() throws {
        let snap = try fixtureData("chat_doc_legacy.bin")
        let doc = CoreChatDoc(key: "chat-legacy")
        try doc.importUpdate(snap)
        let messages = doc.readAll()
        let expected = try expectedRows("chat_doc_legacy_expected.json")
        assertCoreChatDocRows(messages, expected: expected)
        XCTAssertEqual(messages.count, 7)
        XCTAssertEqual(messages[0].id, "msg-ios-user")
        XCTAssertEqual(messages[0].plainText, "hello from ios")
        XCTAssertEqual(messages[0].replyTo, "m0")
        XCTAssertEqual(messages[5].plainText, "plain string text body")
        XCTAssertTrue(messages[6].parts.contains { if case .question(_, _) = $0 { return true }; return false })
        XCTAssertTrue(messages[6].parts.contains { if case .status(_, let code, _) = $0 { return code == .askUser }; return false })
    }

    /// arbos-ffi path: same snapshot through UniFFI ChatDoc.readRowsJson.
    func testFfiReadsCoreChatDocFixture() throws {
        let snap = try fixtureData("chat_doc.bin")
        let doc = try ArbosFFI.ChatDoc.openFromSnapshot(snapshot: snap)
        let raw = doc.readRowsJson()
        let json = try JSONSerialization.jsonObject(with: Data(raw.utf8))
        guard let rows = json as? [[String: Any]] else {
            return XCTFail("FFI rows not an array")
        }
        let expected = try expectedRows()
        XCTAssertEqual(rows.count, expected.count)

        for (i, exp) in expected.enumerated() {
            let row = rows[i]
            XCTAssertEqual(row["id"] as? String, exp["id"] as? String, "row \(i) id")
            XCTAssertEqual(row["role"] as? String, exp["role"] as? String, "row \(i) role")
            let text = Self.messageText(row)
            XCTAssertEqual(text, exp["text"] as? String ?? "", "row \(i) text")
            let hidden = (row["hiddenParts"] as? [Any])?.count ?? 0
            XCTAssertEqual(hidden, exp["hiddenCount"] as? Int ?? 0, "row \(i) hiddenCount")
            if let reactions = exp["reactions"] as? [String: Any] {
                let got = row["reactions"] as? [String: Any] ?? [:]
                XCTAssertEqual(NSDictionary(dictionary: got), NSDictionary(dictionary: reactions), "row \(i) reactions")
            }
        }
        let assistantHidden = (rows[1]["hiddenParts"] as? [Any])?.count ?? 0
        XCTAssertEqual(assistantHidden, 1)
    }

    func testFfiDecodesFrameFixtures() throws {
        let cases: [(String, ArbosFFI.FrameType)] = [
            ("frame_hello.bin", .hello),
            ("frame_state.bin", .state),
            ("frame_row.bin", .row),
            ("frame_rows_done.bin", .rowsDone),
            ("frame_push.bin", .push),
            ("frame_ack.bin", .ack),
        ]
        for (name, ffiTy) in cases {
            let bytes = try fixtureData(name)
            let decoded = try ArbosFFI.decodeFrame(bytes: bytes)
            XCTAssertEqual(decoded.ty, ffiTy, name)
        }
    }

    private static func messageText(_ row: [String: Any]) -> String {
        let parts = row["parts"] as? [[String: Any]] ?? []
        return parts
            .filter { ($0["kind"] as? String) == "text" }
            .compactMap { $0["text"] as? String }
            .joined()
    }

    // MARK: Relay API DTO fixtures

    func testProjectDtoFixture() throws {
        let data = try apiFixtureData("project_json.json")
        let dto = try JSONDecoder().decode(ProjectDto.self, from: data)
        let project = Arbos.Project(dto: dto)
        XCTAssertEqual(project.id, "proj_fixture")
        XCTAssertEqual(project.name, "Fixture")
        XCTAssertEqual(project.state, .hibernatedLight)
        XCTAssertEqual(project.ownerHandle, "user101")
        XCTAssertEqual(project.primaryChatId, "chat_primary")
    }

    func testParseChatFixture() throws {
        let raw = try apiFixtureJSON("chat_json_for.json")
        let chat = try XCTUnwrap(RelayAPI.parseChat(raw))
        XCTAssertEqual(chat.id, "chat_fixture")
        XCTAssertEqual(chat.projectId, "proj_fixture")
        XCTAssertEqual(chat.title, "Hello")
    }

    func testPeopleChatParseFixtures() throws {
        let withMe = try XCTUnwrap(PeopleChat.parse(try apiFixtureJSON("people_chat_json_with_me.json")))
        XCTAssertEqual(withMe.members.count, 2)
        XCTAssertEqual(withMe.title, "user102")
        XCTAssertEqual(withMe.myRole, "owner")
        XCTAssertEqual(withMe.muted, false)

        let withoutMe = try XCTUnwrap(PeopleChat.parse(try apiFixtureJSON("people_chat_json_without_me.json")))
        XCTAssertEqual(withoutMe.members.count, 2)
        XCTAssertEqual(withoutMe.title, "user101, user102")
        XCTAssertEqual(withoutMe.myRole, "member")
        XCTAssertEqual(withoutMe.muted, false)
    }

    func testChatMemberParseFixtures() throws {
        let human = try XCTUnwrap(ChatMember.parse(try apiFixtureJSON("member_json_human.json")))
        XCTAssertEqual(human.handle, "user101")
        XCTAssertNil(human.project)

        let agent = try XCTUnwrap(ChatMember.parse(try apiFixtureJSON("member_json_agent.json")))
        XCTAssertEqual(agent.handle, "fixture")
        XCTAssertEqual(agent.project?.id, "proj_fixture")
    }

    func testProjectMembersFixture() throws {
        let data = try apiFixtureData("project_members_json.json")
        let dto = try JSONDecoder().decode(ProjectMembersDto.self, from: data)
        XCTAssertEqual(dto.members.count, 1)
        let members = dto.members.map { ProjectMember(dto: $0) }
        XCTAssertEqual(members.count, 1)
        XCTAssertEqual(members[0].id, "gh_101")
    }

    func testParseChatWithoutIdReturnsNil() throws {
        XCTAssertNil(RelayAPI.parseChat(["project_id": "proj_fixture", "title": "Nope"] as [String: Any]))
    }

    // MARK: People / me / machine / unread

    func testUserCardParseFixtures() throws {
        let legacy = try XCTUnwrap(Person.parse(try apiFixtureJSON("user_card_legacy.json")))
        XCTAssertEqual(legacy.id, "u_mac")
        XCTAssertEqual(legacy.handle, "jacobs-mac")
        XCTAssertEqual(legacy.name, "Jacob's Mac")

        let presence = try XCTUnwrap(Person.parse(try apiFixtureJSON("user_card_presence.json")))
        XCTAssertEqual(presence.handle, "user301")
        XCTAssertEqual(presence.online, false)
        XCTAssertEqual(presence.avatarColorIndex, 2)

        let github = try XCTUnwrap(Person.parse(try apiFixtureJSON("user_card_github.json")))
        XCTAssertEqual(github.status, "github")
        XCTAssertEqual(github.handle, "ghostuser")
    }

    func testMachineStatusFixture() throws {
        let data = try apiFixtureData("machine_status.json")
        let dto = try JSONDecoder().decode(MachineStatusDto.self, from: data)
        let machine = Machine(dto: dto)
        XCTAssertEqual(machine.state, .hibernatedLight)
        XCTAssertEqual(machine.cpu, 4)
        XCTAssertEqual(machine.memGib, 8)
        XCTAssertEqual(machine.workspaces["proj_fixture"], .idle)
        XCTAssertEqual(dto.user_id, "gh_301")
    }

    func testUnreadFixture() throws {
        let data = try apiFixtureData("unread.json")
        let dto = try JSONDecoder().decode(UnreadDto.self, from: data)
        XCTAssertEqual(dto.total, 2)
        XCTAssertEqual(dto.chats.count, 1)
        XCTAssertEqual(dto.chats[0].chat_id, "people_dm")
        XCTAssertEqual(dto.chats[0].unread, 2)
    }

    // MARK: Calls

    func testCallKitUUIDStablePerCallId() {
        let a1 = callKitUUID(forCallId: "call_fixture")
        let a2 = callKitUUID(forCallId: "call_fixture")
        let b = callKitUUID(forCallId: "call_other")
        XCTAssertEqual(a1, a2)
        XCTAssertNotEqual(a1, b)
    }

    func testVoipPlanIdleRingDuplicateAndBusy() {
        XCTAssertEqual(
            voipPlan(isActive: false, incomingCallId: nil, currentCallId: nil, newCallId: "c1"),
            .ring
        )
        XCTAssertEqual(
            voipPlan(isActive: false, incomingCallId: "c1", currentCallId: nil, newCallId: "c1"),
            .reportDuplicate
        )
        XCTAssertEqual(
            voipPlan(isActive: true, incomingCallId: nil, currentCallId: "c1", newCallId: "c1"),
            .reportDuplicate
        )
        XCTAssertEqual(
            voipPlan(isActive: true, incomingCallId: nil, currentCallId: "c1", newCallId: "c2"),
            .reportAndEnd
        )
        XCTAssertEqual(
            voipPlan(isActive: false, incomingCallId: "c1", currentCallId: nil, newCallId: "c2"),
            .reportAndEnd
        )
    }

    func testAnswerPlanFulfillThenJoinOrFail() {
        XCTAssertEqual(
            answerPlan(hasIncoming: true, micDenied: false),
            .fulfillThenJoin
        )
        XCTAssertEqual(
            answerPlan(hasIncoming: false, micDenied: false),
            .fail
        )
        XCTAssertEqual(
            answerPlan(hasIncoming: true, micDenied: true),
            .fail
        )
        XCTAssertEqual(
            answerPlan(hasIncoming: false, micDenied: true),
            .fail
        )
    }

    func testCallParseFixtures() throws {
        let live = try XCTUnwrap(Call.parse(try apiFixtureJSON("call_live_with_token.json")))
        XCTAssertEqual(live.callId, "call_fixture")
        XCTAssertEqual(live.chatId, "people_call_fixture")
        XCTAssertEqual(live.token, "fixture.join.token")
        XCTAssertEqual(live.startedBy?.handle, "user201")
        XCTAssertEqual(live.participants.count, 1)

        let ended = try XCTUnwrap(Call.parse(try apiFixtureJSON("call_ended.json")))
        XCTAssertEqual(ended.callId, "call_fixture")
        XCTAssertNotNil(ended.endedAt)
        XCTAssertEqual(ended.reason, "last_left")
        XCTAssertNil(ended.token)

        let joinEnv = try apiFixtureData("join_call_response.json")
        let join = try JSONDecoder().decode(CallEnvelopeDto.self, from: joinEnv)
        let joined = Call(dto: join.call)
        XCTAssertEqual(joined.callId, "call_fixture")
        XCTAssertEqual(joined.token, "fixture.join.token")
    }

    func testStartCallResponseFixture() throws {
        let data = try apiFixtureData("start_call_response.json")
        let resp = try JSONDecoder().decode(StartCallResponseDto.self, from: data)
        XCTAssertEqual(resp.joined, false)
        XCTAssertEqual(resp.reachable?.count, 1)
        let call = Call(dto: resp.call, reachable: (resp.reachable ?? []).map(CallReachability.init(dto:)))
        XCTAssertEqual(call.callId, "call_fixture")
        XCTAssertEqual(call.reachable.count, 1)
        XCTAssertEqual(call.reachable[0].userId, "gh_202")
        XCTAssertEqual(call.reachable[0].anyVoip, true)
    }

    func testLeaveAndVoipResponseFixtures() throws {
        let leave = try JSONDecoder().decode(LeaveCallResponseDto.self, from: try apiFixtureData("leave_call_response.json"))
        XCTAssertEqual(leave.ended, true)
        XCTAssertEqual(leave.duration_s, 60)
        XCTAssertNil(leave.already_ended)
    }

    func testCallPayloadFixtures() throws {
        let started = try JSONDecoder().decode(CallStartedPayloadDto.self, from: try apiFixtureData("call_started_payload.json"))
        XCTAssertEqual(started.call.call_id, "call_fixture")
        XCTAssertEqual(started.sender.handle, "user201")
        XCTAssertEqual(Call(dto: started.call).callId, "call_fixture")

        let ended = try JSONDecoder().decode(CallEndedPayloadDto.self, from: try apiFixtureData("call_ended_payload.json"))
        XCTAssertEqual(ended.call_id, "call_fixture")
        XCTAssertEqual(ended.writer_device, "dev_bob_call")

        let missed = try JSONDecoder().decode(CallMissedPayloadDto.self, from: try apiFixtureData("call_missed_payload.json"))
        XCTAssertEqual(missed.from.handle, "user201")
        XCTAssertEqual(CallPerson(dto: missed.from).handle, "user201")

        let ring = try JSONDecoder().decode(CallRingPayloadDto.self, from: try apiFixtureData("call_ring_payload.json"))
        XCTAssertEqual(ring.caller.handle, "user201")
        XCTAssertEqual(ring.room, "call_room_fixture")

        let alert = try JSONDecoder().decode(CallAlertDto.self, from: try apiFixtureData("call_alert.json"))
        XCTAssertEqual(alert.event, "started")
        XCTAssertEqual(alert.call.call_id, "call_fixture")
        XCTAssertEqual(alert.sender?.handle, "user201")
    }

    func testCallPersonMetadataFixture() throws {
        let data = try apiFixtureData("call_live_with_token.json")
        let dto = try JSONDecoder().decode(CallDto.self, from: data)
        let meta = try JSONEncoder().encode(dto.started_by)
        let person = try XCTUnwrap(CallPerson.parse(metadata: String(data: meta, encoding: .utf8)))
        XCTAssertEqual(person.id, "gh_201")
        XCTAssertEqual(person.handle, "user201")
    }

    // MARK: Goals

    func testGoalParseFixtures() throws {
        let raw = try apiFixtureJSON("goal_json.json")
        let goal = try XCTUnwrap(GoalItem.parse(raw))
        XCTAssertEqual(goal.id, "goal_fixture")
        XCTAssertEqual(goal.projectId, "proj_fixture")
        XCTAssertEqual(goal.projectName, "Fixture")
        XCTAssertEqual(goal.title, "Ship the DTO step")
        XCTAssertEqual(goal.status, "active")
        XCTAssertEqual(goal.autonomy, "suggest")
        XCTAssertEqual(goal.plan.count, 2)
        XCTAssertEqual(goal.plan[0].title, "Add GoalDto")
        XCTAssertEqual(goal.plan[0].status, "done")
        XCTAssertEqual(goal.plan[1].status, "doing")
        XCTAssertEqual(goal.progressLog.count, 1)
        XCTAssertEqual(goal.progressLog[0].text, "Started the shared client core step.")
        XCTAssertEqual(goal.linkedChatIds, ["chat_primary"])
        XCTAssertEqual(goal.checkInCron, "0 9 * * 1-5")

        let listData = try apiFixtureData("goals_list.json")
        let list = try JSONDecoder().decode(GoalsListDto.self, from: listData)
        XCTAssertEqual(list.goals.count, 1)
        XCTAssertEqual(list.goals[0].id, "goal_fixture")
        let rawList = try apiFixtureJSON("goals_list.json") as? [String: Any]
        let first = try XCTUnwrap((rawList?["goals"] as? [Any])?.first)
        let listed = try XCTUnwrap(GoalItem.parse(first))
        XCTAssertEqual(listed.id, "goal_fixture")
        XCTAssertEqual(listed.plan.count, 2)
        XCTAssertEqual(listed.progressLog.count, 1)
    }

    // MARK: Secrets / vaults

    func testSecretsParseFixtures() throws {
        let secret = try XCTUnwrap(SecretItem.parse(try apiFixtureJSON("secret_json.json")))
        XCTAssertEqual(secret.id, "sec_fixture")
        XCTAssertEqual(secret.env, "FIXTURE_STRIPE_KEY")
        XCTAssertEqual(secret.agents.count, 1)
        XCTAssertEqual(secret.agents[0].name, "Research")
        XCTAssertEqual(secret.directors.count, 1)
        XCTAssertEqual(secret.directors[0].handle, "user502")

        let login = try XCTUnwrap(SecretItem.parse(try apiFixtureJSON("secret_login_json.json")))
        XCTAssertEqual(login.kind, "login")
        XCTAssertEqual(login.username, "fixture.login")

        let request = try XCTUnwrap(SecretRequestItem.parse(try apiFixtureJSON("secret_request_json.json")))
        XCTAssertEqual(request.id, "req_secret_fixture")
        XCTAssertEqual(request.name, "FIXTURE_STRIPE_KEY")
        XCTAssertTrue(request.isPending)

        let reason = try XCTUnwrap(SecretActivity.parse(try apiFixtureJSON("secret_activity_reason.json")))
        XCTAssertEqual(reason.action, "refused")
        XCTAssertEqual(reason.reason, "host_not_allowed")
        XCTAssertEqual(reason.targetKind, "secret")

        let decision = try XCTUnwrap(SecretActivity.parse(try apiFixtureJSON("secret_activity_decision.json")))
        XCTAssertEqual(decision.decision, "allowed_always")

        let vault = try XCTUnwrap(ConnectedVault.parse(try apiFixtureJSON("vault_json.json")))
        XCTAssertEqual(vault.id, "vault_fixture")
        XCTAssertEqual(vault.providerLabel, "1Password")
        XCTAssertEqual(vault.itemsCount, 1)
        XCTAssertEqual(vault.agents.count, 1)

        let overview = try XCTUnwrap(SecretsOverview.parse(try apiFixtureJSON("secrets_overview.json")))
        XCTAssertEqual(overview.secrets.count, 2)
        XCTAssertEqual(overview.vaults.count, 1)
        XCTAssertEqual(overview.requestsPending, 1)
        XCTAssertEqual(overview.activity.count, 2)
        XCTAssertEqual(overview.activity[0].reason, "host_not_allowed")
        XCTAssertEqual(overview.activity[1].decision, "allowed_always")

        let project = try XCTUnwrap(ProjectSecrets.parse(try apiFixtureJSON("project_secrets_json.json")))
        XCTAssertTrue(project.owner)
        XCTAssertEqual(project.items.count, 2)
        XCTAssertEqual(project.items[0].targetKind, "secret")
        XCTAssertEqual(project.items[0].secret?.env, "FIXTURE_STRIPE_KEY")
        XCTAssertEqual(project.items[1].targetKind, "vault")
        XCTAssertEqual(project.items[1].vault?.name, "Fixture 1Password")

        let access = try XCTUnwrap(SecretAgentAccess.parse(try apiFixtureJSON("secret_access_json.json")))
        XCTAssertEqual(access.id, "acc_secret_fixture")
        XCTAssertEqual(access.projectId, "proj_secrets_fixture")

        let pausedData = try apiFixtureData("secrets_paused.json")
        let paused = try JSONDecoder().decode(SecretsPausedDto.self, from: pausedData)
        XCTAssertTrue(paused.paused)
        XCTAssertEqual(paused.paused_by, "user")

        let decideData = try apiFixtureData("decide_secret_response.json")
        let decide = try JSONDecoder().decode(DecideSecretResponseDto.self, from: decideData)
        XCTAssertEqual(decide.request.status, "allowed_always")
        XCTAssertEqual(decide.access?.access_id, "acc_secret_fixture")
    }

    // MARK: Sign-in / pairing / media

    func testPairTokenHealthVoicesFixtures() throws {
        let pair = try JSONDecoder().decode(PairResponseDto.self, from: try apiFixtureData("pair_response.json"))
        XCTAssertEqual(pair.device_id, "dev_fixture")
        XCTAssertEqual(pair.device_token, "adv_fixture_not_a_real_token")

        let token = try JSONDecoder().decode(TokenResponseDto.self, from: try apiFixtureData("token_response.json"))
        let (toks, user) = RelayAPI.parseTokens(token, relayUrl: "http://relay.test")
        XCTAssertEqual(toks.deviceId, "dev_fixture")
        XCTAssertFalse(toks.accessToken.isEmpty)
        XCTAssertEqual(user?.login, "user301")

        let grace = try JSONDecoder().decode(TokenResponseDto.self, from: try apiFixtureData("token_response_refresh_grace.json"))
        XCTAssertEqual(grace.access_token, token.access_token)
        XCTAssertEqual(grace.refresh_token, token.refresh_token)

        let health = try JSONDecoder().decode(HealthDto.self, from: try apiFixtureData("healthz.json"))
        XCTAssertEqual(health.auth_mode, "github")
        XCTAssertEqual(health.ok, true)
        if let mode = health.auth_mode, let parsed = AuthMode(rawValue: mode) {
            XCTAssertEqual(parsed, .github)
        } else {
            XCTFail("healthz auth_mode")
        }

        let voices = try JSONDecoder().decode(VoiceCatalogDto.self, from: try apiFixtureData("voices.json"))
        XCTAssertEqual(voices.`default`, "marin")
        XCTAssertEqual(voices.voices.count, 1)
        XCTAssertEqual(voices.voices[0].id, "marin")
        let catalog = Arbos.VoiceCatalog(
            defaultId: voices.`default`,
            voices: voices.voices.map {
                Arbos.Voice(id: $0.id, name: $0.name, description: $0.description, seconds: $0.seconds, sampleURL: URL(string: $0.sample_url))
            }
        )
        XCTAssertEqual(catalog.defaultId, "marin")
        XCTAssertEqual(catalog.voices.count, 1)
    }

    // MARK: Pushes / releases / push register

    func testPushItemParseFixtures() throws {
        let names = [
            "push_people_message.json",
            "push_people_chat_updated.json",
            "push_people_member_added.json",
            "push_people_member_removed.json",
            "push_people_chat_created.json",
            "push_people_chat_deleted.json",
            "push_people_agent_added.json",
            "push_people_agent_removed.json",
            "push_people_project_shared.json",
            "push_project_member_removed.json",
            "push_chat_archived.json",
            "push_people_invite_accepted.json",
            "push_feedback_capture.json",
            "push_server_member_removed.json",
            "push_server_member_joined.json",
            "push_server_updated.json",
            "push_server_deleted.json",
            "push_call_started.json",
            "push_call_ended.json",
            "push_call_missed.json",
            "push_machine_ready.json",
            "push_release_published.json",
            "push_secret_request.json",
            "push_secret_request_secret.json",
            "push_secrets_paused.json",
            "push_secret_expiring.json",
            "push_profile_updated.json",
        ]
        for name in names {
            let item = try XCTUnwrap(PushItem.parse(try apiFixtureJSON(name)), name)
            XCTAssertFalse(item.id.isEmpty, name)
            XCTAssertGreaterThan(item.createdAt, 0, name)
        }

        let people = try XCTUnwrap(PushItem.parse(try apiFixtureJSON("push_people_message.json")))
        guard case .peopleMessage(let msg) = people.event else {
            return XCTFail("people_message")
        }
        XCTAssertEqual(msg.chat_id, "people_dm")
        XCTAssertEqual(msg.sender.handle, "user201")

        let started = try XCTUnwrap(PushItem.parse(try apiFixtureJSON("push_call_started.json")))
        guard case .callStarted(let s) = started.event else { return XCTFail("call_started") }
        XCTAssertEqual(s.call.call_id, "call_fixture")

        let capture = try XCTUnwrap(PushItem.parse(try apiFixtureJSON("push_feedback_capture.json")))
        guard case .feedbackCapture(let f) = capture.event else { return XCTFail("feedback_capture") }
        XCTAssertEqual(f.request_id, "cap_fixture")
        XCTAssertEqual(f.expires_at, 1_700_003_600_000)

        let ready = try XCTUnwrap(PushItem.parse(try apiFixtureJSON("push_machine_ready.json")))
        guard case .machineReady(let r) = ready.event else { return XCTFail("machine_ready") }
        XCTAssertEqual(r.message, "Your Arbos computer is ready.")

        let secret = try XCTUnwrap(PushItem.parse(try apiFixtureJSON("push_secret_request_secret.json")))
        guard case .secretRequest(let s) = secret.event else { return XCTFail("secret_request") }
        XCTAssertEqual(s.request_id, "req_secret_fixture")
        XCTAssertEqual(s.host, "api.stripe.com")
        XCTAssertNil(s.vault_id)

        let profile = try XCTUnwrap(PushItem.parse(try apiFixtureJSON("push_profile_updated.json")))
        guard case .profileUpdated(let p) = profile.event else { return XCTFail("profile_updated") }
        XCTAssertEqual(p.user_id, "gh_201")
        XCTAssertEqual(p.avatar_url, "https://relay.example/avatars/gh_201.jpg?v=1700000000000")

        let agentProfile = try XCTUnwrap(PushItem.parse(try apiFixtureJSON("push_agent_profile_updated.json")))
        guard case .agentProfileUpdated(let a) = agentProfile.event else { return XCTFail("agent_profile_updated") }
        XCTAssertEqual(a.project_id, "01FIXTUREAGENT")
        XCTAssertEqual(a.avatar_version, 1_700_000_000_000)
        XCTAssertEqual(a.image_key, "avatar-1700000000000.jpg")

        let cleared = try XCTUnwrap(PushItem.parse(try apiFixtureJSON("push_notifications_cleared.json")))
        guard case .notificationsCleared(let n) = cleared.event else { return XCTFail("notifications_cleared") }
        XCTAssertEqual(n.chat_id, "people_dm")
        XCTAssertEqual(n.project_id, "people")
        XCTAssertEqual(n.badge, 2)

        XCTAssertNil(PushItem.parse(try apiFixtureJSON("push_unknown_kind.json")))
        XCTAssertNil(PushItem.parse([
            "id": "bad", "created_at": 1, "kind": "not_a_real_push_kind", "payload": ["x": 1],
        ] as [String: Any]))
    }

    func testReleasesAndPushRegisterFixtures() throws {
        let releases = try JSONDecoder().decode(ReleasesDto.self, from: try apiFixtureData("releases_current.json"))
        XCTAssertEqual(releases.releases.count, 2)
        XCTAssertEqual(releases.releases.first { $0.platform == "ios" }?.build, "456")

        let reg = try JSONDecoder().decode(PushRegisterResponseDto.self, from: try apiFixtureData("push_register_response.json"))
        XCTAssertEqual(reg.device_id, "dev_fixture")
        XCTAssertEqual(reg.environment, "production")
        XCTAssertEqual(reg.push, true)
    }

    func testConnectorsFixtures() throws {
        let catalog = try JSONDecoder().decode(ConnectorsCatalogDto.self, from: try apiFixtureData("connectors_catalog.json"))
        let list = try JSONDecoder().decode(ConnectorsListDto.self, from: try apiFixtureData("connectors_list.json"))
        let view = ConnectorsView(catalog: catalog, list: list)
        XCTAssertTrue(view.loaded)
        XCTAssertTrue(view.mock)
        XCTAssertEqual(view.providers.map(\.provider), ["google", "slack", "github", "harness_comms"])
        let github = try XCTUnwrap(view.providers.first { $0.provider == "github" })
        XCTAssertTrue(github.hasOauth)
        XCTAssertTrue(github.acceptsToken)
        XCTAssertFalse(github.oauthConfigured)
        XCTAssertEqual(view.connector(for: "github")?.id, "conn_fixture")
        XCTAssertEqual(view.connector(for: "github")?.status, .connected)

        let provider = try JSONDecoder().decode(ProviderDto.self, from: try apiFixtureData("provider_json.json"))
        XCTAssertEqual(ConnectorProvider(dto: provider).connect, "oauth_or_token")
        XCTAssertFalse(provider.actions.isEmpty)

        let connector = Connector(dto: try JSONDecoder().decode(ConnectorDto.self, from: try apiFixtureData("connector_json.json")))
        XCTAssertEqual(connector.label, "GitHub · token")
        XCTAssertNil(connector.lastError)
        XCTAssertTrue(connector.mock)

        let auth = try JSONDecoder().decode(ConnectConnectorReplyDto.self, from: try apiFixtureData("connect_reply_auth.json"))
        XCTAssertEqual(auth.state, "oauth_state_fixture")
        XCTAssertTrue(auth.authUrl?.contains("state=oauth_state_fixture") ?? false)
        XCTAssertNil(auth.connector)

        let stored = try JSONDecoder().decode(ConnectConnectorReplyDto.self, from: try apiFixtureData("connect_reply_connector.json"))
        XCTAssertNil(stored.authUrl)
        XCTAssertEqual(stored.connector?.id, "conn_fixture")
    }

    func testApiErrorDtoDecodesIntoRelayError() throws {
        let converted = try apiFixtureData("api_error_converted.json")
        let fromFixture = RelayError.from(status: 404, body: converted)
        XCTAssertEqual(fromFixture.status, 404)
        XCTAssertEqual(fromFixture.type, "not_found")
        XCTAssertEqual(fromFixture.message, "no such project")

        let body = Data(#"{"error":{"type":"token_expired","message":"access token expired"}}"#.utf8)
        let typed = RelayError.from(status: 401, body: body)
        XCTAssertEqual(typed.status, 401)
        XCTAssertEqual(typed.type, "token_expired")
        XCTAssertEqual(typed.message, "access token expired")

        let plain = RelayError.from(status: 503, body: Data("relay restarting".utf8))
        XCTAssertEqual(plain.status, 503)
        XCTAssertNil(plain.type)
        XCTAssertEqual(plain.message, "relay restarting")
    }

    func testChannelsSkillsLocalFeedbackFixtures() throws {
        let overview = try JSONDecoder().decode(ChannelsOverviewDto.self, from: try apiFixtureData("channels_overview.json"))
        XCTAssertTrue(overview.mock)
        XCTAssertEqual(overview.platforms.count, 4)
        XCTAssertEqual(overview.bindings.first?.externalChatId, "tg_chat_fixture")
        XCTAssertEqual(overview.pairing?.code, "PAIR42")

        let binding = try JSONDecoder().decode(ChannelBindingDto.self, from: try apiFixtureData("channel_binding_json.json"))
        XCTAssertEqual(binding.projectId, "proj_fixture")
        XCTAssertEqual(binding.bound_at, 1_700_000_000_000)

        let pair = try JSONDecoder().decode(ChannelPairingDto.self, from: try apiFixtureData("channel_pair_json.json"))
        XCTAssertEqual(pair.chatId, "chat_primary")
        XCTAssertEqual(pair.expiresAt, 4_102_444_800_000)

        let skill = try JSONDecoder().decode(SkillDto.self, from: try apiFixtureData("skill_json.json"))
        XCTAssertEqual(skill.slug, "release-notes")
        XCTAssertEqual(skill.version, 2)
        XCTAssertTrue(skill.markdown.hasPrefix("---\nname: \"Release notes\""))

        let devices = try JSONDecoder().decode(LocalDevicesDto.self, from: try apiFixtureData("local_devices.json"))
        XCTAssertEqual(devices.items.first?.device_id, "ldev_fixture")
        XCTAssertEqual(devices.items.first?.connected, false)

        let feedback = try JSONDecoder().decode(FeedbackSubmitReplyDto.self, from: try apiFixtureData("feedback_submit_reply.json"))
        XCTAssertEqual(feedback.id, "fb_fixture")
        XCTAssertFalse(feedback.has_image)
        XCTAssertNil(feedback.duplicate)
    }

    /// Notification open must wait for store docs when a store is open and not loaded yet.
    func testShouldDeferRoomOpen() {
        XCTAssertTrue(AppModel.shouldDeferRoomOpen(hasStore: true, storeDocsLoaded: false))
        XCTAssertFalse(AppModel.shouldDeferRoomOpen(hasStore: true, storeDocsLoaded: true))
        XCTAssertFalse(AppModel.shouldDeferRoomOpen(hasStore: false, storeDocsLoaded: false))
        XCTAssertFalse(AppModel.shouldDeferRoomOpen(hasStore: false, storeDocsLoaded: true))
    }

    /// #1438: the sidebar count comes from the relay's per-user cursor (`/me/unread` and the
    /// per-chat flag), never from `head_seq - read_seq` — own rows and system lines bumped
    /// that and painted false blue dots. On screen nothing is ever unread.
    func testPeopleUnreadCountFollowsTheUserCursorNotRowSeqs() {
        // Own message (any device): the relay lists nothing and the flag is off → clear.
        XCTAssertEqual(AppModel.peopleUnreadCount(remote: nil, flagged: false, watching: false), 0)
        // Message read while viewing: on screen → clear even if the remote count lags.
        XCTAssertEqual(AppModel.peopleUnreadCount(remote: 2, flagged: true, watching: true), 0)
        // Genuine unseen content from someone else, off screen.
        XCTAssertEqual(AppModel.peopleUnreadCount(remote: 3, flagged: false, watching: false), 3)
        XCTAssertEqual(AppModel.peopleUnreadCount(remote: nil, flagged: true, watching: false), 1)
        // Live row lit the flag before /me/unread caught up: a stale remote 0 never hides it.
        XCTAssertEqual(AppModel.peopleUnreadCount(remote: 0, flagged: true, watching: false), 1)
        // Relay cleared it (read on another device): nothing local keeps it lit.
        XCTAssertEqual(AppModel.peopleUnreadCount(remote: 0, flagged: false, watching: false), 0)
    }

    /// Clear on read (ARB-444): a `notifications_cleared` push from another device of mine
    /// takes one chat out of the badge arithmetic — its remote count and its live-row flag —
    /// and leaves every other chat's count alone; the badge then follows the smaller of the
    /// relay's total and the local sum, since each can only be stale-high.
    func testClearOnReadBadgeArithmetic() {
        let hidden: (String) -> Bool = { $0 == "hidden-plain" }
        var remote = ["dm": 2, "group": 1, "hidden-plain": 4]
        var flagged: Set<String> = ["dm", "live-only"]
        // Before: 2 + 1 (+ 0 for the hidden chat) + 1 for the chat only a live row flagged.
        XCTAssertEqual(AppModel.badgeTotal(remote: remote, flagged: flagged, hidden: hidden), 4)

        // The Mac read the DM: its count and flag go, nothing else moves.
        AppModel.clearingChat("dm", remote: &remote, flagged: &flagged)
        XCTAssertEqual(remote["dm"], 0)
        XCTAssertEqual(remote["group"], 1)
        XCTAssertFalse(flagged.contains("dm"))
        XCTAssertTrue(flagged.contains("live-only"))
        XCTAssertEqual(AppModel.badgeTotal(remote: remote, flagged: flagged, hidden: hidden), 2)

        // A clear for a chat the relay never counted here only drops its flag.
        AppModel.clearingChat("live-only", remote: &remote, flagged: &flagged)
        XCTAssertNil(remote["live-only"])
        XCTAssertEqual(AppModel.badgeTotal(remote: remote, flagged: flagged, hidden: hidden), 1)

        // Badge after the clear: relay total wins when a local flag is stale …
        XCTAssertEqual(AppModel.badgeAfterClear(relayTotal: 1, localTotal: 2), 1)
        // … the local sum wins when this device's own read has not reached the relay yet
        // (or the relay counts a hidden plain chat the badge never shows) …
        XCTAssertEqual(AppModel.badgeAfterClear(relayTotal: 5, localTotal: 1), 1)
        // … they agree when both are current, and nothing goes below zero.
        XCTAssertEqual(AppModel.badgeAfterClear(relayTotal: 0, localTotal: 0), 0)
        XCTAssertEqual(AppModel.badgeAfterClear(relayTotal: -1, localTotal: 3), 3)
        XCTAssertEqual(AppModel.badgeAfterClear(relayTotal: 2, localTotal: -4), 0)
    }

    /// Mark-read PUT: skip same value, send after the window, delay inside it.
    /// With `force`, an unchanged value still sends outside the window / delays inside it.
    func testReadPutPlan() {
        let t0 = Date(timeIntervalSince1970: 1_700_000_000)
        XCTAssertEqual(AppModel.readPutPlan(lastSent: nil, lastSentAt: nil, value: 1, now: t0), .send)
        XCTAssertEqual(AppModel.readPutPlan(lastSent: 5, lastSentAt: t0, value: 5, now: t0.addingTimeInterval(10)), .skip)
        XCTAssertEqual(
            AppModel.readPutPlan(lastSent: 5, lastSentAt: t0, value: 6, now: t0.addingTimeInterval(AppModel.readPutThrottleSeconds)),
            .send
        )
        XCTAssertEqual(
            AppModel.readPutPlan(lastSent: 5, lastSentAt: t0, value: 6, now: t0.addingTimeInterval(AppModel.readPutThrottleSeconds + 0.1)),
            .send
        )
        switch AppModel.readPutPlan(lastSent: 5, lastSentAt: t0, value: 6, now: t0.addingTimeInterval(0.5)) {
        case .delay(let seconds):
            XCTAssertEqual(seconds, AppModel.readPutThrottleSeconds - 0.5, accuracy: 0.000_1)
        case .send, .skip:
            XCTFail("expected delay inside the throttle window")
        }
        // force + unchanged: send outside the window, delay inside it; unforced still skips.
        XCTAssertEqual(
            AppModel.readPutPlan(lastSent: 5, lastSentAt: t0, value: 5, now: t0.addingTimeInterval(10), force: true),
            .send
        )
        switch AppModel.readPutPlan(lastSent: 5, lastSentAt: t0, value: 5, now: t0.addingTimeInterval(0.5), force: true) {
        case .delay(let seconds):
            XCTAssertEqual(seconds, AppModel.readPutThrottleSeconds - 0.5, accuracy: 0.000_1)
        case .send, .skip:
            XCTFail("expected delay for forced unchanged value inside the window")
        }
        XCTAssertEqual(
            AppModel.readPutPlan(lastSent: 5, lastSentAt: t0, value: 5, now: t0.addingTimeInterval(10), force: false),
            .skip
        )
    }

    /// Registry prune keeps listed chats, listed project docs, people keys and
    /// recent keys; everything else is returned for wipe.
    func testRoomsToPrune() {
        let keys: Set<String> = ["chat-a", "chat-b", "proj:p1", "proj:p2", "people-1", "gone", "recent-x"]
        let pruned = AppModel.roomsToPrune(
            roomKeys: keys,
            listedChatIds: ["chat-a"],
            listedProjectIds: ["p1"],
            peopleKeys: ["people-1"],
            recentKeys: ["recent-x"]
        )
        XCTAssertEqual(pruned, ["chat-b", "proj:p2", "gone"])
    }

    /// Recreate below 3 failures; stop at 3 within 10 minutes; allow again after.
    func testShouldRecreateRoom() {
        let t0 = Date(timeIntervalSince1970: 1_700_000_000)
        XCTAssertTrue(AppModel.shouldRecreateRoom(failures: 0, lastFailureAt: nil, now: t0))
        XCTAssertTrue(AppModel.shouldRecreateRoom(failures: 2, lastFailureAt: t0, now: t0))
        XCTAssertFalse(AppModel.shouldRecreateRoom(failures: 3, lastFailureAt: t0, now: t0.addingTimeInterval(60)))
        XCTAssertFalse(AppModel.shouldRecreateRoom(
            failures: 3,
            lastFailureAt: t0,
            now: t0.addingTimeInterval(AppModel.roomUnavailableBackoffSeconds - 1)
        ))
        XCTAssertTrue(AppModel.shouldRecreateRoom(
            failures: 3,
            lastFailureAt: t0,
            now: t0.addingTimeInterval(AppModel.roomUnavailableBackoffSeconds)
        ))
    }

    /// Incomplete registry list must not prune rooms (a bad list item could omit a live project).
    func testShouldPrune() {
        XCTAssertTrue(AppModel.shouldPrune(incomplete: false))
        XCTAssertFalse(AppModel.shouldPrune(incomplete: true))
    }

    /// Skipped detail keeps the previous project and chats; neither-fresh-nor-skipped is dropped.
    func testMergeSkippedProjects() {
        let kept = Self.fixtureProject(id: "skip-me", name: "Kept")
        let dropped = Self.fixtureProject(id: "gone", name: "Gone")
        let fresh = Self.fixtureProject(id: "fresh", name: "Fresh")
        let keptChat = Chat(
            id: "chat-skip",
            projectId: "skip-me",
            title: "Saved",
            isPinned: false,
            unread: true,
            lastMessageAt: 42
        )
        let droppedChat = Chat(
            id: "chat-gone",
            projectId: "gone",
            title: "Old",
            isPinned: false,
            unread: false,
            lastMessageAt: 1
        )
        let merged = AppModel.mergeSkippedProjects(
            fresh: [fresh],
            previous: [kept, dropped],
            previousChats: [
                "skip-me": [keptChat],
                "gone": [droppedChat],
            ],
            skippedIds: ["skip-me"]
        )
        XCTAssertEqual(Set(merged.projects.map(\.id)), ["fresh", "skip-me"])
        XCTAssertEqual(merged.chats.keys.sorted(), ["skip-me"])
        XCTAssertEqual(merged.chats["skip-me"], [keptChat])
        XCTAssertNil(merged.chats["gone"])
    }

    private static func fixtureProject(id: String, name: String) -> Project {
        Project(
            id: id,
            name: name,
            icon: "Ω",
            color: "#000000",
            state: .hibernatedLight,
            model: "anthropic/claude-haiku-5.5",
            effort: .low,
            spendTodayUsd: 0,
            spendCapUsd: 0
        )
    }
}
