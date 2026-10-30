import XCTest
@testable import Arbos

/// Sub-agent job tabs (feedback `subagent-job-tabs-ios-2026-10-25`): a job's exchange becomes the
/// transcript of a `job:<id>` chat — brief → user line, words → agent lines, tool steps → system
/// lines ("· 4.8 s" done, "…" live), result → agent line, a streaming row with the live step while
/// the job runs; the relay document parses tolerantly and merges by `seq`.
final class JobTabsTests: XCTestCase {
    private func entry(_ seq: Int64, _ kind: RolloutKind, _ words: String, status: String? = nil, dur: Int64? = nil) -> RolloutEntry {
        let tool = kind == .tool
        return RolloutEntry(seq: seq, at: 1_700_000_000_000 + seq * 1000, kind: kind,
                            text: tool ? nil : words, label: tool ? words : nil, name: tool ? "tool" : nil,
                            status: status, durationMs: dur)
    }

    private func rollout(_ entries: [RolloutEntry], ended: Bool) -> JobRollout {
        JobRollout(id: "j1", title: "Searching for surf schools", kind: "research", state: .running,
                   step: "Reading tripadvisor.com", elapsedS: 41, startedAt: 1_700_000_000_000,
                   endedAt: ended ? 1_700_000_400_000 : nil, chatId: "c1", askMessageId: "m1",
                   parentProjectId: "p1", parentName: "Affine", brief: "Find the 3 best surf schools.",
                   entries: entries, toolCalls: 2, nextSeq: 10)
    }

    func testARunningJobReadsAsBriefWordsStepsAndALiveRow() {
        let r = rollout([
            entry(1, .prompt, "Find the 3 best surf schools."),
            entry(2, .text, "On it."),
            entry(3, .tool, "Read tripadvisor.com", status: "done", dur: 4800),
            entry(4, .tool, "Reading delsoulsurf.com", status: "running"),
        ], ended: false)
        let m = JobTabs.messages(r)
        XCTAssertEqual(m.count, 5)
        XCTAssertEqual(m[0].role, .user); XCTAssertEqual(m[0].plainText, "Find the 3 best surf schools.")
        XCTAssertEqual(m[1].role, .assistant); XCTAssertEqual(m[1].plainText, "On it.")
        XCTAssertEqual(m[2].role, .system); XCTAssertEqual(m[2].plainText, "Read tripadvisor.com · 4.8 s")
        XCTAssertEqual(m[3].plainText, "Reading delsoulsurf.com…")
        XCTAssertTrue(m[4].isStreaming, "a live row keeps the presence row and Stop up")
        XCTAssertEqual(m[4].activity, "Reading delsoulsurf.com")
        XCTAssertEqual(m[0].chatId, "job:j1")
        XCTAssertTrue(r.isRunning)
        XCTAssertEqual(r.statusShort, "Running · 0:41")
    }

    func testAFinishedJobEndsWithTheResultAndNoLiveRow() {
        let r = rollout([
            entry(1, .prompt, "Find the 3 best surf schools."),
            entry(2, .tool, "Read tripadvisor.com", status: "done", dur: 4800),
            entry(3, .result, "Blue Mystic is the best value.", status: "done", dur: 372_000),
        ], ended: true)
        let m = JobTabs.messages(r)
        XCTAssertEqual(m.count, 3)
        XCTAssertEqual(m[2].role, .assistant)
        XCTAssertEqual(m[2].plainText, "Blue Mystic is the best value.")
        XCTAssertFalse(m.contains { $0.isStreaming })
        XCTAssertFalse(r.isRunning)
        XCTAssertEqual(r.statusShort, "Finished · 0:41")
        // The transcript renders the steps as system lines and the result as a bubble; nothing hidden.
        let rows = Transcript.rows(m, projectId: "p1")
        XCTAssertTrue(rows.contains { $0.messageId == "job:j1:2" }, "the step line: \(rows.map(\.id))")
        XCTAssertTrue(rows.contains { $0.messageId == "job:j1:3" }, "the result: \(rows.map(\.id))")
    }

    func testATypedSteerShowsUntilTheBoxEchoesIt() {
        let typed = JobTabs.typedSteer(chat: "job:j1", messageId: "local-1", text: "only group lessons under $70", ask: "m1")
        XCTAssertEqual(typed.replyTo, "m1")
        let r0 = rollout([entry(1, .prompt, "Find schools.")], ended: false)
        XCTAssertTrue(JobTabs.messages(r0, pending: [typed]).contains { $0.id == "local-1" })
        let r1 = rollout([entry(1, .prompt, "Find schools."), entry(2, .steer, "only group lessons under $70")], ended: false)
        let m = JobTabs.messages(r1, pending: [typed])
        XCTAssertFalse(m.contains { $0.id == "local-1" }, "the echo replaces it")
        XCTAssertEqual(m[1].role, .user)
    }

    func testTheRelayDocumentParsesAndMergesBySeq() {
        let raw: [String: Any] = [
            "id": "j1", "title": "Searching for surf schools", "kind": "research", "state": "running",
            "step": "Reading tripadvisor.com", "elapsed_s": 51, "started_at": 1_700_000_000_000,
            "chat_id": "c1", "ask_message_id": "m1", "parent": ["project_id": "p1", "name": "Affine"],
            "brief": "Find the 3 best surf schools.",
            "entries": [
                ["seq": 1, "at": 1_700_000_001_000, "kind": "prompt", "text": "Find the 3 best surf schools."],
                ["seq": 2, "at": 1_700_000_004_000, "kind": "tool", "name": "fetch", "label": "Reading tripadvisor.com", "status": "running"],
                ["bogus": true],
            ],
            "tool_calls": 1, "next_seq": 3,
        ]
        guard var doc = JobRollout(raw) else { return XCTFail("parses") }
        XCTAssertEqual(doc.entries.map(\.seq), [1, 2])
        XCTAssertEqual(doc.parentName, "Affine")
        XCTAssertTrue(doc.isRunning)
        let newer: [String: Any] = [
            "id": "j1", "title": "Searching for surf schools", "state": "running", "elapsed_s": 60,
            "entries": [
                ["seq": 2, "at": 1_700_000_004_000, "kind": "tool", "label": "Read tripadvisor.com", "status": "done", "duration_ms": 4800],
                ["seq": 3, "at": 1_700_000_009_000, "kind": "text", "text": "On it."],
            ],
            "tool_calls": 1, "next_seq": 4,
        ]
        doc.merge(JobRollout(newer)!)
        XCTAssertEqual(doc.entries.map(\.seq), [1, 2, 3], "appended by seq, the step replaced")
        XCTAssertEqual(doc.entries[1].status, "done")
        XCTAssertEqual(doc.entries[1].durationLabel, "4.8 s")
        XCTAssertEqual(doc.nextSeq, 4)
        XCTAssertEqual(doc.elapsedS, 60)
        XCTAssertNil(JobRollout(["title": "no id"]))
    }

    func testIdsAndTabsRoundTrip() {
        XCTAssertEqual(JobTabs.chatId("01M"), "job:01M")
        XCTAssertEqual(JobTabs.jobId("job:01M"), "01M")
        XCTAssertNil(JobTabs.jobId("01M"))
        XCTAssertTrue(JobTabs.isJobChat("job:x")); XCTAssertFalse(JobTabs.isJobChat("c1"))
        let tab = JobTabs.tab(projectId: "p1", jobId: "j1", title: "", at: 5)
        XCTAssertEqual(tab.title, "Job"); XCTAssertEqual(tab.kind, "job"); XCTAssertEqual(tab.projectId, "p1")
    }
}
